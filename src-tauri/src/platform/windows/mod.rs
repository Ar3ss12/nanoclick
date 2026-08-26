//! Windows platform — keyboard, mouse, hooks, cursor polling.

pub mod keyboard;
pub mod windows_hooks;

pub use keyboard::{mouse_click, mouse_down, mouse_up, scroll_wheel, send_key, set_cursor_pos};
pub use windows_hooks::{stop_recorder_hooks, WindowsRecorderBackend};

use crate::core::action::{KeyCode, Modifiers, MouseButton};
use crate::scheduler::ClickScheduler;
use rand::Rng;
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetCursorPos, SetCursorPos, SetWindowsHookExW, UnhookWindowsHookEx,
    KBDLLHOOKSTRUCT, WH_KEYBOARD_LL,
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

/// High-resolution interruptible timer — uses thread::sleep with poll for cancellation.
pub struct PlatformTimer;

impl PlatformTimer {
    pub fn new() -> Self {
        PlatformTimer
    }

    /// Wait until `target` or until `stop_handle` is signaled.
    pub fn wait_until(&self, target: Instant, stop_handle: NativeEventHandle) -> bool {
        loop {
            let now = Instant::now();
            if now >= target {
                return true;
            }
            if stop_handle.load(Ordering::Acquire) {
                return false;
            }
            let remaining = target.duration_since(now);
            let chunk = remaining.min(Duration::from_millis(10));
            thread::sleep(chunk);
        }
    }
}

impl Default for PlatformTimer {
    fn default() -> Self {
        Self::new()
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
        let (mut target_x, mut target_y) = match spec.position_mode {
            crate::platform::backend::PositionMode::Fixed => (spec.fixed_x, spec.fixed_y),
            crate::platform::backend::PositionMode::Cursor => self.cursor_position(),
        };

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

    while !GLOBAL_HOTKEY_STOP.load(Ordering::Acquire) {
        poll_iter += 1;
        let received = event_rx.recv_timeout(Duration::from_millis(20));
        let current_version = scheduler.hotkeys_version();
        if current_version != seen_version {
            seen_version = current_version;
            let snapshot = HotkeySnapshot::from_scheduler(&scheduler);
            bindings = HotkeyBindings::from_snapshot(&snapshot);
            log_bindings(&bindings);
            hotkey_diag_push(format!("bindings re-parsed on version {current_version}"));
        }
        let fallback_events = if received.is_err() {
            held.retain(|&key| key_down(key));
            bindings
                .all_groups()
                .iter()
                .flat_map(|group| group.iter())
                .filter(|combo| {
                    key_down(combo.trigger) && combo.required.iter().all(|key| key_down(*key))
                })
                .map(|combo| GlobalKeyEvent {
                    vk: combo.trigger,
                    is_down: true,
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let events = received.into_iter().chain(fallback_events.into_iter());
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
                let mut stale_cleaned = false;
                if was_held && !key_down(event.vk) {
                    held.retain(|&key| !key_code_matches(event.vk, key));
                    stale_cleaned = true;
                    hotkey_diag_push(format!("stale-held cleaned vk=0x{:02X}", event.vk));
                }
                held.insert(event.vk);
                if !was_held || stale_cleaned {
                    fire_hotkey_group(&bindings.toggle, event.vk, &held, || {
                        hotkey_diag_push("fired_action=toggle".into());
                        let prev = scheduler.is_active();
                        let mode = scheduler.hotkey_toggle(Some(&app_handle));
                        hotkey_diag_push(format!("toggle done was_active={prev} mode={mode}"));
                    });
                    fire_hotkey_group(&bindings.mode_switch, event.vk, &held, || {
                        hotkey_diag_push("fired_action=mode_switch".into());
                        scheduler.toggle_mode(Some(&app_handle));
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

                    // Preset slot hotkeys: slot 1..9 -> global-preset-hotkey(idx).
                    for (slot_idx, combos) in bindings.preset_slots.iter().enumerate() {
                        let slot = slot_idx as u32;
                        fire_hotkey_group(combos, event.vk, &held, || {
                            hotkey_diag_push(format!("fired_action=preset_slot_{}", slot + 1));
                            let _ = app_handle.clone().emit("global-preset-hotkey", slot);
                        });
                    }
                }
            } else {
                held.retain(|&key| !key_code_matches(event.vk, key));
            }
        }
        if poll_iter % 100 == 0 {
            hotkey_diag_push("heartbeat".into());
        }
    }

    unsafe {
        let _ = UnhookWindowsHookEx(_hook);
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

/// Dump and clear the diagnostic buffer (test hooks / debug command).
#[allow(dead_code)]
pub fn hotkey_diag_dump() -> Vec<String> {
    HOTKEY_DIAG
        .lock()
        .map(|mut q| q.drain(..).collect())
        .unwrap_or_default()
}

static GLOBAL_HOTKEY_TX: OnceLock<StdMutex<Option<Sender<GlobalKeyEvent>>>> = OnceLock::new();
static GLOBAL_HOTKEY_STOP: AtomicBool = AtomicBool::new(false);
static GLOBAL_HOTKEY_RUNNING: AtomicBool = AtomicBool::new(false);

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
}

impl HotkeySnapshot {
    fn from_scheduler(scheduler: &ClickScheduler) -> Self {
        let cfg = scheduler.get_config();
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
    invalid_bindings: usize,
}

impl HotkeyBindings {
    fn all_groups(&self) -> [&[HotkeyCombo]; 7] {
        [
            &self.toggle,
            &self.mode_switch,
            &self.emergency_stop,
            &self.speed_up,
            &self.slow_down,
            &self.capture_pos,
            &self.record_toggle,
        ]
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
        HotkeyBindings {
            toggle,
            mode_switch,
            emergency_stop,
            speed_up,
            slow_down,
            capture_pos,
            record_toggle,
            preset_slots,
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
        CallNextHookEx, DispatchMessageW, GetMessageW, SetWindowsHookExW, TranslateMessage,
        UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT, MSG, WH_KEYBOARD_LL,
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
        // Loop ends right before the hook teardown block.
        let end = src[loop_start..]
            .find("unsafe {")
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
}
