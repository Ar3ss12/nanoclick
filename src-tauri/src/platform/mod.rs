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

/// One selectable application for the Smart Guard app filter.
///
/// `exe` is the process image name the click loop matches against
/// (`discord.exe`); `label` is what the picker shows the user.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AppEntry {
    pub label: String,
    /// `None` when an installed app exposes no usable executable path.
    pub exe: Option<String>,
    /// `"running"` or `"installed"` — the picker groups by this.
    pub source: &'static str,
}

/// Lowercase base file name of a path (`C:\A\B\Discord.exe` → `discord.exe`).
pub fn exe_name_from_path(path: &str) -> Option<String> {
    let trimmed = path.trim().trim_matches('"');
    if trimmed.is_empty() {
        return None;
    }
    let base = trimmed
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(trimmed)
        .trim()
        .trim_matches('"');
    if base.is_empty() {
        return None;
    }
    let lower = base.to_ascii_lowercase();
    // Keep only real image names — registry metadata can contain other junk.
    if lower.ends_with(".exe") {
        Some(lower)
    } else {
        None
    }
}

/// Pull the executable name out of an uninstall-entry `DisplayIcon` value.
///
/// Real-world shapes:
/// * `C:\Program Files\App\app.exe`          → `app.exe`
/// * `"C:\Program Files\App\app.exe",0`      → `app.exe`
/// * `C:\Program Files\App\app.exe,0`        → `app.exe`
/// * `@C:\Windows\Installer\{...}.dll,-101`  → `None` (no exe to match)
pub fn exe_from_display_icon(value: &str) -> Option<String> {
    let raw = value.trim();
    if raw.is_empty() || raw.starts_with('@') {
        return None;
    }
    // Strip a trailing `,<resource-id>` (quoted values keep the comma too).
    let without_index = match raw.rfind(',') {
        Some(idx) if raw[idx + 1..].trim().parse::<i32>().is_ok() => &raw[..idx],
        _ => raw,
    };
    exe_name_from_path(without_index)
}

/// Lowercase image name of THIS process (`NanoClick.exe` → `nanoclick.exe`).
///
/// Used by the Focus Guard to recognise our own window: returning to
/// NanoClick mid-run must never pause, while leaving it (even via Alt+Tab
/// from our own window) must stop. Resolved from `std::env::current_exe()`,
/// so dev (`nanoclick.exe`) and release (`NanoClick.exe`) builds agree.
pub fn own_exe_name() -> Option<String> {
    std::env::current_exe()
        .ok()
        .and_then(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|s| s.to_string())
        })
        .as_deref()
        .and_then(exe_name_from_path)
}

/// `true` when `name` is our own process image (case-insensitive).
pub fn is_own_exe(name: &str) -> bool {
    match own_exe_name() {
        Some(own) => name.eq_ignore_ascii_case(&own),
        None => false,
    }
}

#[cfg(not(target_os = "windows"))]
pub fn list_running_apps() -> Vec<AppEntry> { Vec::new() }
#[cfg(not(target_os = "windows"))]
pub fn list_installed_apps() -> Vec<AppEntry> { Vec::new() }

#[cfg(test)]
mod app_catalog_tests {
    use super::*;

    #[test]
    fn exe_name_is_lowercased_and_stripped_of_directories() {
        assert_eq!(
            exe_name_from_path(r"C:\Program Files\Discord\Discord.exe").as_deref(),
            Some("discord.exe")
        );
        assert_eq!(
            exe_name_from_path(r"C:/tools/JAVA/W.exe").as_deref(),
            Some("w.exe")
        );
        assert_eq!(exe_name_from_path("  chrome.exe  ").as_deref(), Some("chrome.exe"));
        assert_eq!(exe_name_from_path("").as_deref(), None);
        assert_eq!(exe_name_from_path("   ").as_deref(), None);
        // Non-executables must never reach the filter list.
        assert_eq!(exe_name_from_path(r"C:\App\readme.txt").as_deref(), None);
    }

    #[test]
    fn display_icon_shapes_resolve_to_the_launcher_exe() {
        assert_eq!(
            exe_from_display_icon(r"C:\Program Files\App\app.exe").as_deref(),
            Some("app.exe")
        );
        // Quoted with resource index — the shape most installers write.
        assert_eq!(
            exe_from_display_icon(r#""C:\Program Files\App\app.exe",0"#).as_deref(),
            Some("app.exe")
        );
        assert_eq!(
            exe_from_display_icon(r"C:\Games\Launcher\javaw.exe,0").as_deref(),
            Some("javaw.exe")
        );
        // Resource-only icons carry no executable to match against.
        assert_eq!(
            exe_from_display_icon(r"@C:\Windows\Installer\{A}.dll,-101"),
            None
        );
        assert_eq!(exe_from_display_icon(""), None);
    }

    #[test]
    fn scenario_prefix_is_not_treated_as_a_resource_index() {
        // A comma is only stripped when it is followed by an integer.
        assert_eq!(
            exe_from_display_icon(r#"C:\App, Inc\App.exe"#).as_deref(),
            Some("app.exe")
        );
    }

    #[test]
    fn own_exe_name_resolves_and_matches_itself() {
        // Must be a lowercase .exe derived from the running binary, and
        // is_own_exe must accept it case-insensitively but nothing else.
        let Some(own) = own_exe_name() else {
            return; // non-UTF8 exe path edge: nothing to assert
        };
        assert!(own.ends_with(".exe"), "own exe must be an image name, got {own:?}");
        assert_eq!(own, own.to_ascii_lowercase());
        assert!(is_own_exe(&own));
        assert!(is_own_exe(&own.to_ascii_uppercase()));
        assert!(!is_own_exe("game.exe"));
        assert!(!is_own_exe("explorer.exe"));
        assert!(!is_own_exe(""));
    }
}

#[cfg(not(target_os = "windows"))]
pub fn is_current_process_elevated() -> bool { false }
#[cfg(not(target_os = "windows"))]
pub fn get_foreground_window_title() -> Option<String> { None }
#[cfg(not(target_os = "windows"))]
pub fn get_foreground_process_name() -> Option<String> { None }
#[cfg(not(target_os = "windows"))]
pub fn is_target_point_elevated(_x: i32, _y: i32) -> bool { false }
#[cfg(not(target_os = "windows"))]
pub fn restart_as_admin() -> Result<(), String> { Err("Only supported on Windows".into()) }
#[cfg(not(target_os = "windows"))]
pub fn set_always_run_as_admin(_enabled: bool) -> Result<(), String> { Ok(()) }
#[cfg(not(target_os = "windows"))]
pub fn is_always_run_as_admin() -> bool { false }
#[cfg(not(target_os = "windows"))]
pub fn play_warning_sound() {}
#[cfg(not(target_os = "windows"))]
pub fn init_dpi_awareness() {}
/// No input layer to report on other platforms. Windows provides the real
/// ring buffer (`windows::hotkey_diag_dump`), re-exported through
/// `pub use windows::*` above; this stub keeps the caller platform-agnostic.
#[cfg(not(target_os = "windows"))]
pub fn hotkey_diag_dump() -> Vec<String> {
    Vec::new()
}
