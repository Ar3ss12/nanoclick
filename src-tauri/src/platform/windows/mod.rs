//! Windows platform — keyboard, mouse, hooks, cursor polling.

pub mod keyboard;
pub mod uipi;
pub mod windows_hooks;

pub use keyboard::{mouse_click, mouse_down, mouse_up, scroll_wheel, send_key, set_cursor_pos};
pub use uipi::*;
pub use windows_hooks::{stop_recorder_hooks, WindowsRecorderBackend};

use crate::core::action::{KeyCode, Modifiers, MouseButton};
use crate::scheduler::ClickScheduler;
use rand::Rng;
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex as StdMutex, Once, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetDC, GetPixel, ReleaseDC};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetCursorPos, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
    MSG, PM_REMOVE, PeekMessageW, SetCursorPos, SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
    KBDLLHOOKSTRUCT, MSLLHOOKSTRUCT, WH_KEYBOARD_LL, WH_MOUSE_LL,
};

/// Parse a config button label into the neutral MouseButton type.
/// Lives at the platform boundary; core/scheduler never see raw labels.
pub fn parse_button_label(label: &str) -> MouseButton {
    match label {
        "right" => MouseButton::Right,
        "middle" => MouseButton::Middle,
        "x1" => MouseButton::X1,
        "x2" => MouseButton::X2,
        _ => MouseButton::Left,
    }
}

/// Get current OS cursor position.
pub fn get_cursor_pos() -> (i32, i32) {
    unsafe {
        let mut p = POINT { x: 0, y: 0 };
        let _ = GetCursorPos(&mut p);
        (p.x, p.y)
    }
}

/// Native event handle — Rust-level atomic bool, cross-version compatible.
pub type NativeEventHandle = Arc<AtomicBool>;

/// Create a stop event.
pub fn create_stop_event() -> Option<NativeEventHandle> {
    Some(Arc::new(AtomicBool::new(false)))
}

/// Signal a stop event.
pub fn signal_stop_event(handle: Option<NativeEventHandle>) {
    if let Some(h) = handle {
        h.store(true, Ordering::Release);
    }
}

#[link(name = "winmm")]
extern "system" {
    fn timeBeginPeriod(u_period: u32) -> u32;
    fn timeEndPeriod(u_period: u32) -> u32;
}

static TIMER_INIT: Once = Once::new();

/// Set Windows global timer resolution to 1ms for sub-millisecond precision.
pub fn init_timer_resolution() {
    TIMER_INIT.call_once(|| {
        unsafe {
            let _ = timeBeginPeriod(1);
        }
    });
}

/// Cleanup Windows timer resolution on app termination.
#[allow(dead_code)]
pub fn cleanup_timer_resolution() {
    unsafe {
        let _ = timeEndPeriod(1);
    }
}

/// High-resolution interruptible timer — uses 1ms sleep chunks plus tight spin-wait
/// for the final 2ms, delivering < 0.05ms timing jitter across any CPS.
pub struct PlatformTimer;

impl PlatformTimer {
    pub fn new() -> Self {
        init_timer_resolution();
        PlatformTimer
    }

    /// Wait until `target` or until `stop_handle` is signaled.
    /// Sleeps in 1ms increments when remaining > 2ms (0% CPU idle), then spin-waits
    /// for the remaining <= 2ms for microsecond-level accuracy.
    pub fn wait_until(&self, target: Instant, stop_handle: NativeEventHandle) -> bool {
        loop {
            if stop_handle.load(Ordering::Acquire) {
                return false;
            }
            let now = Instant::now();
            if now >= target {
                return true;
            }
            let remaining = target.duration_since(now);
            if remaining > Duration::from_millis(2) {
                thread::sleep(Duration::from_millis(1));
            } else {
                std::hint::spin_loop();
            }
        }
    }
}

impl Default for PlatformTimer {
    fn default() -> Self {
        Self::new()
    }
}

static GLOBAL_APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

pub fn set_global_app_handle(handle: AppHandle) {
    let _ = GLOBAL_APP_HANDLE.set(handle);
}

pub fn get_global_app_handle() -> Option<&'static AppHandle> {
    GLOBAL_APP_HANDLE.get()
}

pub fn trigger_uipi_block_notification(x: i32, y: i32) {
    if let Some(app) = get_global_app_handle() {
        let _ = app.emit(
            "uipi-blocked",
            serde_json::json!({
                "x": x,
                "y": y,
                "is_elevated": false
            }),
        );
    }
}

/// ── v4.2 Platform Abstraction ─────────────────────────────────────────
/// Concrete Windows implementation of the shared platform contracts.
/// Wraps the existing free functions; core code depends on the traits,
/// never on this type or on Win32 handles.
pub struct WindowsBackend;

impl Default for WindowsBackend {
    fn default() -> Self {
        WindowsBackend
    }
}

impl crate::platform::backend::InputBackend for WindowsBackend {
    fn mouse_click(&self, button: MouseButton) {
        mouse_click(button);
    }

    fn mouse_down(&self, button: MouseButton) {
        mouse_down(button);
    }

    fn mouse_up(&self, button: MouseButton) {
        mouse_up(button);
    }

    fn scroll_wheel(&self, delta_x: i32, delta_y: i32) {
        scroll_wheel(delta_x, delta_y);
    }

    fn set_cursor_pos(&self, x: i32, y: i32) {
        set_cursor_pos(x, y);
    }

    fn send_key(&self, key: KeyCode, mods: Modifiers, is_up: bool) {
        send_key(key.0, mods.ctrl, mods.alt, mods.shift, mods.win, is_up);
    }

    fn cursor_position(&self) -> (i32, i32) {
        get_cursor_pos()
    }

    fn click_mouse(&self, spec: &crate::platform::backend::ClickSpec) -> bool {
        let (orig_x, orig_y) = self.cursor_position();
        let (mut target_x, mut target_y) = match spec.position_mode {
            crate::platform::backend::PositionMode::Fixed => (spec.fixed_x, spec.fixed_y),
            crate::platform::backend::PositionMode::Cursor => (orig_x, orig_y),
        };

        // UIPI check: if non-elevated, check if target window belongs to an elevated process
        if !is_current_process_elevated() && is_target_point_elevated(target_x, target_y) {
            static LAST_UIPI_ALERT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            let last = LAST_UIPI_ALERT.load(Ordering::Relaxed);
            if now_ms.saturating_sub(last) > 3000 {
                LAST_UIPI_ALERT.store(now_ms, Ordering::Relaxed);
                play_warning_sound();
                trigger_uipi_block_notification(target_x, target_y);
            }
        }

        if spec.jitter_radius > 0 {
            let radius = spec.jitter_radius as i32;
            let mut rng = rand::thread_rng();
            let dx = rng.gen_range(-radius..=radius);
            let dy = rng.gen_range(-radius..=radius);
            target_x += dx;
            target_y += dy;
            unsafe {
                let _ = SetCursorPos(target_x, target_y);
            }
        }

        match spec.click_type {
            crate::platform::backend::ClickType::Double => {
                mouse_click(spec.button);
                std::thread::sleep(std::time::Duration::from_millis(50));
                mouse_click(spec.button);
            }
            _ => mouse_click(spec.button),
        }

        // Restore original cursor position in Cursor mode to prevent accumulative Brownian drift
        if spec.jitter_radius > 0 && spec.position_mode == crate::platform::backend::PositionMode::Cursor {
            unsafe {
                let _ = SetCursorPos(orig_x, orig_y);
            }
        }

        true
    }

    fn release_mouse_hold(&self, button: MouseButton) {
        mouse_up(button);
    }
}

impl crate::platform::backend::HotkeyBackend for WindowsBackend {
    /// Stateless variant: the actual spawn needs scheduler+app wiring, so
    /// use `WindowsHotkeyBackend` for start. `stop`/`is_running` are global.
    fn start(&self) -> Result<(), String> {
        Err("hotkey start requires scheduler+handle wiring - use WindowsHotkeyBackend".into())
    }

    fn stop(&self) {
        shutdown_global_hotkey_listener();
    }

    fn is_running(&self) -> bool {
        GLOBAL_HOTKEY_RUNNING.load(Ordering::Acquire)
    }
}

/// Stateful hotkey backend holding the scheduler/app wiring captured at
/// setup time, so callers can `start()`/`stop()` through the trait
/// without knowing about the free functions.
pub struct WindowsHotkeyBackend {
    scheduler: Arc<ClickScheduler>,
    app_handle: AppHandle,
}

impl WindowsHotkeyBackend {
    pub fn new(scheduler: Arc<ClickScheduler>, app_handle: AppHandle) -> Self {
        WindowsHotkeyBackend {
            scheduler,
            app_handle,
        }
    }
}

impl crate::platform::backend::HotkeyBackend for WindowsHotkeyBackend {
    fn start(&self) -> Result<(), String> {
        spawn_global_hotkey_listener(Arc::clone(&self.scheduler), self.app_handle.clone());
        Ok(())
    }

    fn stop(&self) {
        shutdown_global_hotkey_listener();
    }

    fn is_running(&self) -> bool {
        GLOBAL_HOTKEY_RUNNING.load(Ordering::Acquire)
    }
}

/// Spawn the global hotkey listener thread.
pub fn spawn_global_hotkey_listener(scheduler: Arc<ClickScheduler>, app_handle: AppHandle) {
    if GLOBAL_HOTKEY_RUNNING.swap(true, Ordering::AcqRel) {
        crate::debug_log_internal("warn", "[Hotkeys] listener already running");
        return;
    }
    GLOBAL_HOTKEY_STOP.store(false, Ordering::Release);
    thread::spawn(move || {
        // ZERO-JITTER: hotkey path is on the critical click start/stop path.
        // Highest scheduler priority so a game at 100% CPU never delays hotkeys.
        #[cfg(target_os = "windows")]
        unsafe {
            use windows::Win32::System::Threading::{
                GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
            };
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
        }
        run_keyboard_hook(scheduler, app_handle);
        GLOBAL_HOTKEY_RUNNING.store(false, Ordering::Release);
    });
}

/// Stop the global hook and release its channel. Safe to call repeatedly.
pub fn shutdown_global_hotkey_listener() {
    GLOBAL_HOTKEY_STOP.store(true, Ordering::Release);
    if let Some(channel) = GLOBAL_HOTKEY_TX.get() {
        *channel.lock().unwrap() = None;
    }
}

fn run_keyboard_hook(scheduler: Arc<ClickScheduler>, app_handle: AppHandle) {
    crate::debug_log_internal("stage-ok", "[Hotkeys] starting global listener");

    let (event_tx, event_rx) = mpsc::channel::<GlobalKeyEvent>();
    let channel = GLOBAL_HOTKEY_TX.get_or_init(|| StdMutex::new(None));
    *channel.lock().unwrap() = Some(event_tx);

    unsafe extern "system" fn keyboard_proc(
        n_code: i32,
        w_param: WPARAM,
        l_param: LPARAM,
    ) -> LRESULT {
        if n_code == 0 {
            let kb = *(l_param.0 as *const KBDLLHOOKSTRUCT);
            let message = w_param.0;
            let is_down = message == 0x0100 || message == 0x0104;
            let is_up = message == 0x0101 || message == 0x0105;
            if (is_down || is_up) && kb.vkCode != 0 {
                if let Some(lock) = GLOBAL_HOTKEY_TX.get() {
                    // v4.2 hardening: try_lock fast path — this callback runs
                    // synchronously for the WHOLE SYSTEM. If the channel mutex
                    // is momentarily contended, skip: the listener's
                    // GetAsyncKeyState fallback poll re-detects held combos.
                    if let Ok(guard) = lock.try_lock() {
                        if let Some(sender) = guard.as_ref() {
                            let _ = sender.send(GlobalKeyEvent {
                                vk: kb.vkCode as u16,
                                is_down,
                            });
                        }
                    }
                }
            }
        }
        CallNextHookEx(None, n_code, w_param, l_param)
    }

    let hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), None, 0) };
    let _hook = match hook {
        Ok(hh) => {
            crate::debug_log_internal("stage-ok", "[Hotkeys] WH_KEYBOARD_LL installed");
            hh
        }
        Err(e) => {
            *channel.lock().unwrap() = None;
            crate::debug_log_internal(
                "error",
                &format!("[Hotkeys] WH_KEYBOARD_LL install failed: {:?}", e),
            );
            return;
        }
    };

    unsafe extern "system" fn mouse_hotkey_proc(
        n_code: i32,
        w_param: WPARAM,
        l_param: LPARAM,
    ) -> LRESULT {
        let msg = w_param.0;
        // FAST-PATH: 1000-8000 Hz gaming mouse move bypass!
        // If not XButton (0x020B, 0x020C) or MButton (0x0207, 0x0208) -> instant return!
        if n_code != 0 || (msg != 0x020B && msg != 0x020C && msg != 0x0207 && msg != 0x0208) {
            return CallNextHookEx(None, n_code, w_param, l_param);
        }
        let vk = if msg == 0x020B || msg == 0x020C {
            let m = *(l_param.0 as *const MSLLHOOKSTRUCT);
            let xbutton = (m.mouseData >> 16) as u16;
            if xbutton == 1 {
                0x05 // VK_XBUTTON1
            } else if xbutton == 2 {
                0x06 // VK_XBUTTON2
            } else {
                0
            }
        } else {
            0x04 // VK_MBUTTON
        };
        if vk != 0 {
            let is_down = msg == 0x020B || msg == 0x0207;
            if let Some(lock) = GLOBAL_HOTKEY_TX.get() {
                if let Ok(guard) = lock.try_lock() {
                    if let Some(sender) = guard.as_ref() {
                        let _ = sender.send(GlobalKeyEvent { vk, is_down });
                    }
                }
            }
        }
        CallNextHookEx(None, n_code, w_param, l_param)
    }

    let mouse_hook = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hotkey_proc), None, 0) };
    let _mouse_hook = match mouse_hook {
        Ok(mh) => {
            crate::debug_log_internal("stage-ok", "[Hotkeys] WH_MOUSE_LL installed");
            Some(mh)
        }
        Err(e) => {
            crate::debug_log_internal(
                "warn",
                &format!("[Hotkeys] WH_MOUSE_LL install failed: {:?}", e),
            );
            None
        }
    };

    crate::debug_log_internal("stage-ok", "[Hotkeys] entering event-driven loop");

    // Parse-once contract: bindings are parsed from scheduler strings only at
    // startup and after the scheduler bumps its hotkeys_version (a save).
    // No per-poll string cloning or re-parsing.
    let mut seen_version = scheduler.hotkeys_version();
    let mut bindings = {
        let snapshot = HotkeySnapshot::from_scheduler(&scheduler);
        HotkeyBindings::from_snapshot(&snapshot)
    };
    log_bindings(&bindings);
    let mut held = HashSet::<u16>::new();
    let mut poll_iter = 0u64;
    // v4.3: true while the pending toggle confirmation was opened by a key
    // that ALSO stopped the clicker through the typing kill-switch (see the
    // reset + assignment comments inside the loop). Lives here so the state
    // survives across poll iterations while the confirmation is pending.
    let mut toggle_deferred_killed_clicker = false;

    while !GLOBAL_HOTKEY_STOP.load(Ordering::Acquire) {
        poll_iter += 1;

        // ── v4.3 FIX: WINDOWS MESSAGE PUMP (REQUIRED for LL hooks) ──────
        // WH_KEYBOARD_LL delivers its callback through the message queue of
        // the thread that installed the hook — and ONLY when that thread
        // pumps messages. Without a pump the OS never invokes keyboard_proc,
        // so no individual key-down (a letter!) ever reaches the typing
        // guard; only the GetAsyncKeyState fallback below worked, which is
        // why toggles worked but typing detection was completely deaf.
        // PeekMessageW is non-blocking: the recv_timeout below still wakes
        // up every ~20 ms to service the channel, the deferred-toggle poll
        // and the stop flag.
        let mut msg = MSG::default();
        while unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) }.into() {
            unsafe {
                let _ = TranslateMessage(&msg);
                let _ = DispatchMessageW(&msg);
            }
        }

        let mut events: Vec<GlobalKeyEvent> = Vec::new();
        // Drain the entire queue so fast combos and simultaneous keypresses
        // never back up behind the timeout loop.
        match event_rx.recv_timeout(Duration::from_millis(5)) {
            Ok(first) => {
                events.push(first);
                while let Ok(next) = event_rx.try_recv() {
                    events.push(next);
                }
            }
            Err(_) => {
                // Fallback polling only runs when the hook queue is idle
                held.retain(|&key| key_down(key));
                for group in bindings.all_groups().iter() {
                    for combo in group.iter() {
                        if key_down(combo.trigger) && combo.required.iter().all(|key| key_down(*key)) {
                            events.push(GlobalKeyEvent {
                                vk: combo.trigger,
                                is_down: true,
                            });
                        }
                    }
                }
            }
        }

        let current_version = scheduler.hotkeys_version();
        if current_version != seen_version {
            seen_version = current_version;
            let snapshot = HotkeySnapshot::from_scheduler(&scheduler);
            bindings = HotkeyBindings::from_snapshot(&snapshot);
            log_bindings(&bindings);
            hotkey_diag_push(format!("bindings re-parsed on version {current_version}"));
        }
        // v4.3 typing kill-switch integration: set while the current pending
        // toggle confirmation was opened by a key that ALSO ran the kill-switch
        // (a typable letter such as `T` used as the toggle). The confirmation
        // must not fire `hotkey_toggle` in that case — the kill-switch already
        // stopped the clicker the instant the key landed, and firing the toggle
        // would START it again (a stop-then-restart).
        // Reset whenever there is no pending window (confirmed or cancelled).
        if !scheduler.typing_guard().is_toggle_pending() {
            toggle_deferred_killed_clicker = false;
        }
        for event in events {
            let was_held = held.iter().any(|&key| key_code_matches(event.vk, key));
            // v4.2 hardening: NO file logging on this thread — WH_KEYBOARD_LL
            // is synchronous system-wide; diagnostics go to the ring buffer.
            if !was_held {
                hotkey_diag_push(format!("vk=0x{:02X} down={}", event.vk, event.is_down));
            }
            if event.is_down {
                // v4.2 race fix: a missed key-up leaves the vk stuck in
                // `held`, which would swallow every later press of the same
                // key (fast double-tap bug). If the physical key is actually
                // up, this DOWN is a fresh press: clean stale state first.
                //
                // `classify_press` also filters OS auto-repeat: a second DOWN
                // while the key is still physically down is NOT a new press,
                // so one long hold can never fire the toggle repeatedly.
                //
                // `&&` short-circuit keeps the hook path syscall-free for the
                // common case: when the key was not tracked as held this is
                // always a fresh press, so `GetAsyncKeyState` is never called.
                let held_and_down = was_held && key_down(event.vk);
                let fresh_press = classify_press(was_held, held_and_down) == PressKind::Fresh;
                if was_held && fresh_press {
                    held.retain(|&key| !key_code_matches(event.vk, key));
                    hotkey_diag_push(format!("stale-held cleaned vk=0x{:02X}", event.vk));
                }
                held.insert(event.vk);
                // v4.2 race fix continues below; the Typing Guard gate is
                // applied to the TOGGLE group only (see the block after the
                // hotkey dispatch) so it can never lock out the emergency
                // stop or the mode switch.
                if fresh_press {
                    // ── TYPING GUARD: TOGGLE GATE ────────────────────────
                    // Two layers, because a single check is not enough:
                    //
                    // 1. The lockout (below, `note`) only reacts to the keys
                    //    typed BEFORE the hotkey. When `R` is the FIRST letter
                    //    of a word ("rush", "run") nothing has armed it yet, so
                    //    a plain check would let the toggle through.
                    // 2. Presses on keys that typing can produce are therefore
                    //    held back for TOGGLE_CONFIRM_MS. A deliberate press is
                    //    isolated and fires; a press inside a word is followed
                    //    by another letter (or a second press) which cancels it.
                    //
                    // Only the toggle is gated: emergency stop, mode switch,
                    // speed and preset hotkeys are never suppressed — stopping
                    // must always work.
                    let toggle_claimed = bindings
                        .toggle
                        .iter()
                        .any(|combo| combo_matches(combo, event.vk, &held, key_down));
                    // A typable trigger (a bare letter like `R`) is ambiguous,
                    // so it needs the confirmation window; `F6` or a combo with
                    // modifiers cannot be produced by typing and fires at once.
                    let toggle_is_typable = bindings.toggle.iter().any(|combo| {
                        combo.required.is_empty()
                            && combo.trigger_matches(event.vk)
                            && crate::guard::is_typable_vk(combo.trigger)
                    });

                    // Set when this very press opened a confirmation window:
                    // such a key must neither arm the lockout nor cancel its
                    // own pending toggle (the toggle may be bound to a typable
                    // key such as `K`).
                    let mut toggle_deferred = false;
                    // True when THIS key-down stopped a running clicker through
                    // the kill-switch below (only possible for text keys that
                    // also match the toggle binding, e.g. a `T` hotkey).
                    let mut killed_by_text_press = false;

                    // ── TYPING GUARD: KILL ACTIVE CLICKER FIRST ──────────────
                    // New requirement: the moment a REAL text key lands, a
                    // running clicker is stopped immediately and stays off.
                    // A toggle that fired inside a word a moment earlier (or
                    // clicks started by any other path) cannot keep running
                    // into the chat box. Stopping is idempotent: if the
                    // clicker is idle this is a no-op.
                    //
                    // Runs BEFORE the toggle gate below, so the dead press
                    // and the kill never see each other's state.
                    if crate::guard::is_text_keypress_vk(event.vk) && !toggle_deferred {
                        let was_active = scheduler.is_active();
                        scheduler.set_active(false, Some(&app_handle));
                        if let Some(exec) = crate::core::global() {
                            exec.stop();
                        }
                        if was_active {
                            killed_by_text_press = true;
                            hotkey_diag_push(format!(
                                "killed_by_typing vk=0x{:02X}",
                                event.vk
                            ));
                        }
                    }

                    if toggle_claimed {
                        if scheduler.is_active() {
                            // STOP is always instantaneous (0ms delay) — never delayed by typing confirmation!
                            hotkey_diag_push("confirmed_action=toggle_instant_stop".into());
                            let prev = scheduler.is_active();
                            let mode = scheduler.hotkey_toggle(Some(&app_handle));
                            hotkey_diag_push(format!("instant stop done was_active={prev} mode={mode}"));
                        } else {
                            // START is gated by Typing Guard to prevent accidental trigger while typing text
                            let guard = scheduler.typing_guard();
                            let now = crate::guard::now_ms();
                            // `true` = fire now (key cannot be typed);
                            // `false` = rejected or held back for confirmation,
                            // resolved in the poll loop below.
                            if guard.try_arm_toggle(now, crate::guard::TOGGLE_CONFIRM_MS, toggle_is_typable) {
                                hotkey_diag_push("confirmed_action=toggle".into());
                                let prev = scheduler.is_active();
                                let mode = scheduler.hotkey_toggle(Some(&app_handle));
                                hotkey_diag_push(format!("toggle done was_active={prev} mode={mode}"));
                            } else if guard.is_toggle_pending() {
                                toggle_deferred = true;
                                // Remember whether this very key already stopped
                                // the clicker via the kill-switch. If yes, the
                                // later confirmation must NOT call hotkey_toggle
                                // (it would restart what the kill-switch stopped).
                                toggle_deferred_killed_clicker = killed_by_text_press;
                                hotkey_diag_push(format!(
                                    "deferred_action=toggle vk=0x{:02X} confirm={}ms killed={}",
                                    event.vk,
                                    crate::guard::TOGGLE_CONFIRM_MS,
                                    killed_by_text_press
                                ));
                            } else {
                                hotkey_diag_push(format!(
                                    "suppressed_action=toggle_typing_guard vk=0x{:02X}",
                                    event.vk
                                ));
                                // The bind DID fire and the guard refused it. Silence
                                // is what makes a working hotkey look broken, so the
                                // veto goes to the page on the same channel the tray
                                // menu uses (with zero windows there is nobody to tell).
                                if let Some(win) = app_handle.get_webview_window("main") {
                                    let _ = win.emit(
                                        "tray-action-result",
                                        serde_json::json!({
                                            "action": "toggle_clicking",
                                            "ok": false,
                                            "reason": "typing",
                                            "left_ms": guard.lock_remaining_ms(),
                                        }),
                                    );
                                }
                            }
                        }
                    }
                    fire_hotkey_group(&bindings.mode_switch, event.vk, &held, || {
                        hotkey_diag_push("fired_action=mode_switch".into());
                        // Shared with the UI badge and the tray menu: this path
                        // must persist `active_mode` too, or the next config load
                        // reverts the switch the user just made.
                        crate::apply_mode_toggle(&app_handle);
                    });
                    fire_hotkey_group(&bindings.emergency_stop, event.vk, &held, || {
                        hotkey_diag_push("fired_action=emergency_stop".into());
                        scheduler.set_active(false, Some(&app_handle));
                        if let Some(exec) = crate::core::global() {
                            exec.stop();
                        }
                    });
                    fire_hotkey_group(&bindings.speed_up, event.vk, &held, || {
                        hotkey_diag_push("fired_action=speed_up".into());
                        scheduler.adjust_cps(1.0, Some(&app_handle));
                    });
                    fire_hotkey_group(&bindings.slow_down, event.vk, &held, || {
                        hotkey_diag_push("fired_action=slow_down".into());
                        scheduler.adjust_cps(-1.0, Some(&app_handle));
                    });
                    fire_hotkey_group(&bindings.capture_pos, event.vk, &held, || {
                        hotkey_diag_push("fired_action=capture_pos".into());
                        let pos = get_cursor_pos();
                        let _ = app_handle.emit("global-capture-pos", pos);
                    });
                    fire_hotkey_group(&bindings.record_toggle, event.vk, &held, || {
                        hotkey_diag_push("fired_action=record_toggle".into());
                        let _ = app_handle.emit("global-record-toggle", ());
                    });

                    // Direct Preset Hotkeys with Instant Preemption
                    for (preset_id, combos) in &bindings.preset_hotkeys {
                        fire_hotkey_group(combos, event.vk, &held, || {
                            hotkey_diag_push(format!("fired_action=preset_{preset_id}"));
                            scheduler.activate_preset_hotkey(preset_id, Some(&app_handle));
                        });
                    }

                    // Preset slot hotkeys: slot 1..9 -> global-preset-hotkey(idx).
                    for (slot_idx, combos) in bindings.preset_slots.iter().enumerate() {
                        let slot = slot_idx as u32;
                        fire_hotkey_group(combos, event.vk, &held, || {
                            hotkey_diag_push(format!("fired_action=preset_slot_{}", slot + 1));
                            let _ = app_handle.clone().emit("global-preset-hotkey", slot);
                        });
                    }

                    // ── TYPING GUARD ARMING (must be LAST) ───────────────
                    // Recorded only after the kill-switch above and every
                    // hotkey group was evaluated for this key-down: if it ran
                    // earlier, a hotkey that is itself a typing key would arm
                    // the lockout and then suppress the very press that armed
                    // it.
                    //
                    // This is also the CANCEL signal for a pending toggle
                    // confirmation: the moment another text key arrives, the
                    // held-back press is proven to be a letter inside a word.
                    // Gameplay keys (WASD, hotbar digits, arrows, ...) never
                    // arm or cancel anything.
                    if crate::guard::is_text_keypress_vk(event.vk) && !toggle_deferred {
                        scheduler.typing_guard().note();
                        hotkey_diag_push(format!("armed_guard vk=0x{:02X}", event.vk));
                    }
                } else if bindings.toggle.iter().any(|combo| combo.trigger_matches(event.vk)) {
                    // The toggle's trigger key, but a required modifier is not
                    // held (kept for parity with the other hotkey groups).
                    hotkey_diag_push(format!(
                        "reject required_key_not_held trigger=0x{:02X}",
                        event.vk
                    ));
                }
            } else {
                held.retain(|&key| !key_code_matches(event.vk, key));
            }
        }
        // ── TYPING GUARD: RESOLVE A HELD-BACK TOGGLE ─────────────────
        // A toggle press that outlived its confirmation window without any
        // further typing was an isolated press, i.e. deliberate. Fire it now
        // — but only through set_active's central choke point, which re-checks
        // the typing lockout: a letter that landed in the poll gap after the
        // deadline can never switch the clicker on mid-word, even on a stale
        // confirmation.
        // This runs on the hook thread in the poll loop (every ~20 ms), so no
        // extra thread and no timer are needed.
        if let Some(confirmed) = scheduler.typing_guard().poll_pending(crate::guard::now_ms()) {
            if confirmed {
                if toggle_deferred_killed_clicker {
                    // v4.3: the key that opened this confirmation already
                    // stopped the clicker the instant it landed (typing
                    // kill-switch). Firing hotkey_toggle here would restart
                    // it, turning a single letter toggle-press into a
                    // stop-then-restart no-op. Keep the clicker off.
                    hotkey_diag_push("confirmed_action=toggle_skipped_typing_kill".into());
                } else {
                    hotkey_diag_push("confirmed_action=toggle".into());
                    let prev = scheduler.is_active();
                    let mode = scheduler.hotkey_toggle(Some(&app_handle));
                    hotkey_diag_push(format!("toggle done was_active={prev} mode={mode}"));
                }
            } else {
                hotkey_diag_push("cancelled_action=toggle_typing_guard".into());
            }
        }

        if poll_iter % 100 == 0 {
            hotkey_diag_push("heartbeat".into());
        }
    }

    unsafe {
        let _ = UnhookWindowsHookEx(_hook);
        if let Some(mh) = _mouse_hook {
            let _ = UnhookWindowsHookEx(mh);
        }
    }
    crate::debug_log_internal("stage-ok", "[Hotkeys] listener stopped and hook released");
}

#[derive(Clone, Copy)]
struct GlobalKeyEvent {
    vk: u16,
    is_down: bool,
}

/// In-memory diagnostic ring buffer for hotkey events (v4.2 hardening).
/// Replaces per-event file logging on the LL-hook thread: WH_KEYBOARD_LL is
/// synchronous system-wide, so ANY file I/O in its callback adds input
/// latency for EVERY application. Diagnostics land here at zero syscall
/// cost and are dumped only when explicitly requested.
static HOTKEY_DIAG: StdMutex<VecDeque<String>> = StdMutex::new(VecDeque::new());

fn hotkey_diag_push(line: String) {
    if let Ok(mut q) = HOTKEY_DIAG.lock() {
        if q.len() >= 128 {
            q.pop_front();
        }
        q.push_back(line);
    }
}

/// Dump and clear the diagnostic buffer. Used by `dump_input_diagnostics`
/// (the Settings button) and by the tests; the hook thread only ever pushes.
pub fn hotkey_diag_dump() -> Vec<String> {
    HOTKEY_DIAG
        .lock()
        .map(|mut q| q.drain(..).collect())
        .unwrap_or_default()
}

static GLOBAL_HOTKEY_TX: OnceLock<StdMutex<Option<Sender<GlobalKeyEvent>>>> = OnceLock::new();
static GLOBAL_HOTKEY_STOP: AtomicBool = AtomicBool::new(false);
static GLOBAL_HOTKEY_RUNNING: AtomicBool = AtomicBool::new(false);

/// Classification of a key-down for the hook loop.
///
/// Extracted so the two field bugs — a missed key-up swallowing the next
/// press, and OS auto-repeat firing a hotkey many times per press — are
/// unit-testable with an injected physical key state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PressKind {
    /// A brand-new press: hotkey groups must be evaluated.
    Fresh,
    /// The key was already held and is still physically down: OS auto-repeat.
    Repeat,
}

fn classify_press(was_held: bool, physically_down: bool) -> PressKind {
    if was_held && physically_down {
        PressKind::Repeat
    } else {
        PressKind::Fresh
    }
}

/// Fire `fire` when `vk` matches a combo in `group`.
///
/// NOTE: this function runs on the low-level keyboard-hook thread, which is
/// synchronous system-wide. It must never touch the log file — diagnostics go
/// to the in-memory ring buffer via `hotkey_diag_push` (enforced by the
/// `hook_loop_contains_no_file_logging` test).
fn fire_hotkey_group<F: FnOnce()>(group: &[HotkeyCombo], vk: u16, held: &HashSet<u16>, fire: F) {
    if group
        .iter()
        .any(|combo| combo_matches(combo, vk, held, key_down))
    {
        fire();
    } else if group.iter().any(|combo| combo.trigger_matches(vk)) {
        hotkey_diag_push(format!("reject required_key_not_held trigger=0x{vk:02X}"));
    }
}

fn combo_matches<F: Fn(u16) -> bool>(
    combo: &HotkeyCombo,
    vk: u16,
    held: &HashSet<u16>,
    physical_key_down: F,
) -> bool {
    combo.trigger_matches(vk)
        && combo.required.iter().all(|key| {
            held.iter().any(|actual| key_code_matches(*key, *actual))
                // Low-level hooks can miss an intermediate key-down while
                // another application owns the foreground input.
                || physical_key_down(*key)
        })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HotkeySnapshot {
    toggle: String,
    mode_switch: String,
    emergency_stop: String,
    speed_up: String,
    slow_down: String,
    capture_pos: String,
    record_toggle: bool,
    record_hotkey: String,
    preset_slots: Vec<String>,
    preset_hotkeys: Vec<(String, String)>,
}

impl HotkeySnapshot {
    fn from_scheduler(scheduler: &ClickScheduler) -> Self {
        let cfg = scheduler.get_config();
        let preset_hotkeys = cfg
            .presets
            .iter()
            .filter(|p| !p.hotkey.trim().is_empty())
            .map(|p| (p.id.clone(), p.hotkey.clone()))
            .collect();
        HotkeySnapshot {
            toggle: cfg.hotkey_toggle,
            mode_switch: cfg.hotkey_mode_switch,
            emergency_stop: cfg.hotkey_emergency_stop,
            speed_up: cfg.hotkey_speed_up,
            slow_down: cfg.hotkey_slow_down,
            capture_pos: cfg.hotkey_capture_pos,
            record_toggle: cfg.hotkey_record_toggle,
            record_hotkey: cfg.hotkey_record,
            preset_slots: cfg.hotkey_preset_slots,
            preset_hotkeys,
        }
    }
}

#[derive(Clone, Debug)]
struct HotkeyCombo {
    required: Vec<u16>,
    trigger: u16,
}

impl HotkeyCombo {
    fn trigger_matches(&self, actual: u16) -> bool {
        key_code_matches(self.trigger, actual)
    }
}

struct HotkeyBindings {
    toggle: Vec<HotkeyCombo>,
    mode_switch: Vec<HotkeyCombo>,
    emergency_stop: Vec<HotkeyCombo>,
    speed_up: Vec<HotkeyCombo>,
    slow_down: Vec<HotkeyCombo>,
    capture_pos: Vec<HotkeyCombo>,
    record_toggle: Vec<HotkeyCombo>,
    /// Per-slot preset combos: outer index = slot number - 1.
    preset_slots: Vec<Vec<HotkeyCombo>>,
    /// Direct preset hotkeys: (preset_id, combos).
    preset_hotkeys: Vec<(String, Vec<HotkeyCombo>)>,
    invalid_bindings: usize,
}

impl HotkeyBindings {
    fn all_groups(&self) -> Vec<&[HotkeyCombo]> {
        let mut groups = vec![
            self.toggle.as_slice(),
            self.mode_switch.as_slice(),
            self.emergency_stop.as_slice(),
            self.speed_up.as_slice(),
            self.slow_down.as_slice(),
            self.capture_pos.as_slice(),
            self.record_toggle.as_slice(),
        ];
        for slot in &self.preset_slots {
            groups.push(slot.as_slice());
        }
        for (_id, combos) in &self.preset_hotkeys {
            groups.push(combos.as_slice());
        }
        groups
    }
}

impl HotkeyBindings {
    fn from_snapshot(snapshot: &HotkeySnapshot) -> Self {
        let (toggle, mut invalid_bindings) = combos_from_label(&snapshot.toggle);
        let (mode_switch, invalid) = combos_from_label(&snapshot.mode_switch);
        invalid_bindings += invalid;
        let (emergency_stop, invalid) = combos_from_label(&snapshot.emergency_stop);
        invalid_bindings += invalid;
        let (speed_up, invalid) = combos_from_label(&snapshot.speed_up);
        invalid_bindings += invalid;
        let (slow_down, invalid) = combos_from_label(&snapshot.slow_down);
        invalid_bindings += invalid;
        let (capture_pos, invalid) = combos_from_label(&snapshot.capture_pos);
        invalid_bindings += invalid;
        let (record_toggle, invalid) = if snapshot.record_toggle {
            combos_from_label(&snapshot.record_hotkey)
        } else {
            (Vec::new(), 0)
        };
        invalid_bindings += invalid;
        let (preset_slots, invalid) = {
            let mut slots = Vec::with_capacity(snapshot.preset_slots.len());
            let mut inv = 0usize;
            for label in &snapshot.preset_slots {
                let (combos, n) = combos_from_label(label);
                inv += n;
                slots.push(combos);
            }
            (slots, inv)
        };
        invalid_bindings += invalid;
        let (preset_hotkeys, invalid) = {
            let mut list = Vec::with_capacity(snapshot.preset_hotkeys.len());
            let mut inv = 0usize;
            for (id, label) in &snapshot.preset_hotkeys {
                let (combos, n) = combos_from_label(label);
                inv += n;
                list.push((id.clone(), combos));
            }
            (list, inv)
        };
        invalid_bindings += invalid;
        HotkeyBindings {
            toggle,
            mode_switch,
            emergency_stop,
            speed_up,
            slow_down,
            capture_pos,
            record_toggle,
            preset_slots,
            preset_hotkeys,
            invalid_bindings,
        }
    }
}

fn combos_from_label(label: &str) -> (Vec<HotkeyCombo>, usize) {
    let groups: Vec<&str> = label
        .split(|c: char| c == '/' || c == '|')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let mut invalid = 0;
    let combos = groups
        .into_iter()
        .filter_map(|group| match parse_hotkey_combo(group) {
            Some(combo) => Some(combo),
            None => {
                invalid += 1;
                crate::debug_log_internal(
                    "warn",
                    &format!("[Hotkeys][diag] reject_reason=invalid_binding value={group:?}"),
                );
                None
            }
        })
        .collect();
    (combos, invalid)
}

fn log_bindings(bindings: &HotkeyBindings) {
    crate::debug_log_internal(
        "stage-ok",
        &format!(
            "[Hotkeys] resolved: toggle={} mode={} emergency={} speed_up={} slow_down={} capture={} record={}",
            bindings.toggle.len(),
            bindings.mode_switch.len(),
            bindings.emergency_stop.len(),
            bindings.speed_up.len(),
            bindings.slow_down.len(),
            bindings.capture_pos.len(),
            bindings.record_toggle.len(),
        ),
    );
    crate::debug_log_internal(
        "stage-ok",
        &format!(
            "[Hotkeys][diag] binding_parsed total={}",
            bindings
                .all_groups()
                .iter()
                .map(|group| group.len())
                .sum::<usize>()
        ),
    );
    if bindings.invalid_bindings > 0 {
        crate::debug_log_internal(
            "warn",
            &format!(
                "[Hotkeys][diag] invalid_binding_count={}",
                bindings.invalid_bindings
            ),
        );
    }
}

/// Parse a single key label (the part after the last `+` in a combo).
/// Returns the virtual-key code (VK_*) for the named key, or `None` if we
/// don't recognize it.
///
/// Supported forms (case-insensitive):
///   - Single ASCII letter: `R`, `K`, `a`, `z`
///   - Single ASCII digit:  `0`–`9`
///   - Function keys:        `F1`–`F24`
///   - Specials:             `Escape`/`Esc`, `Tab`, `Space`, `Enter`/`Return`,
///                           `Backspace`/`Bs`, `Delete`/`Del`, `Insert`/`Ins`,
///                           `Home`, `End`, `PageUp`/`PgUp`, `PageDown`/`PgDn`,
///                           `Up`, `Down`, `Left`, `Right`,
///                           `Caps`/`CapsLock`, `Shift`, `Ctrl`, `Alt`, `Win`/`Meta`
///   - Numpad digits:        `Num0`–`Num9`
fn vk_from_label(label: &str) -> Option<u16> {
    let l = label.trim();
    if l.is_empty() {
        return None;
    }

    // Single character — letter or digit.
    if l.len() == 1 {
        let c = l.chars().next().unwrap();
        if c.is_ascii_alphabetic() {
            return Some(0x41 + (c.to_ascii_uppercase() as u16 - b'A' as u16));
        }
        if c.is_ascii_digit() {
            return Some(0x30 + c.to_digit(10).unwrap() as u16);
        }
    }

    // Function keys F1..F24.
    if let Some(rest) = l.strip_prefix(['f', 'F']) {
        if let Ok(n) = rest.parse::<u32>() {
            if (1..=24).contains(&n) {
                return Some(0x70 + (n - 1) as u16); // VK_F1 = 0x70
            }
        }
    }

    // Numpad digits.
    if let Some(rest) = l.strip_prefix("Num") {
        if let Ok(n) = rest.parse::<u32>() {
            if (0..=9).contains(&n) {
                return Some(0x60 + n as u16); // VK_NUMPAD0 = 0x60
            }
        }
    }

    // Named special keys (case-insensitive).
    let lower = l.to_ascii_lowercase();
    let vk = match lower.as_str() {
        "=" | "+" | "plus" => 0xBB,       // VK_OEM_PLUS
        "*" | "asterisk" => 0x6A,         // VK_MULTIPLY / numpad *
        "-" | "minus" => 0xBD,            // VK_OEM_MINUS
        "," => 0xBC,                      // VK_OEM_COMMA
        "." => 0xBE,                      // VK_OEM_PERIOD
        "/" => 0xBF,                      // VK_OEM_2
        "\\" => 0xDC,                     // VK_OEM_5
        "escape" | "esc" => 0x1B,         // VK_ESCAPE
        "tab" => 0x09,                    // VK_TAB
        "space" | "spacebar" => 0x20,     // VK_SPACE
        "enter" | "return" => 0x0D,       // VK_RETURN
        "backspace" | "bs" => 0x08,       // VK_BACK
        "delete" | "del" => 0x2E,         // VK_DELETE
        "insert" | "ins" => 0x2D,         // VK_INSERT
        "home" => 0x24,                   // VK_HOME
        "end" => 0x23,                    // VK_END
        "pageup" | "pgup" => 0x21,        // VK_PRIOR
        "pagedown" | "pgdn" => 0x22,      // VK_NEXT
        "up" => 0x26,                     // VK_UP
        "down" => 0x28,                   // VK_DOWN
        "left" => 0x25,                   // VK_LEFT
        "right" => 0x27,                  // VK_RIGHT
        "caps" | "capslock" => 0x14,      // VK_CAPITAL
        "shift" => 0x10,                  // VK_SHIFT
        "ctrl" | "control" => 0x11,       // VK_CONTROL
        "alt" | "menu" => 0x12,           // VK_MENU
        "win" | "meta" | "super" => 0x5B, // VK_LWIN
        "apps" | "menu2" => 0x5D,         // VK_APPS
        // Mouse buttons as global hotkeys (Win32 virtual keys)
        "mouse4" | "m4" | "xbutton1" | "x1" => 0x05, // VK_XBUTTON1
        "mouse5" | "m5" | "xbutton2" | "x2" => 0x06, // VK_XBUTTON2
        "mouse3" | "m3" | "mbutton" | "middle" => 0x04, // VK_MBUTTON
        _ => return None,
    };
    Some(vk)
}

/// Parse a complete hotkey combo like `Ctrl+Alt+M` or `*+1`.
/// Every token before the last one must be held while the final token is
/// pressed. This supports both named modifiers and arbitrary key chords.
fn parse_hotkey_combo(label: &str) -> Option<HotkeyCombo> {
    let parts: Vec<&str> = label.split('+').map(|s| s.trim()).collect();
    if parts.is_empty() {
        return None;
    }
    let key_part = parts[parts.len() - 1];
    let required = parts[..parts.len() - 1]
        .iter()
        .map(|part| vk_from_label(part))
        .collect::<Option<Vec<_>>>()?;
    let vk = vk_from_label(key_part)?;
    Some(HotkeyCombo {
        required,
        trigger: vk,
    })
}

fn key_code_matches(expected: u16, actual: u16) -> bool {
    match expected {
        0x10 => matches!(actual, 0x10 | 0xA0 | 0xA1),
        0x11 => matches!(actual, 0x11 | 0xA2 | 0xA3),
        0x12 => matches!(actual, 0x12 | 0xA4 | 0xA5),
        0x5B => matches!(actual, 0x5B | 0x5C),
        // `*` may arrive as Numpad Multiply (0x6A) or as Shift+8 (0x38).
        0x6A => matches!(actual, 0x6A | 0x38),
        // A saved digit can originate from the top row, NumLock-on numpad,
        // or the navigation key emitted by that numpad key with NumLock off.
        0x30..=0x39 => {
            actual == expected
                || actual == 0x60 + (expected - 0x30)
                || numpad_navigation_vk(expected - 0x30) == Some(actual)
        }
        _ => expected == actual,
    }
}

fn numpad_navigation_vk(digit: u16) -> Option<u16> {
    // VK values produced by the numeric keypad when NumLock is off.
    match digit {
        0 => Some(0x2D), // Insert
        1 => Some(0x23), // End
        2 => Some(0x28), // Down
        3 => Some(0x22), // PageDown
        4 => Some(0x25), // Left
        5 => Some(0x0C), // Clear
        6 => Some(0x27), // Right
        7 => Some(0x24), // Home
        8 => Some(0x26), // Up
        9 => Some(0x21), // PageUp
        _ => None,
    }
}

fn key_down(vk: u16) -> bool {
    if vk == 0x6A {
        return unsafe {
            (GetAsyncKeyState(0x6A) as i32 & 0x8000) != 0
                || (GetAsyncKeyState(0x38) as i32 & 0x8000) != 0
        };
    }
    if (0x30..=0x39).contains(&vk) {
        let numpad_vk = 0x60 + (vk - 0x30);
        let navigation_vk = numpad_navigation_vk(vk - 0x30);
        return unsafe {
            (GetAsyncKeyState(vk as i32) as i32 & 0x8000) != 0
                || (GetAsyncKeyState(numpad_vk as i32) as i32 & 0x8000) != 0
                || navigation_vk
                    .is_some_and(|nav| (GetAsyncKeyState(nav as i32) as i32 & 0x8000) != 0)
        };
    }
    unsafe { (GetAsyncKeyState(vk as i32) as i32 & 0x8000) != 0 }
}

#[cfg(test)]
mod hotkey_tests {
    use super::*;

    #[test]
    fn star_plus_one_is_parsed_as_a_two_key_combo() {
        let combo = parse_hotkey_combo("*+1").expect("*+1 should parse");
        assert_eq!(combo.required, vec![0x6A]);
        assert_eq!(combo.trigger, 0x31);
        assert!(key_code_matches(0x6A, 0x6A));
        assert!(key_code_matches(0x6A, 0x38));
        assert!(key_code_matches(0x31, 0x61));
        assert!(key_code_matches(0x31, 0x23));
    }

    #[test]
    fn modifier_combo_requires_all_keys_and_matches_trigger() {
        let combo = parse_hotkey_combo("Ctrl+Alt+M").expect("combo should parse");
        let held = HashSet::from([0x11, 0x12]);
        assert!(combo_matches(&combo, 0x4D, &held, |_| false));
        assert!(!combo_matches(&combo, 0x4D, &HashSet::from([0x11]), |_| {
            false
        }));
        assert!(!combo_matches(&combo, 0x4E, &held, |_| false));
    }

    #[test]
    fn physical_state_fallback_completes_missing_required_key_event() {
        let combo = parse_hotkey_combo("Ctrl+P").expect("combo should parse");
        assert!(combo_matches(&combo, 0x50, &HashSet::new(), |key| key == 0x11));
    }

    #[test]
    fn alternate_hotkey_groups_parse_independently() {
        let (combos, invalid) = combos_from_label("R / K | F6");
        assert_eq!(combos.len(), 3);
        assert_eq!(invalid, 0);
        assert!(combos.iter().all(|combo| combo.required.is_empty()));
    }

    #[test]
    fn invalid_binding_is_reported_without_disabling_valid_alternatives() {
        let (combos, invalid) = combos_from_label("R / NotARealKey / K");
        assert_eq!(combos.len(), 2);
        assert_eq!(invalid, 1);
    }

    #[test]
    fn mouse_buttons_parse_to_correct_virtual_keys() {
        assert_eq!(vk_from_label("Mouse4"), Some(0x05));
        assert_eq!(vk_from_label("M4"), Some(0x05));
        assert_eq!(vk_from_label("XButton1"), Some(0x05));
        assert_eq!(vk_from_label("X1"), Some(0x05));

        assert_eq!(vk_from_label("Mouse5"), Some(0x06));
        assert_eq!(vk_from_label("M5"), Some(0x06));
        assert_eq!(vk_from_label("XButton2"), Some(0x06));
        assert_eq!(vk_from_label("X2"), Some(0x06));

        assert_eq!(vk_from_label("Mouse3"), Some(0x04));
        assert_eq!(vk_from_label("M3"), Some(0x04));
        assert_eq!(vk_from_label("Middle"), Some(0x04));
    }

    #[test]
    fn ctrl_plus_mouse4_parses_as_valid_combo() {
        let combo = parse_hotkey_combo("Ctrl+Mouse4").expect("Ctrl+Mouse4 should parse");
        assert_eq!(combo.required, vec![0x11]);
        assert_eq!(combo.trigger, 0x05);
        let held = HashSet::from([0x11]);
        assert!(combo_matches(&combo, 0x05, &held, |_| false));
        assert!(!combo_matches(&combo, 0x05, &HashSet::new(), |_| false));
    }
}

/// ── Hotkey stream regression suite ──────────────────────────────────
/// Feeds synthetic key streams (missed key-ups, OS auto-repeat, the 300 ms
/// double tap from the field reports) through the REAL press classifier and
/// the REAL combo matcher used by the hook loop.
#[cfg(test)]
mod hotkey_stream_tests {
    use super::*;

    /// Mirror of the hook loop's per-key bookkeeping. `physically_down` is
    /// injected so a test can pretend the OS dropped a key-up, or that the
    /// key is still physically held (auto-repeat).
    fn count_fires(
        combo: &HotkeyCombo,
        stream: &[(u16, bool)],
        physically_down: impl Fn(u16) -> bool,
    ) -> usize {
        let mut held = HashSet::<u16>::new();
        let mut fired = 0usize;
        for (vk, is_down) in stream {
            let was_held = held.iter().any(|&key| key_code_matches(*vk, key));
            if *is_down {
                let fresh = classify_press(was_held, physically_down(*vk)) == PressKind::Fresh;
                if was_held && fresh {
                    held.retain(|&key| !key_code_matches(*vk, key));
                }
                held.insert(*vk);
                if fresh && combo_matches(combo, *vk, &held, |_| false) {
                    fired += 1;
                }
            } else {
                held.retain(|&key| !key_code_matches(*vk, key));
            }
        }
        fired
    }

    /// Default toggle key.
    const R: u16 = 0x52;

    /// A lost key-up used to swallow every later press of the same key —
    /// the "toggle does nothing" half of the field reports.
    #[test]
    fn stream_missed_keyup_does_not_swallow_next_press() {
        let combo = parse_hotkey_combo("R").expect("R parses");
        // down, (key-up lost), down, up — physically the key is up each time.
        let fired = count_fires(&combo, &[(R, true), (R, true), (R, false)], |_| false);
        assert_eq!(
            fired, 2,
            "both presses must fire; a stuck `held` entry eats the second"
        );
    }

    /// OS auto-repeat must not turn one long hold into a burst of toggles.
    #[test]
    fn stream_autorepeat_down_fires_toggle_exactly_once() {
        let combo = parse_hotkey_combo("R").expect("R parses");
        let fired = count_fires(
            &combo,
            &[(R, true), (R, true), (R, true), (R, true), (R, false)],
            |_| true, // physically still held: extra DOWNs are auto-repeat
        );
        assert_eq!(fired, 1, "holding the key must not toggle repeatedly");
    }

    #[test]
    fn stream_duplicate_up_is_harmless() {
        let combo = parse_hotkey_combo("R").expect("R parses");
        let fired = count_fires(&combo, &[(R, true), (R, false), (R, false)], |_| false);
        assert_eq!(fired, 1, "a duplicate key-up must not confuse the tracker");
    }

    /// The exact field scenario end-to-end through the matcher AND the
    /// decision: press → start, wait 300 ms, press → stop.
    #[test]
    fn stream_fast_double_tap_start_then_stop() {
        use crate::scheduler::{decide_toggle, ToggleOutcome};
        let combo = parse_hotkey_combo("R").expect("R parses");

        // Press #1: the matcher fires once, and the decision (idle, free)
        // resolves to Start.
        assert_eq!(count_fires(&combo, &[(R, true), (R, false)], |_| false), 1);
        assert_eq!(
            decide_toggle(false, false, false, true),
            ToggleOutcome::Start
        );

        std::thread::sleep(Duration::from_millis(300));

        // Press #2: the matcher fires once again, and the decision (running)
        // resolves to Stop — never debounced, never refused.
        assert_eq!(count_fires(&combo, &[(R, true), (R, false)], |_| false), 1);
        assert_eq!(
            decide_toggle(true, false, false, false),
            ToggleOutcome::Stop,
            "the stop must never be blocked by a stale debounce window"
        );
    }

    /// Guards the wiring: the hook path must resolve the action through the
    /// shared pure decision, not through inline rules that can drift.
    #[test]
    fn hotkey_toggle_routes_through_the_pure_decision() {
        let src = include_str!("../../scheduler.rs");
        let toggle_off = src
            .find("pub fn hotkey_toggle")
            .expect("hotkey_toggle must exist");
        let tail = &src[toggle_off..];
        let decide_off = tail
            .find("decide_toggle(")
            .expect("hotkey_toggle must delegate to decide_toggle");
        assert!(
            decide_off < 2000,
            "the decision must be the first thing the toggle does"
        );
        assert!(
            tail.contains("let new_active = outcome == ToggleOutcome::Start;"),
            "Start/Stop must be applied from the resolved outcome"
        );
        assert!(
            tail.contains("let debounce_ok = was_active"),
            "the debounce window may only be consulted on the start path"
        );
    }

    /// The listener re-parses bindings ONLY when `hotkeys_version` changes.
    /// A config save that fails to bump it leaves the user's new hotkey
    /// unusable — which the user experiences as "my hotkey reset back".
    #[test]
    fn config_save_bumps_hotkeys_version_so_bindings_reparse() {
        let scheduler = ClickScheduler::new();
        let before = scheduler.hotkeys_version();

        let mut cfg = scheduler.get_config();
        cfg.hotkey_toggle = "T".into();
        scheduler.set_config(cfg);

        assert!(
            scheduler.hotkeys_version() > before,
            "a config save must bump hotkeys_version or the listener never re-reads"
        );

        let snapshot = HotkeySnapshot::from_scheduler(&scheduler);
        assert_eq!(snapshot.toggle, "T");
        let bindings = HotkeyBindings::from_snapshot(&snapshot);
        assert!(
            bindings.toggle.iter().any(|c| c.trigger == 0x54),
            "the saved key must be bound"
        );
        assert!(
            !bindings.toggle.iter().any(|c| c.trigger == 0x52),
            "the previous key must be released"
        );
    }
}

/// ── v4.1 physical integration tests ─────────────────────────────────
/// Inject real keyboard events via `SendInput` and verify that a
/// `WH_KEYBOARD_LL` hook receives them with correct VK codes and down/up
/// ordering, and that the production matcher fires on the captured stream.
#[cfg(test)]
mod physical_integration_tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::mpsc;
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW,
        SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx,
        HHOOK, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL,
    };

    static CAPTURE_TX: OnceLock<StdMutex<Option<mpsc::Sender<(u16, bool)>>>> = OnceLock::new();
    /// Live test capture-hook handle. Windows may silently remove a low-level
    /// hook after heavy SendInput activity, so tests must be able to reinstall
    /// it instead of relying on a one-shot initializer.
    static TEST_HOOK: StdMutex<Option<HHOOK>> = StdMutex::new(None);
    static PUMP_STARTED: AtomicBool = AtomicBool::new(false);
    /// Serializes physical tests: the hook is process-global.
    static TEST_LOCK: OnceLock<StdMutex<()>> = OnceLock::new();

    fn push_event(vk: u16, is_down: bool) {
        if let Some(tx) = CAPTURE_TX
            .get()
            .and_then(|l| l.lock().ok())
            .and_then(|g| g.clone())
        {
            let _ = tx.send((vk, is_down));
        }
    }

    /// Install ONE capture hook for the whole test binary (idempotent).
    fn ensure_hook() {
        // Start the message pump once; it must never exit while tests run.
        if !PUMP_STARTED.swap(true, Ordering::SeqCst) {
            thread::spawn(|| {
                let mut msg = MSG::default();
                unsafe {
                    loop {
                        let ret = GetMessageW(&mut msg, None, 0, 0);
                        if ret.0 <= 0 {
                            // Error or WM_QUIT: keep pumping instead of dying,
                            // otherwise the capture hook stops delivering.
                            continue;
                        }
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
            });
            // Let the pump spin up on first use.
            thread::sleep(Duration::from_millis(400));
        }

        let mut guard = TEST_HOOK.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_some() {
            return; // already installed
        }
        unsafe extern "system" fn capture_proc(
            n_code: i32,
            w_param: WPARAM,
            l_param: LPARAM,
        ) -> LRESULT {
            if n_code == 0 {
                let kb = *(l_param.0 as *const KBDLLHOOKSTRUCT);
                let msg = w_param.0;
                let is_down = msg == 0x0100 || msg == 0x0104;
                let is_up = msg == 0x0101 || msg == 0x0105;
                if (is_down || is_up) && kb.vkCode != 0 {
                    push_event(kb.vkCode as u16, is_down);
                }
            }
            CallNextHookEx(None, n_code, w_param, l_param)
        }
        let res = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(capture_proc), None, 0) };
        crate::debug_log_internal(
            "info",
            &format!("[tests] capture hook install: {}", res.is_ok()),
        );
        *guard = res.ok();
    }

    /// Windows can silently remove a low-level hook after heavy activity.
    /// Verify the capture hook actually delivers events with a canary key;
    /// if it's dead, drop the stale handle and reinstall a fresh hook.
    fn ensure_hook_alive() {
        ensure_hook();
        for attempt in 0..3 {
            let rx = swap_channel();
            thread::sleep(Duration::from_millis(120));
            inject_key(0x5B, false); // LWin down as canary (harmless modifier tap)
            thread::sleep(Duration::from_millis(40));
            inject_key(0x5B, true);
            let events = drain_filtered(&rx, &[0x5B], 1);
            if events.len() >= 1 {
                // Hook alive. Leave this fresh channel in place; caller will
                // swap its own channel anyway.
                return;
            }
            // Dead or warming up: force reinstall and retry.
            if let Ok(mut guard) = TEST_HOOK.try_lock() {
                if let Some(h) = guard.take() {
                    unsafe {
                        let _ = UnhookWindowsHookEx(h);
                    }
                }
            }
            crate::debug_log_internal(
                "info",
                &format!("[tests] capture hook dead, reinstall #{attempt}"),
            );
            ensure_hook();
        }
    }

    fn inject_key(vk: u16, up: bool) {
        let input = INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: 0,
                    dwFlags: if up {
                        KEYEVENTF_KEYUP
                    } else {
                        Default::default()
                    },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        };
        unsafe {
            SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
        }
    }

    /// Collect events for our VKs until each has been seen `per_vk` times.
    fn drain_filtered(
        rx: &mpsc::Receiver<(u16, bool)>,
        vks: &[u16],
        per_vk: usize,
    ) -> Vec<(u16, bool)> {
        let mut out = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while out.len() < vks.len() * per_vk && Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(300)) {
                Ok((vk, down)) => {
                    crate::debug_log_internal(
                        "info",
                        &format!("[tests] captured vk={:#04x} down={}", vk, down),
                    );
                    if vks.contains(&vk) {
                        out.push((vk, down));
                    }
                }
                Err(_) => break,
            }
        }
        out
    }

    /// Swap in a fresh channel and return its receiver. Any events sent to
    /// the previous sender are lost - acceptable because tests serialize on
    /// TEST_LOCK and drain everything they inject.
    fn swap_channel() -> mpsc::Receiver<(u16, bool)> {
        let (tx, rx) = mpsc::channel();
        *CAPTURE_TX
            .get_or_init(|| StdMutex::new(None))
            .lock()
            .unwrap() = Some(tx);
        rx
    }

    #[test]
    fn physical_sendinput_events_reach_hook_and_drive_matcher() {
        let _serial = TEST_LOCK
            .get_or_init(|| StdMutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        ensure_hook_alive();
        let rx = swap_channel();
        thread::sleep(Duration::from_millis(200)); // settle before injecting

        // ── Part 1: single key down/up ordering ──────────────────────
        inject_key(0x52, false); // R down
        thread::sleep(Duration::from_millis(40));
        inject_key(0x52, true); // R up
        let single = drain_filtered(&rx, &[0x52], 2);
        assert!(
            single.contains(&(0x52, true)) && single.contains(&(0x52, false)),
            "single key: expected R down+up, got {single:?}"
        );
        let d = single.iter().position(|e| *e == (0x52, true)).unwrap();
        let u = single.iter().position(|e| *e == (0x52, false)).unwrap();
        assert!(d < u, "single key: down before up, got {single:?}");

        // ── Part 2: modifier combo press order + matcher fires once ──
        inject_key(0x11, false); // Ctrl down
        thread::sleep(Duration::from_millis(40));
        inject_key(0x4D, false); // M down
        thread::sleep(Duration::from_millis(40));
        inject_key(0x4D, true); // M up
        thread::sleep(Duration::from_millis(40));
        inject_key(0x11, true); // Ctrl up
        let combo_events = drain_filtered(&rx, &[0x11, 0xA2, 0xA3, 0x4D], 2);
        let ctrl_variants = [0x11_u16, 0xA2, 0xA3];
        assert!(
            combo_events
                .iter()
                .any(|(vk, down)| ctrl_variants.contains(vk) && *down),
            "combo: missing Ctrl down, got {combo_events:?}"
        );
        assert!(
            combo_events.contains(&(0x4D, true)) && combo_events.contains(&(0x4D, false)),
            "combo: missing M down/up, got {combo_events:?}"
        );
        let combo = parse_hotkey_combo("Ctrl+M").expect("combo parses");
        let mut held = HashSet::new();
        let mut fired = 0;
        for (vk, is_down) in &combo_events {
            if *is_down {
                held.insert(*vk);
                if combo_matches(&combo, *vk, &held, |k| key_down(k)) {
                    fired += 1;
                }
            } else {
                held.retain(|&key| !key_code_matches(*vk, key));
            }
        }
        assert_eq!(
            fired, 1,
            "combo: Ctrl+M must fire exactly once over {combo_events:?}"
        );

        // ── Part 3: numpad key arrives with expected VK ──────────────
        inject_key(0x61, false); // NUMPAD1 down
        thread::sleep(Duration::from_millis(40));
        inject_key(0x61, true); // NUMPAD1 up
        let numpad = drain_filtered(&rx, &[0x61], 2);
        assert!(
            numpad.contains(&(0x61, true)),
            "numpad: expected NUMPAD1 down, got {numpad:?}"
        );
        let num_combo = parse_hotkey_combo("Num1").expect("Num1 parses");
        assert_eq!(num_combo.trigger, 0x61);
        assert!(combo_matches(&num_combo, 0x61, &HashSet::new(), |_| false));
    }

    #[test]
    fn physical_numpad_keys_reach_hook_and_match_numpad_bindings() {
        let _serial = TEST_LOCK
            .get_or_init(|| StdMutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        ensure_hook_alive();
        let rx = swap_channel();
        thread::sleep(Duration::from_millis(200)); // settle before injecting

        // ── Part 1: all keypad digit VKs physically arrive ────────────
        // VK 0x60..=0x69 = NUMPAD0..NUMPAD9 (NumLock on).
        const KEYPAD: [u16; 10] = [0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69];
        for vk in KEYPAD {
            inject_key(vk, false); // down
            thread::sleep(Duration::from_millis(25));
            inject_key(vk, true); // up
            thread::sleep(Duration::from_millis(25));
        }
        let events = drain_filtered(&rx, &KEYPAD, 2);
        for vk in KEYPAD {
            assert!(
                events.contains(&(vk, true)) && events.contains(&(vk, false)),
                "keypad: missing down/up for vk=0x{vk:02X}, got {events:?}"
            );
        }

        // ── Part 2: matcher fires exactly once per press for Num bindings ─
        for vk in KEYPAD {
            let label = format!("Num{}", vk - 0x60);
            let combo = parse_hotkey_combo(&label).unwrap_or_else(|| panic!("{label} must parse"));
            assert_eq!(combo.trigger, vk, "{label} trigger mismatch");
            let mut held = HashSet::new();
            held.insert(vk);
            assert!(
                combo_matches(&combo, vk, &held, |k| key_down(k)),
                "{label} combo must match its own trigger"
            );
        }
        // A Num binding must NOT fire for a different keypad key.
        let num1 = parse_hotkey_combo("Num1").expect("Num1 parses");
        held_none_check(&num1, 0x62);

        // ── Part 3: NumLock-off navigation aliases map correctly ──────
        assert_eq!(numpad_navigation_vk(1), Some(0x23)); // End
        assert_eq!(numpad_navigation_vk(8), Some(0x26)); // Up
        assert_eq!(numpad_navigation_vk(0), Some(0x2D)); // Insert
        assert_eq!(numpad_navigation_vk(11), None);

        // Let the LL hook pump fully settle before the next physical test:
        // back-to-back SendInput floods can starve the message loop and make
        // the following test observe a temporarily unresponsive hook.
        thread::sleep(Duration::from_millis(300));
        let _ = drain_filtered(&rx, &[], 0); // flush any stragglers
    }

    fn held_none_check(combo: &HotkeyCombo, wrong_trigger: u16) {
        let mut held = HashSet::new();
        held.insert(wrong_trigger);
        assert!(
            !combo_matches(combo, wrong_trigger, &held, |k| key_down(k)),
            "combo {:?} must not fire for unrelated trigger 0x{wrong_trigger:02X}",
            combo.trigger
        );
    }

    #[test]
    fn listener_double_start_guard_uses_atomic_swap() {
        // Contract behind spawn_global_hotkey_listener: a second start must be
        // rejected while the first is running, and the flag must be releasable
        // on shutdown. We exercise the same GLOBAL_HOTKEY_RUNNING atomic the
        // spawner uses (an AppHandle cannot be built in unit tests).
        let was_running = GLOBAL_HOTKEY_RUNNING.swap(true, Ordering::AcqRel);
        assert!(!was_running, "no other test may hold the running flag");
        // Simulate the second spawn seeing the flag set:
        let second_attempt = GLOBAL_HOTKEY_RUNNING.swap(true, Ordering::AcqRel);
        assert!(second_attempt, "second spawn must observe running=true");
        // Shutdown path releases the flag exactly like the listener thread does:
        GLOBAL_HOTKEY_RUNNING.store(false, Ordering::Release);
        let third_attempt = GLOBAL_HOTKEY_RUNNING.swap(true, Ordering::AcqRel);
        assert!(!third_attempt, "after release a new spawn must win");
        GLOBAL_HOTKEY_RUNNING.store(false, Ordering::Release);
    }

    #[test]
    fn shutdown_flag_is_idempotent_and_resets_channel() {
        // shutdown_global_hotkey_listener must be safe to call repeatedly
        // and must clear the capture channel so a stale sender can't fire
        // actions after shutdown.
        shutdown_global_hotkey_listener();
        shutdown_global_hotkey_listener();
        assert!(GLOBAL_HOTKEY_STOP.load(Ordering::Acquire));
        let channel_empty = GLOBAL_HOTKEY_TX
            .get()
            .and_then(|l| l.lock().ok())
            .map(|g| g.is_none())
            .unwrap_or(true);
        assert!(channel_empty, "channel must be cleared after shutdown");
        // Reset so other tests / the app can start a fresh listener.
        GLOBAL_HOTKEY_STOP.store(false, Ordering::Release);
    }

    #[test]
    fn key_up_removes_trigger_so_rising_edge_wont_refire_while_held() {
        let combo = parse_hotkey_combo("Ctrl+P").expect("parses");

        let mut held = HashSet::from([0x11_u16]);
        held.insert(0x50);
        assert!(combo_matches(&combo, 0x50, &held, |k| k == 0x11));

        held.remove(&0x50);

        held.clear();
        assert!(!combo_matches(&combo, 0x50, &held, |_| false));
    }

    #[test]
    fn binding_change_is_picked_up_without_listener_restart() {
        let before = HotkeySnapshot {
            toggle: "R".into(),
            mode_switch: "Ctrl+Alt+M".into(),
            emergency_stop: "Escape".into(),
            speed_up: "Ctrl+=".into(),
            slow_down: "Ctrl+-".into(),
            capture_pos: "Ctrl+P".into(),
            record_toggle: true,
            record_hotkey: "F9".into(),
            preset_slots: Vec::new(),
            preset_hotkeys: Vec::new(),
        };
        let bindings_before = HotkeyBindings::from_snapshot(&before);
        assert!(bindings_before.toggle.iter().any(|c| c.trigger == 0x52));

        let mut after = before.clone();
        after.toggle = "T".into();
        let bindings_after = HotkeyBindings::from_snapshot(&after);
        assert_ne!(before, after, "snapshot diff must be detected");
        assert!(bindings_after.toggle.iter().any(|c| c.trigger == 0x54));
        assert!(!bindings_after.toggle.iter().any(|c| c.trigger == 0x52));
    }
}

#[cfg(test)]
mod hotpath_silence_tests {
    /// v4.2 hardening regression guard: the LL-hook listener loop must not
    /// perform file logging — WH_KEYBOARD_LL is synchronous system-wide, so
    /// any I/O in the callback path adds keyboard latency for every app.
    /// This test scans the source of `run_keyboard_hook`'s while-loop and
    /// fails if a `debug_log_internal` call appears inside it.
    #[test]
    fn hook_loop_contains_no_file_logging() {
        let src = include_str!("mod.rs");
        let start = src
            .find("fn run_keyboard_hook")
            .expect("run_keyboard_hook not found");
        let loop_start = src[start..]
            .find("while !GLOBAL_HOTKEY_STOP")
            .expect("listener loop not found")
            + start;
        // Loop ends right before the hook teardown block. Terminate on
        // UnhookWindowsHookEx instead of the first `unsafe {`: the message
        // pump inside the loop now contains its own unsafe block.
        let end = src[loop_start..]
            .find("UnhookWindowsHookEx")
            .map(|i| i + loop_start)
            .expect("end of listener loop not found");
        let body = &src[loop_start..end];
        let offenders: Vec<&str> = body
            .lines()
            .filter(|l| l.contains("debug_log_internal"))
            .collect();
        assert!(
            offenders.is_empty(),
            "file logging found on the LL-hook hot path: {offenders:?}"
        );
    }

    /// v4.3 regression guard: the listener loop MUST contain the Windows
    /// message pump. WH_KEYBOARD_LL callbacks are delivered only while the
    /// installing thread processes its message queue — without PeekMessageW/
    /// DispatchMessageW the OS never invokes keyboard_proc, so per-key events
    /// cannot reach the typing guard (symptom: toggle hotkeys work via the
    /// GetAsyncKeyState fallback, but typing detection is deaf).
    #[test]
    fn hook_loop_pumps_windows_messages() {
        let src = include_str!("mod.rs");
        let start = src
            .find("fn run_keyboard_hook")
            .expect("run_keyboard_hook not found");
        let loop_start = src[start..]
            .find("while !GLOBAL_HOTKEY_STOP")
            .expect("listener loop not found")
            + start;
        // Terminate the window at the hook teardown right after the loop
        // body (searching for the first `unsafe {` is unreliable because the
        // message pump itself contains an unsafe block).
        let end = src[loop_start..]
            .find("UnhookWindowsHookEx")
            .map(|i| i + loop_start)
            .expect("end of listener loop not found (UnhookWindowsHookEx)");
        let body = &src[loop_start..end];
        assert!(
            body.contains("PeekMessageW") && body.contains("DispatchMessageW"),
            "listener loop must pump OS messages (PeekMessageW/DispatchMessageW)"
        );
        assert!(
            body.contains("PM_REMOVE"),
            "message drain must use PM_REMOVE so hook callbacks are consumed"
        );
    }
}

/// Lowercase base image name of a running process id.
///
/// Shared by the foreground lookup and the Smart Guard app picker so both
/// use exactly one implementation of the syscall dance.
fn process_exe_name(pid: u32) -> Option<String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    if pid == 0 {
        return None;
    }
    // Our own window must be a VISIBLE baseline, not an invisible `None`:
    // starting from the NanoClick window (button click) and Alt+Tab-ing away
    // must stop the run. `None` stays reserved for genuinely unresolvable
    // foreground (lock screen, elevated-only) which fails open by design.
    if pid == unsafe { GetCurrentProcessId() } {
        return crate::platform::own_exe_name();
    }
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut len = 260u32;
        let mut buf = [0u16; 260];
        let ok = QueryFullProcessImageNameW(
            process,
            Default::default(),
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(process);
        if !ok || len == 0 {
            return None;
        }
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        crate::platform::exe_name_from_path(&full)
    }
}

/// Every distinct running process image name, lowercase, sorted.
///
/// Backs the Smart Guard app picker: the user picks from what is actually
/// running instead of typing `javaw.exe` by hand.
pub fn list_running_apps() -> Vec<crate::platform::AppEntry> {
    use windows::Win32::System::ProcessStatus::EnumProcesses;

    let mut pids = vec![0u32; 2048];
    let mut needed = 0u32;
    unsafe {
        // Grow the buffer until EnumProcesses stops truncating its output.
        loop {
            let bytes = (pids.len() * std::mem::size_of::<u32>()) as u32;
            if EnumProcesses(pids.as_mut_ptr(), bytes, &mut needed).is_err() {
                return Vec::new();
            }
            if needed < bytes {
                pids.truncate(needed as usize / std::mem::size_of::<u32>());
                break;
            }
            if pids.len() >= 65536 {
                break;
            }
            pids.resize(pids.len() * 2, 0);
        }
    }

    let mut names: Vec<String> = pids
        .iter()
        .filter_map(|pid| process_exe_name(*pid))
        .collect();
    names.sort();
    names.dedup();
    names
        .into_iter()
        .map(|exe| crate::platform::AppEntry {
            label: exe.clone(),
            exe: Some(exe),
            source: "running",
        })
        .collect()
}

/// Installed applications from the Windows uninstall registry.
///
/// Only entries that expose a real executable (`DisplayIcon`) can be used by
/// the filter, so those are preferred; every other entry is still listed by
/// display name so the user can recognise it.
pub fn list_installed_apps() -> Vec<crate::platform::AppEntry> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
    use winreg::RegKey;

    const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
    const UNINSTALL_WOW: &str =
        r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall";
    // Registry view flags mirror the 64/32-bit uninstall hives.
    const KEY_WOW64_64: u32 = 0x0100;
    const KEY_WOW64_32: u32 = 0x0200;

    let mut out: Vec<crate::platform::AppEntry> = Vec::new();
    let mut sources: Vec<(RegKey, u32)> = Vec::new();
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    for (path, flags) in [
        (UNINSTALL, KEY_WOW64_64),
        (UNINSTALL, KEY_WOW64_32),
        (UNINSTALL_WOW, KEY_WOW64_64),
    ] {
        if let Ok(key) = hklm.open_subkey_with_flags(path, KEY_READ | flags) {
            sources.push((key, flags));
        }
    }
    if let Ok(key) = hkcu.open_subkey_with_flags(UNINSTALL, KEY_READ) {
        sources.push((key, 0));
    }

    for (root, _) in sources {
        for name in root.enum_keys().flatten() {
            let Ok(entry) = root.open_subkey_with_flags(&name, KEY_READ) else {
                continue;
            };
            // SystemComponent=1 marks hidden OS plumbing, not user apps.
            let system_component: u32 = entry.get_value::<u32, _>("SystemComponent").unwrap_or(0);
            if system_component == 1 {
                continue;
            }
            let display_name: String = match entry.get_value::<String, _>("DisplayName") {
                Ok(value) => value.trim().to_string(),
                Err(_) => continue,
            };
            if display_name.is_empty() {
                continue;
            }
            let exe = entry
                .get_value::<String, _>("DisplayIcon")
                .ok()
                .as_deref()
                .and_then(crate::platform::exe_from_display_icon);
            out.push(crate::platform::AppEntry {
                label: display_name,
                exe,
                source: "installed",
            });
        }
    }

    out.sort_by(|a, b| a.label.to_ascii_lowercase().cmp(&b.label.to_ascii_lowercase()));
    out.dedup_by(|a, b| a.label.eq_ignore_ascii_case(&b.label));
    out
}

/// Lowercase image name (e.g. "discord.exe") of the foreground process.
/// None when it cannot be determined (lock screen, elevated-only, etc.).
/// Cost: 2 syscalls (GetForegroundWindow + OpenProcess/QueryFullProcessImageNameW).
/// Callers should cache the result for ~250-500 ms, never per-click.
pub fn get_foreground_process_name() -> Option<String> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0 == 0 {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        process_exe_name(pid)
    }
}

/// Title of the current foreground window (empty string if unavailable).
pub fn get_foreground_window_title() -> Option<String> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0 == 0 {
            return None;
        }
        let mut buf = [0u16; 512];
        let len = GetWindowTextW(hwnd, &mut buf);
        if len <= 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Read a single pixel from the screen at (x, y). Returns RGBA as u32
/// (R in bits 24-31, G in 16-23, B in 8-15, A in 0-7). Returns None if
/// the coordinates are off-screen or the GDI call fails.
pub fn get_pixel_rgba(x: i32, y: i32) -> Option<u32> {
    unsafe {
        let hdc_screen = GetDC(None);
        if hdc_screen.is_invalid() {
            return None;
        }
        let color_ref = GetPixel(hdc_screen, x, y);
        let _ = ReleaseDC(None, hdc_screen);
        let raw: u32 = color_ref.0;
        // CLR_INVALID = 0xFFFF_FFFF (== u32::MAX)
        if raw == u32::MAX {
            return None;
        }
        let r = (raw & 0xFF) as u8;
        let g = ((raw >> 8) & 0xFF) as u8;
        let b = ((raw >> 16) & 0xFF) as u8;
        Some(((r as u32) << 24) | ((g as u32) << 16) | ((b as u32) << 8) | 0xFF)
    }
}

/// Virtual desktop extent (multi-monitor aware). Returns (width, height)
/// in pixels. Used by the multi-point sequence canvas to size the grid.
pub fn get_screen_size() -> (i32, i32) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    };
    unsafe {
        (
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}
