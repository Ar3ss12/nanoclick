//! Platform layer — re-exports Windows / Linux / macOS-specific code.
//!
//! The executor and recorder use these functions to dispatch input.
//! Reference: `docs/MACRO_ARCHITECTURE.md` §11.

pub mod backend;

pub use backend::PlatformCapabilities;

#[cfg(target_os = "windows")]
pub mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

/// v4.2 — capability-honest no-op backend for platforms without input
/// synthesis yet. Every call is a silent no-op; UI surfaces availability
/// via `PlatformCapabilities` instead of fake success.
#[cfg(not(target_os = "windows"))]
pub struct NoopInputBackend;

#[cfg(not(target_os = "windows"))]
impl Default for NoopInputBackend {
    fn default() -> Self {
        NoopInputBackend
    }
}

#[cfg(not(target_os = "windows"))]
impl backend::InputBackend for NoopInputBackend {
    fn mouse_click(&self, _b: crate::core::action::MouseButton) {}
    fn mouse_down(&self, _b: crate::core::action::MouseButton) {}
    fn mouse_up(&self, _b: crate::core::action::MouseButton) {}
    fn scroll_wheel(&self, _dx: i32, _dy: i32) {}
    fn set_cursor_pos(&self, _x: i32, _y: i32) {}
    fn send_key(
        &self,
        _k: crate::core::action::KeyCode,
        _m: crate::core::action::Modifiers,
        _up: bool,
    ) {
    }
    fn cursor_position(&self) -> (i32, i32) {
        (0, 0)
    }
    fn click_mouse(&self, _spec: &backend::ClickSpec) -> bool {
        false
    }
    fn release_mouse_hold(&self, _b: crate::core::action::MouseButton) {}
}

/// Platform-agnostic backend selection — the ONLY place that names the
/// concrete backend type (v4.2 completion criterion: core/scheduler/
/// commands/lib contain zero `platform::windows` references).
#[cfg(target_os = "windows")]
pub fn default_input_backend() -> std::sync::Arc<dyn backend::InputBackend> {
    std::sync::Arc::new(windows::WindowsBackend)
}

#[cfg(not(target_os = "windows"))]
pub fn default_input_backend() -> std::sync::Arc<dyn backend::InputBackend> {
    std::sync::Arc::new(NoopInputBackend)
}

/// Hotkey backend factory — scheduler + app handle wiring (Windows only;
/// other platforms return an error-reporting stub).
#[cfg(target_os = "windows")]
pub fn default_hotkey_backend(
    scheduler: std::sync::Arc<crate::scheduler::ClickScheduler>,
    app_handle: tauri::AppHandle,
) -> std::sync::Arc<dyn backend::HotkeyBackend> {
    std::sync::Arc::new(windows::WindowsHotkeyBackend::new(scheduler, app_handle))
}

#[cfg(not(target_os = "windows"))]
pub fn default_hotkey_backend(
    _scheduler: std::sync::Arc<crate::scheduler::ClickScheduler>,
    _app_handle: tauri::AppHandle,
) -> std::sync::Arc<dyn backend::HotkeyBackend> {
    std::sync::Arc::new(NoopHotkeyBackend)
}

#[cfg(not(target_os = "windows"))]
pub struct NoopHotkeyBackend;

#[cfg(not(target_os = "windows"))]
impl backend::HotkeyBackend for NoopHotkeyBackend {
    fn start(&self) -> Result<(), String> {
        Err("global hotkeys unavailable on this platform".into())
    }
    fn stop(&self) {}
    fn is_running(&self) -> bool {
        false
    }
}

/// Stop the global hotkey listener through the backend contract.
pub fn default_input_backend_hotkey_stop() {
    #[cfg(target_os = "windows")]
    {
        use backend::HotkeyBackend as _;
        windows::WindowsBackend::default().stop();
    }
    #[cfg(not(target_os = "windows"))]
    {
        // nothing to stop — no listener was started
    }
}

/// Recorder backend factory — label is parsed at the platform boundary.
#[cfg(target_os = "windows")]
pub fn default_recorder_backend(
    ignored_hotkey_label: &str,
) -> std::sync::Arc<dyn backend::RecorderBackend> {
    std::sync::Arc::new(windows::WindowsRecorderBackend::new(ignored_hotkey_label))
}

#[cfg(not(target_os = "windows"))]
pub fn default_recorder_backend(
    _ignored_hotkey_label: &str,
) -> std::sync::Arc<dyn backend::RecorderBackend> {
    std::sync::Arc::new(NoopRecorderBackend)
}

#[cfg(not(target_os = "windows"))]
pub struct NoopRecorderBackend;

#[cfg(not(target_os = "windows"))]
impl backend::RecorderBackend for NoopRecorderBackend {
    fn start(
        &self,
        _sender: std::sync::mpsc::Sender<crate::recorder::raw_event::RawEvent>,
    ) -> Result<(), String> {
        Err("global input recording unavailable on this platform".into())
    }
    fn stop(&self) {}
}

/// Stop the recorder capture through the backend contract.
pub fn recorder_backend_stop() {
    #[cfg(target_os = "windows")]
    {
        windows::stop_recorder_hooks();
    }
    #[cfg(not(target_os = "windows"))]
    {
        // nothing running
    }
}
