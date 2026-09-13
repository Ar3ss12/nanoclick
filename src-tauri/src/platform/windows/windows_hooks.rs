//! # Windows low-level hooks for Macro Recorder.
//!
//! Spawns a dedicated thread that installs both `WH_MOUSE_LL` and
//! `WH_KEYBOARD_LL` hooks. Every event the user produces on the system
//! (clicks, key presses, cursor moves, scrolling) is captured and pushed
//! into the supplied [`Sender<RawEvent>`] channel, where it lands in the
//! recorder's consumer thread.
//!
//! Reference: `docs/MACRO_ARCHITECTURE.md` §3.2.
//!
//! ## Lifecycle
//! 1. Tauri command `record_start` calls [`spawn_recorder_hooks`], passing
//!    the recorded-event sender.
//! 2. The hook thread installs both hooks and enters the standard Windows
//!    `GetMessageW` pump.
//! 3. `record_stop` calls [`stop_recorder_hooks`] which sets a stop flag,
//!    posts a quit message (WM_QUIT) to break out of the message pump, and
//!    unhooks both hooks.
//! 4. The thread exits, ending all captures.

use crate::core::action::{KeyCode, Modifiers, MouseButton};
use crate::recorder::raw_event::RawEvent;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::thread::JoinHandle;
use std::time::Instant;

use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, DispatchMessageW, GetMessageW, PostThreadMessageW,
    SetWindowsHookExW, TranslateMessage, UnhookWindowsHookEx, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT,
    WH_KEYBOARD_LL, WH_MOUSE_LL, WM_QUIT,
};

static STOP_FLAG: AtomicBool = AtomicBool::new(false);
static RECORDER_RUNNING: AtomicBool = AtomicBool::new(false);
static RECORDER_THREAD_ID: AtomicU32 = AtomicU32::new(0);
static RECORDER_CONTEXT: OnceLock<Mutex<Option<Arc<RecorderContext>>>> = OnceLock::new();
static RECORDER_THREAD: OnceLock<Mutex<Option<JoinHandle<()>>>> = OnceLock::new();
// Silence imports that are unused after we keep only what we use.
use std::sync::atomic::AtomicU32;

/// Start (or restart) the recorder hooks on a fresh background thread.
///
/// Re-calling this reuses the same thread ID store. Returns immediately;
/// use [`stop_recorder_hooks`] to break the message pump.
pub fn spawn_recorder_hooks(
    tx: Sender<RawEvent>,
    ignored_hotkey: String,
) -> Arc<dyn std::any::Any + Send + Sync> {
    // Avoid replacing raw-pointer resources while a previous hook thread exits.
    stop_recorder_hooks();
    // Reset stop flag.
    STOP_FLAG.store(false, Ordering::Release);
    let any: Arc<dyn std::any::Any + Send + Sync> = Arc::new(());
    let thread = thread::spawn(move || {
        let tid = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
        RECORDER_THREAD_ID.store(tid, Ordering::Release);
        run_hooks(tx, IgnoredHotkey::parse(&ignored_hotkey));
        drop(any);
    });
    *RECORDER_THREAD
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap() = Some(thread);
    let any_keep: Arc<dyn std::any::Any + Send + Sync> = Arc::new(());
    any_keep
}

/// ── v4.2 Platform Abstraction ─────────────────────────────────────────
/// Concrete Windows recorder backend implementing the shared contract.
/// The ignored-hotkey label stays a config-string concern: it is parsed
/// here at the platform boundary, never inside core/commands.
pub struct WindowsRecorderBackend {
    /// Config label of the hotkey the recorder must not capture
    /// (e.g. "Ctrl+Shift+R"). Parsed once via `IgnoredHotkey::parse`.
    pub ignored_hotkey_label: std::sync::Mutex<String>,
}

impl Default for WindowsRecorderBackend {
    fn default() -> Self {
        WindowsRecorderBackend {
            ignored_hotkey_label: std::sync::Mutex::new(String::new()),
        }
    }
}

impl WindowsRecorderBackend {
    pub fn new(label: impl Into<String>) -> Self {
        WindowsRecorderBackend {
            ignored_hotkey_label: std::sync::Mutex::new(label.into()),
        }
    }
}

impl crate::platform::backend::RecorderBackend for WindowsRecorderBackend {
    fn start(&self, sender: Sender<RawEvent>) -> Result<(), String> {
        let label = self.ignored_hotkey_label.lock().unwrap().clone();
        spawn_recorder_hooks(sender, label);
        Ok(())
    }

    fn stop(&self) {
        stop_recorder_hooks();
    }
}

/// Set the stop flag and post WM_QUIT to the recorder thread. After this
/// the thread unhooks and exits. Safe to call multiple times.
pub fn stop_recorder_hooks() {
    STOP_FLAG.store(true, Ordering::Release);
    let tid = RECORDER_THREAD_ID.load(Ordering::Acquire);
    if tid != 0 {
        unsafe {
            let _ = PostThreadMessageW(tid, WM_QUIT, WPARAM(0), LPARAM(0));
        }
    }
    if let Some(thread) = RECORDER_THREAD
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap()
        .take()
    {
        let _ = thread.join();
    }
}

fn now_ms_since(start: Instant) -> u64 {
    start.elapsed().as_millis() as u64
}

fn read_modifiers() -> Modifiers {
    let m = Modifiers {
        ctrl: unsafe { (GetAsyncKeyState(VK_CONTROL.0 as i32) as u32 & 0x8000) != 0 },
        alt: unsafe { (GetAsyncKeyState(VK_MENU.0 as i32) as u32 & 0x8000) != 0 },
        shift: unsafe { (GetAsyncKeyState(VK_SHIFT.0 as i32) as u32 & 0x8000) != 0 },
        win: unsafe {
            (GetAsyncKeyState(VK_LWIN.0 as i32) as u32 & 0x8000) != 0
                || (GetAsyncKeyState(0x5C) /* VK_RWIN */ as i32 as u32 & 0x8000) != 0
        },
    };
    m
}

#[derive(Clone, Debug)]
struct IgnoredHotkey {
    required: Vec<u16>,
    trigger: u16,
}

impl IgnoredHotkey {
    fn parse(label: &str) -> Option<Self> {
        let mut out = Self {
            required: Vec::new(),
            trigger: 0,
        };
        let parts: Vec<&str> = label.split('+').map(str::trim).collect();
        if parts.is_empty() {
            return None;
        }
        out.trigger = key_to_vk(parts.last().copied().unwrap_or_default());
        out.required = parts[..parts.len().saturating_sub(1)]
            .iter()
            .map(|part| key_to_vk(part))
            .map(|key| (key != 0).then_some(key))
            .collect::<Option<Vec<_>>>()?;
        (out.trigger != 0).then_some(out)
    }

    fn matches(&self, key: u16, _mods: Modifiers) -> bool {
        key_matches(self.trigger, key)
            && self.required.iter().all(|required| key_is_down(*required))
    }
}

fn key_matches(expected: u16, actual: u16) -> bool {
    match expected {
        // `*` can arrive as numpad multiply or Shift+8.
        0x6A => matches!(actual, 0x6A | 0x38),
        // A top-row digit can arrive from NumLock-on keypad or as its
        // navigation key when NumLock is off.
        0x30..=0x39 => {
            actual == expected
                || actual == 0x60 + (expected - 0x30)
                || matches!(
                    (expected - 0x30, actual),
                    (0, 0x2D)
                        | (1, 0x23)
                        | (2, 0x28)
                        | (3, 0x22)
                        | (4, 0x25)
                        | (5, 0x0C)
                        | (6, 0x27)
                        | (7, 0x24)
                        | (8, 0x26)
                        | (9, 0x21)
                )
        }
        _ => expected == actual,
    }
}

fn key_to_vk(key: &str) -> u16 {
    let key = key.trim().to_ascii_lowercase();
    match key.as_str() {
        "escape" | "esc" => 0x1B,
        "space" => 0x20,
        "enter" | "return" => 0x0D,
        "tab" => 0x09,
        "backspace" => 0x08,
        "delete" | "del" => 0x2E,
        "insert" | "ins" => 0x2D,
        "ctrl" | "control" => 0x11,
        "alt" | "menu" => 0x12,
        "shift" => 0x10,
        "win" | "windows" | "meta" | "super" => 0x5B,
        "=" | "+" | "plus" => 0xBB,
        "*" | "asterisk" => 0x6A,
        "-" | "minus" => 0xBD,
        _ if key.len() == 1 && key.as_bytes()[0].is_ascii_alphanumeric() => {
            key.as_bytes()[0].to_ascii_uppercase() as u16
        }
        _ if key
            .strip_prefix('f')
            .and_then(|n| n.parse::<u16>().ok())
            .is_some() =>
        {
            let n = key[1..].parse::<u16>().unwrap_or(0);
            if (1..=24).contains(&n) {
                0x6F + n
            } else {
                0
            }
        }
        _ if key
            .strip_prefix("num")
            .and_then(|n| n.parse::<u16>().ok())
            .is_some() =>
        {
            let n = key[3..].parse::<u16>().unwrap_or(10);
            if n <= 9 {
                0x60 + n
            } else {
                0
            }
        }
        _ => 0,
    }
}

fn key_is_down(vk: u16) -> bool {
    let aliases: &[u16] = match vk {
        0x6A => &[0x6A, 0x38],
        0x10 => &[0x10, 0xA0, 0xA1],
        0x11 => &[0x11, 0xA2, 0xA3],
        0x12 => &[0x12, 0xA4, 0xA5],
        0x5B => &[0x5B, 0x5C],
        _ => &[vk],
    };
    aliases
        .iter()
        .any(|key| unsafe { (GetAsyncKeyState(*key as i32) as i32 & 0x8000) != 0 })
}

struct RecorderContext {
    tx: Sender<RawEvent>,
    start: Instant,
    ignored_hotkey: Option<IgnoredHotkey>,
}

thread_local! {
    static LOCAL_CONTEXT: std::cell::RefCell<Option<Arc<RecorderContext>>> = const { std::cell::RefCell::new(None) };
    static LAST_MOUSE_MOVE: std::cell::Cell<(i32, i32, u64)> = const { std::cell::Cell::new((-99999, -99999, 0)) };
}

fn with_context<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&RecorderContext) -> R,
{
    LOCAL_CONTEXT.with(|cell| {
        let guard = cell.borrow();
        if let Some(ctx) = guard.as_ref() {
            Some(f(ctx))
        } else {
            RECORDER_CONTEXT
                .get_or_init(|| Mutex::new(None))
                .lock()
                .ok()
                .and_then(|lock| lock.as_ref().map(|arc| f(arc.as_ref())))
        }
    })
}

fn clear_recorder_context() {
    LOCAL_CONTEXT.with(|cell| *cell.borrow_mut() = None);
    if let Some(lock) = RECORDER_CONTEXT.get() {
        if let Ok(mut context) = lock.lock() {
            *context = None;
        }
    }
    RECORDER_THREAD_ID.store(0, Ordering::Release);
    RECORDER_RUNNING.store(false, Ordering::Release);
}

unsafe extern "system" fn keyboard_proc(n_code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    if n_code == 0 && !STOP_FLAG.load(Ordering::Acquire) {
        let kb = *(l_param.0 as *const KBDLLHOOKSTRUCT);
        let vk = kb.vkCode as u16;
        // Filter out modifier-only keys themselves (we already capture their state).
        let is_modifier_vk = matches!(vk, 0x10 | 0x11 | 0x12 | 0x5B | 0x5C);
        if is_modifier_vk {
            return CallNextHookEx(None, n_code, w_param, l_param);
        }
        let msg = w_param.0;
        let res = with_context(|context| {
            let t_ms = now_ms_since(context.start);
            let mods = read_modifiers();
            let ev = if msg == 0x0100 || msg == 0x0104 {
                Some(RawEvent::KeyDown {
                    key: KeyCode(vk),
                    mods,
                    t_ms,
                })
            } else if msg == 0x0101 || msg == 0x0105 {
                Some(RawEvent::KeyUp {
                    key: KeyCode(vk),
                    mods,
                    t_ms,
                })
            } else {
                None
            };
            if let Some(e) = ev {
                if let Some(ignore) = context.ignored_hotkey.as_ref() {
                    if ignore.matches(vk, mods) {
                        return true; // ignored
                    }
                }
                let _ = context.tx.send(e);
            }
            false
        });
        if res == Some(true) {
            return CallNextHookEx(None, n_code, w_param, l_param);
        }
    }
    CallNextHookEx(None, n_code, w_param, l_param)
}

unsafe extern "system" fn mouse_proc(n_code: i32, w_param: WPARAM, l_param: LPARAM) -> LRESULT {
    if n_code == 0 && !STOP_FLAG.load(Ordering::Acquire) {
        let m_struct = &*(l_param.0 as *const MSLLHOOKSTRUCT);
        let msg = w_param.0;
        let pt = POINT {
            x: m_struct.pt.x,
            y: m_struct.pt.y,
        };
        with_context(|context| {
            let t_ms = now_ms_since(context.start);
            let ev = match msg {
                0x0200 => {
                    let should_send = LAST_MOUSE_MOVE.with(|cell| {
                        let (lx, ly, lt) = cell.get();
                        let dx = (pt.x - lx).abs();
                        let dy = (pt.y - ly).abs();
                        let dt = t_ms.saturating_sub(lt);
                        if dx >= 4 || dy >= 4 || dt >= 30 {
                            cell.set((pt.x, pt.y, t_ms));
                            true
                        } else {
                            false
                        }
                    });
                    if should_send {
                        Some(RawEvent::MouseMove {
                            x: pt.x,
                            y: pt.y,
                            t_ms,
                        })
                    } else {
                        None
                    }
                }
                0x0201 => Some(RawEvent::MouseDown {
                    button: MouseButton::Left,
                    t_ms,
                }),
                0x0202 => Some(RawEvent::MouseUp {
                    button: MouseButton::Left,
                    t_ms,
                }),
                0x0204 => Some(RawEvent::MouseDown {
                    button: MouseButton::Right,
                    t_ms,
                }),
                0x0205 => Some(RawEvent::MouseUp {
                    button: MouseButton::Right,
                    t_ms,
                }),
                0x0207 => Some(RawEvent::MouseDown {
                    button: MouseButton::Middle,
                    t_ms,
                }),
                0x0208 => Some(RawEvent::MouseUp {
                    button: MouseButton::Middle,
                    t_ms,
                }),
                0x020A | 0x020E => {
                    let delta = (m_struct.mouseData >> 16) as i16 as i32;
                    Some(RawEvent::Scroll {
                        delta_x: 0,
                        delta_y: delta,
                        t_ms,
                    })
                }
                0x020B | 0x020C => {
                    let button = match (m_struct.mouseData >> 16) as u16 {
                        1 => MouseButton::X1,
                        2 => MouseButton::X2,
                        _ => return,
                    };
                    if msg == 0x020B {
                        Some(RawEvent::MouseDown { button, t_ms })
                    } else {
                        Some(RawEvent::MouseUp { button, t_ms })
                    }
                }
                _ => None,
            };
            if let Some(e) = ev {
                let _ = context.tx.send(e);
            }
        });
    }
    CallNextHookEx(None, n_code, w_param, l_param)
}

fn run_hooks(tx: Sender<RawEvent>, ignored_hotkey: Option<IgnoredHotkey>) {
    RECORDER_RUNNING.store(true, Ordering::Release);
    let start = Instant::now();
    let ctx = Arc::new(RecorderContext {
        tx,
        start,
        ignored_hotkey,
    });
    LOCAL_CONTEXT.with(|cell| *cell.borrow_mut() = Some(ctx.clone()));
    *RECORDER_CONTEXT
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap() = Some(ctx);

    unsafe {
        let kb_hook = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), None, 0) {
            Ok(hook) => hook,
            Err(error) => {
                crate::debug_log_internal(
                    "error",
                    &format!("[Recorder] keyboard hook install failed: {:?}", error),
                );
                clear_recorder_context();
                return;
            }
        };
        let ms_hook = match SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), None, 0) {
            Ok(hook) => hook,
            Err(error) => {
                let _ = UnhookWindowsHookEx(kb_hook);
                crate::debug_log_internal(
                    "error",
                    &format!("[Recorder] mouse hook install failed: {:?}", error),
                );
                clear_recorder_context();
                return;
            }
        };

        let mut msg = MSG::default();
        // GetMessageW returns 0 on WM_QUIT — that's our stop signal.
        while GetMessageW(&mut msg, None, 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
            if STOP_FLAG.load(Ordering::Acquire) {
                break;
            }
        }

        let _ = UnhookWindowsHookEx(kb_hook);
        let _ = UnhookWindowsHookEx(ms_hook);

        clear_recorder_context();
    }
}

// ─── tests ────────────────────────────────────────────────────────
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_sets_flag() {
        STOP_FLAG.store(false, Ordering::Release);
        stop_recorder_hooks();
        assert!(STOP_FLAG.load(Ordering::Acquire));
        // Reset for other tests.
        STOP_FLAG.store(false, Ordering::Release);
    }

    #[test]
    fn read_modifiers_returns_struct_with_defaults() {
        let m = read_modifiers();
        // We can't assert the actual booleans (they depend on the OS state)
        // but we can construct and read them.
        let _ = m.ctrl;
        let _ = m.alt;
        let _ = m.shift;
        let _ = m.win;
    }

    #[test]
    fn ignored_hotkey_supports_star_plus_one_and_numpad_keys() {
        let hotkey = IgnoredHotkey::parse("*+1").expect("combo should parse");
        assert_eq!(hotkey.required, vec![0x6A]);
        assert_eq!(hotkey.trigger, 0x31);
        assert!(key_matches(hotkey.trigger, 0x61));
        assert!(key_matches(hotkey.trigger, 0x23));

        let numpad = IgnoredHotkey::parse("Ctrl+Num1").expect("numpad combo should parse");
        assert_eq!(numpad.required, vec![0x11]);
        assert_eq!(numpad.trigger, 0x61);
    }

    #[test]
    fn ignored_hotkey_rejects_unknown_trigger() {
        assert!(IgnoredHotkey::parse("Ctrl+NotAKey").is_none());
    }
}
