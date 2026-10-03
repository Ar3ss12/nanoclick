//! Passive foreground patrol — `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)`.
//!
//! Why a hook and not a poller: the patrol must cost 0% CPU while the user
//! works. Windows calls us exactly once per foreground switch; between two
//! switches this module owns no timer, no sleep, no busy loop.
//!
//! The contract is the same as for `WH_KEYBOARD_LL` (AGENTS.md §2.17) — the
//! callback runs inside the system event chain, so it may do NOTHING but a
//! lock-free hand-over:
//!
//! * no lock, no `Mutex`/`RwLock`/`OnceLock` read on the callback path;
//! * no allocation, no `String`/`Vec`, no file I/O, no `emit`, no log;
//! * no scheduler call, no `is_window_elevated` (three syscalls), no balloon.
//!
//! The worker lives on the SAME thread that installed the hook (the hook
//! needs its installing thread to pump messages): the channel drains between
//! pumps. Heavy work — throttle, own-PID skip, elevation probe, sound,
//! notification, Z-ram dispatch via `run_on_main_thread` — never runs in the
//! callback.
//!
//! Lifecycle mirrors `windows_hooks.rs`: `start_foreground_watch` installs
//! the hook and spawns the thread, `stop_foreground_watch` latches the flag,
//! posts `WM_QUIT` to break the pump, unhooks, and joins. Idempotent, never
//! panics, safe from shutdown paths.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::thread::JoinHandle;
use tauri::Manager;

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
#[cfg(target_os = "windows")]
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, PostThreadMessageW, TranslateMessage, MSG, WM_QUIT,
};

/// `EVENT_SYSTEM_FOREGROUND` — the foreground window changed.
/// (lives in `WindowsAndMessaging` in the `windows` 0.52 bindings, not in
/// `Accessibility` next to `SetWinEventHook` itself.)
#[cfg(target_os = "windows")]
const EVENT_SYSTEM_FOREGROUND: u32 = 3;
/// Out-of-context delivery: the callback runs on OUR thread, never injected
/// into the foreground process. Mandatory for a passive observer.
#[cfg(target_os = "windows")]
const WINEVENT_OUTOFCONTEXT: u32 = 0x0000;
/// Never call us for our own windows: NanoClick restoring itself must not
/// re-trigger the patrol.
#[cfg(target_os = "windows")]
const WINEVENT_SKIPOWNPROCESS: u32 = 0x0002;

static STOP_FLAG: AtomicBool = AtomicBool::new(false);
static WATCH_RUNNING: AtomicBool = AtomicBool::new(false);
static WATCH_THREAD_ID: AtomicU32 = AtomicU32::new(0);
static WATCH_THREAD: OnceLock<Mutex<Option<JoinHandle<()>>>> = OnceLock::new();

/// A foreground switch, as handed over by the callback. Plain data: no
/// handle is dereferenced here, the worker resolves everything.
#[derive(Debug, Clone, Copy)]
pub struct ForegroundEvent {
    /// Raw `HWND` value of the new foreground window (0 = none).
    pub hwnd_raw: isize,
    /// Owning thread id reported by the hook (0 = unknown).
    pub event_thread: u32,
}

// Thread-local sender: lock-free hand-over, same pattern as the LL hooks.
#[cfg(target_os = "windows")]
thread_local! {
    static LOCAL_TX: std::cell::RefCell<Option<mpsc::Sender<ForegroundEvent>>> =
        const { std::cell::RefCell::new(None) };
}

/// Install the patrol. Re-calling restarts cleanly.
pub fn start_foreground_watch() {
    #[cfg(not(target_os = "windows"))]
    {
        return;
    }
    #[cfg(target_os = "windows")]
    {
        stop_foreground_watch();
        STOP_FLAG.store(false, Ordering::Release);
        let thread = std::thread::Builder::new()
            .name("nanoclick-foreground-watch".into())
            .spawn(run_watch)
            .expect("foreground watch thread must spawn");
        *WATCH_THREAD.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(thread);
    }
}

/// Stop the patrol: latch the flag, wake the pump, join. Idempotent.
pub fn stop_foreground_watch() {
    #[cfg(not(target_os = "windows"))]
    {
        return;
    }
    #[cfg(target_os = "windows")]
    {
        STOP_FLAG.store(true, Ordering::Release);
        let tid = WATCH_THREAD_ID.load(Ordering::Acquire);
        if tid != 0 {
            unsafe {
                let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
        if let Some(thread) = WATCH_THREAD
            .get_or_init(|| Mutex::new(None))
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
        {
            let _ = thread.join();
        }
        WATCH_THREAD_ID.store(0, Ordering::Release);
    }
}

/// Pure predicate, unit-tested: is this event even eligible for the worker?
///
/// * `hwnd_raw == 0` — no foreground window (logoff, secure desktop
///   transition): fail-open, nothing to probe.
/// * `event_thread == own_tid` — our own window took focus: skip even with
///   `WINEVENT_SKIPOWNPROCESS` as a belt.
pub fn should_handle_foreground_event(ev: ForegroundEvent, own_tid: u32) -> bool {
    if ev.hwnd_raw == 0 {
        return false;
    }
    if own_tid != 0 && ev.event_thread == own_tid {
        return false;
    }
    true
}
#[cfg(target_os = "windows")]
fn run_watch() {
    let tid = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
    WATCH_THREAD_ID.store(tid, Ordering::Release);

    let (tx, rx) = mpsc::channel::<ForegroundEvent>();
    LOCAL_TX.with(|cell| *cell.borrow_mut() = Some(tx));

    // SAFETY: out-of-context hook with a process lifetime longer than the
    // hook's. `UnhookWinEvent` runs on this same thread before it exits.
    // Signature (windows 0.52): (eventmin, eventmax, hmod, proc, pid, tid,
    // flags) — hmod is `Option<HMODULE>`-shaped via IntoParam, so `None`.
    let hook: HWINEVENTHOOK = unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(winevent_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        )
    };
    if hook.0 == 0 {
        crate::debug_log_internal(
            "error",
            "[ForegroundWatch] SetWinEventHook failed; patrol not running",
        );
        LOCAL_TX.with(|cell| *cell.borrow_mut() = None);
        WATCH_THREAD_ID.store(0, Ordering::Release);
        return;
    }
    WATCH_RUNNING.store(true, Ordering::Release);
    crate::debug_log_internal(
        "info",
        "[ForegroundWatch] patrol installed (EVENT_SYSTEM_FOREGROUND)",
    );

    // The worker IS this thread: the hook requires its installing thread to
    // pump messages, and the channel drains between pumps. No second thread,
    // no cross-thread handle dance.
    let own_pid = unsafe { windows::Win32::System::Threading::GetCurrentProcessId() };
    loop {
        // Drain everything the callback handed over since the last pump.
        while let Ok(ev) = rx.try_recv() {
            if STOP_FLAG.load(Ordering::Acquire) {
                break;
            }
            handle_foreground_event(ev, own_pid, tid);
        }
        if STOP_FLAG.load(Ordering::Acquire) {
            break;
        }
        // Blocking pump: 0% CPU while the user works. Any posted message —
        // including our own WM_QUIT — wakes it.
        let mut msg = MSG::default();
        let got = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if !got.as_bool() {
            break; // WM_QUIT
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    unsafe {
        let _ = UnhookWinEvent(hook);
    }
    LOCAL_TX.with(|cell| *cell.borrow_mut() = None);
    WATCH_THREAD_ID.store(0, Ordering::Release);
    WATCH_RUNNING.store(false, Ordering::Release);
    crate::debug_log_internal("info", "[ForegroundWatch] patrol stopped");
}

/// Heavy work — runs on the watch thread, NEVER in the callback.
#[cfg(target_os = "windows")]
fn handle_foreground_event(ev: ForegroundEvent, own_pid: u32, own_tid: u32) {
    use std::sync::atomic::AtomicU64;

    if !should_handle_foreground_event(ev, own_tid) {
        return;
    }

    // THROTTLE FIRST, SYSCALLS SECOND — same rule as `uipi_guard_check`:
    // one relaxed atomic load, only then elevation work. Separate counter:
    // a foreground switch is rarer than a click, so it must not eat the
    // click path's throttle budget (and vice versa).
    static LAST_ALERT: AtomicU64 = AtomicU64::new(0);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    if now_ms.saturating_sub(LAST_ALERT.load(Ordering::Relaxed))
        < crate::platform::windows::uipi::UIPI_ALERT_THROTTLE_MS
    {
        return;
    }

    let hwnd = HWND(ev.hwnd_raw as _);
    if hwnd.0 == 0 {
        return;
    }
    // Skip our own windows by PID as well: `WINEVENT_SKIPOWNPROCESS` covers
    // the hook delivery, this covers a focus that landed while/after restore.
    let mut pid = 0u32;
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    if pid == 0 || pid == own_pid {
        return;
    }
    // Secure desktop / probe failure = fail-open: a lock screen must never
    // raise an admin alarm.
    if !crate::platform::windows::uipi::is_window_elevated(hwnd) {
        return;
    }
    // Only warn while automation can actually hit the wall: clicker running
    // or macro playing. An idle user Alt-Tabbing into Task Manager learns
    // nothing from a balloon.
    let automation_live = crate::platform::get_global_app_handle()
        .and_then(|app| app.try_state::<crate::AppState>())
        .map(|s| {
            s.scheduler.is_active()
                || crate::core::global()
                    .map(|e| e.is_running())
                    .unwrap_or(false)
        })
        .unwrap_or(false);
    if !automation_live {
        return;
    }

    LAST_ALERT.store(now_ms, Ordering::Relaxed);
    crate::platform::windows::uipi::play_warning_sound();
    if let Some(handle) = crate::platform::get_global_app_handle() {
        // The Z-ram decision needs the window state: take it through the
        // single gate (`main_window_visible`) and dispatch window work to the
        // Tauri main thread — this worker must never touch a WebView.
        let main = handle.clone();
        let _ = handle
            .clone()
            .run_on_main_thread(move || {
                crate::foreground_alert_on_elevated_focus(&main);
            });
    }
    crate::notifications::push_notification(
        crate::notifications::NotificationCode::UipiBlocked,
        "NanoClick — Administrator Rights Required",
        "Focused window runs elevated (UIPI): clicks will be blocked",
        None,
        true,
    );
}
/// Dropped-event counter: the callback has no fallback for a lost switch
/// (unlike keys, there is no `GetAsyncKeyState` re-check for focus), so a
/// failed hand-over is counted here and reported by the worker, never logged
/// from inside the callback.
#[cfg(target_os = "windows")]
static WINEVENT_TX_FAILED: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// The system callback. Lock-free hand-over ONLY — see the module docs.
/// A failed `send` is counted, never logged, never retried here.
#[cfg(target_os = "windows")]
unsafe extern "system" fn winevent_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    _idobject: i32,
    _idchild: i32,
    event_thread: u32,
    _event_time: u32,
) {
    if event != EVENT_SYSTEM_FOREGROUND {
        return;
    }
    if STOP_FLAG.load(Ordering::Acquire) {
        return;
    }
    let ev = ForegroundEvent {
        hwnd_raw: hwnd.0 as isize,
        event_thread,
    };
    let delivered = LOCAL_TX.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|tx| tx.send(ev).is_ok())
            .unwrap_or(false)
    });
    if !delivered {
        WINEVENT_TX_FAILED.fetch_add(1, Ordering::Relaxed);
    }
}

/// Lifetime total of foreground switches the callback could not hand over.
/// Reported by the worker thread (which may log), never by the callback.
#[cfg(target_os = "windows")]
pub fn foreground_events_undelivered_total() -> u64 {
    WINEVENT_TX_FAILED.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_hwnd_is_fail_open() {
        let ev = ForegroundEvent { hwnd_raw: 0, event_thread: 1234 };
        assert!(!should_handle_foreground_event(ev, 9999));
    }

    #[test]
    fn own_thread_event_is_skipped() {
        let ev = ForegroundEvent { hwnd_raw: 0x1234, event_thread: 777 };
        assert!(!should_handle_foreground_event(ev, 777));
    }

    #[test]
    fn foreign_window_event_is_eligible() {
        let ev = ForegroundEvent { hwnd_raw: 0x1234, event_thread: 777 };
        assert!(should_handle_foreground_event(ev, 888));
    }

    #[test]
    fn unknown_own_tid_does_not_skip() {
        // Boot edge: we do not know our tid yet — never drop an event for it.
        let ev = ForegroundEvent { hwnd_raw: 0x1234, event_thread: 0 };
        assert!(should_handle_foreground_event(ev, 0));
    }
}
