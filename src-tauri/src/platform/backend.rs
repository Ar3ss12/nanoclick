//! Platform contracts shared by the executor, recorder and hotkey layer.
//! Implementations are platform-specific; core code depends only on these
//! contracts and never on VK/X11/AppKit handles.
//!
//! v4.2: traits now match the real architecture. `InputBackend` covers
//! every primitive the macro executor dispatches. `HotkeyBackend` wraps
//! the global-listener lifecycle (hook + matcher loop with parse-once
//! snapshot rebinding), NOT per-binding register/unregister — the old
//! shape lied about how hotkeys actually work.

#![allow(dead_code)]

use crate::core::action::{KeyCode, Modifiers, MouseButton};
use std::sync::mpsc::Sender;

/// How a click is delivered (parsed once from config strings at the
/// boundary — see `config_manager`; the platform layer never sees raw
/// strings anymore, v4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickType {
    Single,
    Double,
    Hold,
}

impl ClickType {
    pub fn from_config_str(s: &str) -> Self {
        match s {
            "double" => ClickType::Double,
            "hold" => ClickType::Hold,
            _ => ClickType::Single,
        }
    }
}

/// Where the click lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionMode {
    /// Click at the current cursor position.
    Cursor,
    /// Click at a fixed screen coordinate.
    Fixed,
}

impl PositionMode {
    pub fn from_config_str(s: &str) -> Self {
        match s {
            "fixed" => PositionMode::Fixed,
            _ => PositionMode::Cursor,
        }
    }
}

/// Fully-typed click request. Replaces the old stringly-typed
/// `click_mouse_ext(&str button, &str click_type, ...)` contract.
#[derive(Debug, Clone, Copy)]
pub struct ClickSpec {
    pub button: MouseButton,
    pub click_type: ClickType,
    pub position_mode: PositionMode,
    pub fixed_x: i32,
    pub fixed_y: i32,
    pub jitter_radius: u32,
}

/// Every synthesized-input primitive the executor needs.
pub trait InputBackend: Send + Sync {
    fn mouse_click(&self, button: MouseButton);
    fn mouse_down(&self, button: MouseButton);
    fn mouse_up(&self, button: MouseButton);
    fn scroll_wheel(&self, delta_x: i32, delta_y: i32);
    fn set_cursor_pos(&self, x: i32, y: i32);
    /// Synthesize a key event. `key.0` is the platform-neutral virtual
    /// key code (VK_* on Windows); modifiers are applied explicitly.
    fn send_key(&self, key: KeyCode, mods: Modifiers, is_up: bool);
    fn cursor_position(&self) -> (i32, i32);
    /// Typed autoclicker entry point (was stringly `click_mouse_ext`).
    /// Returns false when the platform cannot deliver input.
    fn click_mouse(&self, spec: &ClickSpec) -> bool;
    /// Release a held mouse button (hold-click teardown).
    fn release_mouse_hold(&self, button: MouseButton);
}

/// Semantic action emitted by the global hotkey layer after a binding
/// matches. The platform layer never calls app logic directly; it only
/// reports *what* matched (v4.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyAction {
    StartStop,
    ModeSwitch,
    EmergencyStop,
    SpeedUp,
    SlowDown,
    CapturePosition,
    RecordToggle,
}

impl HotkeyAction {
    pub fn from_label(label: &str) -> Self {
        match label {
            "mode" => HotkeyAction::ModeSwitch,
            "emergency" => HotkeyAction::EmergencyStop,
            "speed_up" => HotkeyAction::SpeedUp,
            "slow_down" => HotkeyAction::SlowDown,
            "capture_pos" => HotkeyAction::CapturePosition,
            "record" => HotkeyAction::RecordToggle,
            _ => HotkeyAction::StartStop,
        }
    }
}

/// Global-hotkey lifecycle. The real Windows model is ONE low-level hook
/// plus a matcher loop that re-reads bindings when the scheduler bumps
/// its `hotkeys_version` (parse-once-per-change, v4.1). The contract is
/// therefore start/stop of that loop with an event stream out — NOT
/// per-binding register/unregister, which would lie about the design.
pub trait HotkeyBackend: Send + Sync {
    /// Spawn the global listener loop (idempotent; second call is a no-op).
    fn start(&self) -> Result<(), String>;
    /// Stop the loop and release its channel. Safe to call repeatedly.
    fn stop(&self);
    /// Whether the listener loop is currently alive.
    fn is_running(&self) -> bool;
}

/// Recorder capture lifecycle.
pub trait RecorderBackend: Send + Sync {
    fn start(&self, sender: Sender<crate::recorder::raw_event::RawEvent>) -> Result<(), String>;
    fn stop(&self);
}

/// What the current platform can actually do. UI surfaces availability
/// through this instead of platforms faking success with silent no-ops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct PlatformCapabilities {
    /// Global hotkey listener (LL-hook / X11 grab / AddEvent).
    pub global_hotkeys: bool,
    /// Global input capture for the recorder.
    pub global_input_recording: bool,
    /// Synthetic mouse events (clicks, moves, scroll).
    pub mouse_injection: bool,
    /// Synthetic keyboard events.
    pub keyboard_injection: bool,
}

impl PlatformCapabilities {
    #[cfg(target_os = "windows")]
    pub fn detect() -> Self {
        PlatformCapabilities {
            global_hotkeys: true,
            global_input_recording: true,
            mouse_injection: true,
            keyboard_injection: true,
        }
    }

    #[cfg(not(target_os = "windows"))]
    pub fn detect() -> Self {
        PlatformCapabilities {
            global_hotkeys: false,
            global_input_recording: false,
            mouse_injection: false,
            keyboard_injection: false,
        }
    }

    /// Whether macro playback can work at all on this platform.
    pub fn can_play_macros(&self) -> bool {
        self.mouse_injection && self.keyboard_injection
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    use crate::core::action::Modifiers;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    use std::sync::mpsc;

    /// Mock backend recording every dispatch — proves core code can run
    /// against ANY InputBackend implementation without Win32.
    #[derive(Default)]
    struct MockInputBackend {
        clicks: AtomicUsize,
        holds_released: AtomicUsize,
        last_spec: std::sync::Mutex<Option<(String, String)>>, // debug of button+mode
    }

    impl InputBackend for MockInputBackend {
        fn mouse_click(&self, _: MouseButton) {
            self.clicks.fetch_add(1, Ordering::Relaxed);
        }
        fn mouse_down(&self, _: MouseButton) {}
        fn mouse_up(&self, _: MouseButton) {}
        fn scroll_wheel(&self, _: i32, _: i32) {}
        fn set_cursor_pos(&self, _: i32, _: i32) {}
        fn send_key(&self, _: KeyCode, _: Modifiers, _: bool) {}
        fn cursor_position(&self) -> (i32, i32) { (0, 0) }
        fn click_mouse(&self, spec: &ClickSpec) -> bool {
            self.clicks.fetch_add(1, Ordering::Relaxed);
            *self.last_spec.lock().unwrap() = Some((
                format!("{:?}", spec.click_type),
                format!("{:?}", spec.position_mode),
            ));
            true
        }
        fn release_mouse_hold(&self, _: MouseButton) {
            self.holds_released.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Mock recorder capturing lifecycle calls.
    struct MockRecorderBackend {
        started: AtomicUsize,
        stopped: AtomicUsize,
    }

    impl Default for MockRecorderBackend {
        fn default() -> Self {
            Self { started: AtomicUsize::new(0), stopped: AtomicUsize::new(0) }
        }
    }

    impl RecorderBackend for MockRecorderBackend {
        fn start(
            &self,
            _sender: mpsc::Sender<crate::recorder::raw_event::RawEvent>,
        ) -> Result<(), String> {
            self.started.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        fn stop(&self) {
            self.stopped.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn click_type_parses_from_config_labels() {
        assert_eq!(ClickType::from_config_str("single"), ClickType::Single);
        assert_eq!(ClickType::from_config_str("double"), ClickType::Double);
        assert_eq!(ClickType::from_config_str("hold"), ClickType::Hold);
        // Unknown labels fall back to the safe default.
        assert_eq!(ClickType::from_config_str("???"), ClickType::Single);
    }

    #[test]
    fn position_mode_parses_from_config_labels() {
        assert_eq!(PositionMode::from_config_str("fixed"), PositionMode::Fixed);
        assert_eq!(PositionMode::from_config_str("cursor"), PositionMode::Cursor);
        assert_eq!(PositionMode::from_config_str("junk"), PositionMode::Cursor);
    }

    #[test]
    fn hotkey_action_maps_config_labels() {
        assert_eq!(HotkeyAction::from_label("mode"), HotkeyAction::ModeSwitch);
        assert_eq!(HotkeyAction::from_label("emergency"), HotkeyAction::EmergencyStop);
        // Unknown labels default to the primary start/stop action.
        assert_eq!(HotkeyAction::from_label("???"), HotkeyAction::StartStop);
    }

    #[test]
    fn mock_backend_receives_typed_click_specs() {
        let backend = MockInputBackend::default();
        let spec = ClickSpec {
            button: MouseButton::Right,
            click_type: ClickType::Hold,
            position_mode: PositionMode::Fixed,
            fixed_x: 100,
            fixed_y: 200,
            jitter_radius: 5,
        };
        assert!(backend.click_mouse(&spec));
        backend.release_mouse_hold(spec.button);
        assert_eq!(backend.clicks.load(Ordering::Relaxed), 1);
        assert_eq!(backend.holds_released.load(Ordering::Relaxed), 1);
        let captured = backend.last_spec.lock().unwrap().clone().unwrap();
        assert_eq!(captured, ("Hold".into(), "Fixed".into()));
    }

    #[test]
    fn mock_recorder_lifecycle_start_stop() {
        let backend = MockRecorderBackend::default();
        let (tx, _rx) = mpsc::channel();
        assert!(RecorderBackend::start(&backend, tx).is_ok());
        backend.stop();
        assert_eq!(backend.started.load(Ordering::Relaxed), 1);
        assert_eq!(backend.stopped.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn windows_capabilities_report_full_support() {
        let caps = PlatformCapabilities::detect();
        // Contract: on Windows everything must be available; on stub
        // platforms `detect()` reports all-false so UI can show it.
        if cfg!(target_os = "windows") {
            assert!(caps.can_play_macros());
            assert!(caps.global_hotkeys && caps.global_input_recording);
        } else {
            assert!(!caps.can_play_macros());
        }
    }
}
