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
