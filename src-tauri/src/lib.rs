mod commands;
mod config;
mod config_manager;
mod core;
pub mod defaults;
mod guard;
mod persistence;
mod platform;
mod recorder;
mod scheduler;
mod overlay;
mod watcher;

use config_manager::{AppConfig, ConfigManager};
use scheduler::ClickScheduler;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State};
use tauri_plugin_updater::UpdaterExt;

static DEBUG_LOG_FILE: OnceLock<Mutex<Option<File>>> = OnceLock::new();
static DEBUG_LOG_BYTES: AtomicU64 = AtomicU64::new(0);
const DEBUG_LOG_MAX_BYTES: u64 = 2 * 1024 * 1024;
static DEBUG_MODE: AtomicBool = AtomicBool::new(true);

pub(crate) fn is_debug_mode() -> bool {
    DEBUG_MODE.load(Ordering::Relaxed)
}

pub(crate) fn set_debug_mode_enabled(enabled: bool) {
    DEBUG_MODE.store(enabled, Ordering::Relaxed);
}

pub struct AppState {
    pub scheduler: Arc<ClickScheduler>,
    pub config_manager: Arc<ConfigManager>,
    /// FIFO queue of mandatory-acknowledge boot notices (Deadbolt Modal).
    /// Drained one-by-one by `get_startup_notices`; the UI stays bolted
    /// until every notice is dismissed with OK. Never blocks boot itself.
    pub startup_notices: Mutex<Vec<AppNotice>>,
    /// Non-blocking runtime toast queue fed by the observer watcher.
    /// Drained by `poll_file_toasts` (auto-dismiss in UI, no OK bolt).
    pub file_toasts: Mutex<Vec<AppNotice>>,
}

/// Shared observer handle so save-commands can mark their own writes.
pub struct WatcherState(pub Arc<crate::watcher::Observer>);

/// Granular notice level for the Deadbolt Modal.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NoticeLevel {
    Info,
    Warning,
    Critical,
}

/// Blocking-modal payload: level + title + message + optional path details.
/// Frontend rules (vanilla JS, no libs): no [X], backdrop clicks are swallowed,
/// Esc is killed, the ONLY exit is the OK button. Multiple notices are shown
/// strictly in order until the queue is empty.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AppNotice {
    pub level: NoticeLevel,
    pub title: String,
    pub message: String,
    pub details: Option<String>,
}

fn healing_notice(action: &crate::defaults::SelfHealingAction) -> Option<AppNotice> {
    use crate::defaults::SelfHealingAction as A;
    match action {
        A::RepairedAndPatched { backup_path, details } => Some(AppNotice {
            level: NoticeLevel::Warning,
            title: "notice_cfg_repaired_title".into(),
            message: format!(
                "notice_cfg_repaired_msg|{}",
                details.len()
            ),
            details: Some(backup_path.display().to_string()),
        }),
        A::CorruptedAndRecovered { backup_path, restored_from_last_good } => {
            let message = if *restored_from_last_good {
                "notice_cfg_lkg_msg".into()
            } else {
                "notice_cfg_factory_msg".into()
            };
            Some(AppNotice {
                level: NoticeLevel::Critical,
                title: "notice_cfg_corrupted_title".into(),
                message,
                details: Some(backup_path.display().to_string()),
            })
        }
        A::Migrated => Some(AppNotice {
            level: NoticeLevel::Info,
            title: "notice_cfg_migrated_title".into(),
            message: "notice_cfg_migrated_msg".into(),
            details: None,
        }),
        A::LoadedExisting | A::CreatedFresh => None,
    }
}

/// Macros-store healing mapped into the same Deadbolt queue shape.
/// Titles/messages are i18n KEYS resolved by the frontend (see locales).
fn macros_healing_notice(
    action: &crate::persistence::macros::MacrosHealAction,
) -> Option<AppNotice> {
    use crate::persistence::macros::MacrosHealAction as M;
    match action {
        M::RecoveredFromLastGood { backup_path } => Some(AppNotice {
            level: NoticeLevel::Critical,
            title: "notice_macros_lkg_title".into(),
            message: "notice_macros_lkg_msg".into(),
            details: Some(backup_path.display().to_string()),
        }),
        M::ResetEmpty { backup_path } => Some(AppNotice {
            level: NoticeLevel::Critical,
            title: "notice_macros_empty_title".into(),
            message: "notice_macros_empty_msg".into(),
            details: Some(backup_path.display().to_string()),
        }),
        M::LoadedExisting | M::CreatedMissing => None,
    }
}

/// Runtime watcher verdict (observer only, never rewrites files).
/// Titles/messages are i18n KEYS resolved by the frontend.
fn file_health_notice(
    file: &str,
    verdict: &crate::watcher::FileHealth,
) -> Option<AppNotice> {
    use crate::watcher::FileHealth as H;
    match verdict {
        H::ChangedValid => Some(AppNotice {
            level: NoticeLevel::Info,
            title: "notice_watch_changed_title".into(),
            message: format!("notice_watch_changed_msg|{file}"),
            details: None,
        }),
        H::ChangedInvalid => Some(AppNotice {
            level: NoticeLevel::Warning,
            title: "notice_watch_invalid_title".into(),
            message: format!("notice_watch_invalid_msg|{file}"),
            details: None,
        }),
        H::Unchanged | H::IgnoredOwnWrite | H::Missing => None,
    }
}

/// Drain ALL pending boot notices as an ordered queue (empty vec = all clear).
/// Frontend shows them one-by-one; each requires its own OK click.
/// Titles/messages are i18n KEYS — the frontend resolves them via I18nEngine;
/// `|` suffix carries a param (count or file name), details carry the path.
#[tauri::command]
fn get_startup_notices(state: State<'_, AppState>) -> Vec<AppNotice> {
    state.startup_notices.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
}

/// Non-blocking runtime toast queue (watcher verdicts). Unlike the boot
/// Deadbolt queue, these auto-dismiss: observer info, never file rewrites.
#[tauri::command]
fn poll_file_toasts(state: State<'_, AppState>) -> Vec<AppNotice> {
    state.file_toasts.lock().map(|mut q| std::mem::take(&mut *q)).unwrap_or_default()
}

#[tauri::command]
fn get_app_config(state: State<'_, AppState>) -> AppConfig {
    let app_cfg = state.config_manager.load();
    state
        .scheduler
        .set_config(config::Config::from(app_cfg.clone()));
    app_cfg
}

#[tauri::command]
fn save_app_config(config: AppConfig, state: State<'_, AppState>, app: AppHandle) -> Result<AppConfig, String> {
    let _ = crate::platform::set_always_run_as_admin(config.ui.always_run_as_admin);
    // Keep persisted UI prefs and the lazy WebViews in sync: toggling ripple/HUD
    // in Settings must create/destroy the WebView on demand, not just flip a flag.
    let prev = state.config_manager.load();
    let mut config = config;
    // Remember window position: capture outer_position() into the SAME atomic
    // save (no extra file, no plugin). Best-effort: window may be absent in
    // tests, remember may be OFF. Never fails the save.
    if config.ui.remember_window_position {
        if let Some(win) = app.get_webview_window("main") {
            if let Ok(pos) = win.outer_position() {
                config.ui.window_x = Some(pos.x);
                config.ui.window_y = Some(pos.y);
            }
        }
    } else {
        // Remember OFF: don't hoard stale coordinates.
        config.ui.window_x = None;
        config.ui.window_y = None;
    }
    state.config_manager.save(&config)?;
    // Swallow OUR OWN echo so the observer never toasts our save.
    if let Some(w) = app.try_state::<crate::WatcherState>() {
        w.0.mark_own_write("config");
    }
    state
        .scheduler
        .set_config(config::Config::from(config.clone()));
    if config.ui.visual_ripple != prev.ui.visual_ripple {
        let _ = overlay::toggle_overlay(app.clone(), config.ui.visual_ripple);
    }
    if config.ui.show_hud != prev.ui.show_hud {
        let _ = toggle_hud_window(app.clone(), config.ui.show_hud);
    }
    Ok(config)
}

#[tauri::command]
fn toggle_mode(app_handle: AppHandle, state: State<'_, AppState>) -> String {
    let new_mode = state.scheduler.toggle_mode(Some(&app_handle));
    let mut app_cfg = state.config_manager.load();
    app_cfg.active_mode = new_mode.clone();
    let _ = state.config_manager.save(&app_cfg);
    new_mode
}

#[tauri::command]
fn complete_onboarding(state: State<'_, AppState>) -> Result<AppConfig, String> {
    let mut app_cfg = state.config_manager.load();
    app_cfg.first_run = false;
    state.config_manager.save(&app_cfg)?;
    Ok(app_cfg)
}

#[tauri::command]
fn reset_config_to_defaults(state: State<'_, AppState>, app: AppHandle) -> Result<AppConfig, String> {
    let prev = state.config_manager.load();
    let app_cfg = state.config_manager.reset_to_defaults()?;
    state
        .scheduler
        .set_config(config::Config::from(app_cfg.clone()));
    // Same lazy-WebView sync as save_app_config: defaults flip visual_ripple
    // (true) / show_hud (false), so create/destroy on demand.
    if app_cfg.ui.visual_ripple != prev.ui.visual_ripple {
        let _ = overlay::toggle_overlay(app.clone(), app_cfg.ui.visual_ripple);
    }
    if app_cfg.ui.show_hud != prev.ui.show_hud {
        let _ = toggle_hud_window(app.clone(), app_cfg.ui.show_hud);
    }
    Ok(app_cfg)
}

#[tauri::command]
fn get_config_path(state: State<'_, AppState>) -> String {
    state.config_manager.get_config_path()
}

#[tauri::command]
fn get_current_mouse_pos() -> (i32, i32) {
    platform::get_cursor_pos()
}

#[tauri::command]
fn toggle_autoclicker(app_handle: AppHandle, state: State<'_, AppState>) -> bool {
    let now_active = !state.scheduler.is_active();
    state.scheduler.set_active(now_active, Some(&app_handle));
    now_active
}

#[tauri::command]
fn get_status(state: State<'_, AppState>) -> scheduler::StatusUpdate {
    let is_auto = state.scheduler.is_autoclicker_mode();
    let is_act = state.scheduler.is_active();
    scheduler::StatusUpdate {
        active: is_act,
        mode: if is_auto {
            "autoclicker".into()
        } else {
            "work".into()
        },
        clicks_done: state.scheduler.get_clicks_done(),
        cps: state.scheduler.get_config().cps,
        status_text: if !is_auto {
            "WORK MODE (PAUSED)".into()
        } else if is_act {
            "RUNNING".into()
        } else {
            "IDLE".into()
        },
    }
}

#[tauri::command]
fn open_config_folder(state: State<'_, AppState>) -> Result<(), String> {
    let path_str = state.config_manager.get_config_path();
    let path = std::path::Path::new(&path_str);
    if let Some(parent) = path.parent() {
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("explorer").arg(parent).spawn();
        }
    }
    Ok(())
}

#[tauri::command]
fn open_external_url(url: String) -> Result<(), String> {
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return Err("Invalid URL protocol".into());
    }
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", &url])
            .spawn();
    }
    Ok(())
}

#[tauri::command]
fn set_windows_autostart(enable: bool) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use winreg::enums::*;
        use winreg::RegKey;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let path = r"Software\Microsoft\Windows\CurrentVersion\Run";
        if let Ok(key) = hkcu.open_subkey_with_flags(path, KEY_WRITE) {
            if enable {
                if let Ok(exe_path) = std::env::current_exe() {
                    let _ = key.set_value("NanoClick", &exe_path.to_string_lossy().as_ref());
                }
            } else {
                let _ = key.delete_value("NanoClick");
            }
        }
    }
    Ok(())
}

#[derive(serde::Deserialize)]
struct LogItem {
    level: String,
    message: String,
}

fn write_log_bytes_internal(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let path = std::env::temp_dir().join("nanoclick_web.log");
    let should_rotate = DEBUG_LOG_BYTES.fetch_add(bytes.len() as u64, Ordering::Relaxed)
        + bytes.len() as u64
        > DEBUG_LOG_MAX_BYTES;
    if should_rotate {
        if let Some(lock) = DEBUG_LOG_FILE.get() {
            if let Ok(mut guard) = lock.lock() {
                *guard = None;
            }
        }
        let backup = path.with_extension("log.1");
        let _ = std::fs::remove_file(&backup);
        let _ = std::fs::rename(&path, &backup);
        DEBUG_LOG_BYTES.store(bytes.len() as u64, Ordering::Relaxed);
    }
    if let Ok(mut guard) = DEBUG_LOG_FILE
        .get_or_init(|| {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map(|file| Mutex::new(Some(file)))
                .unwrap_or_else(|_| Mutex::new(None))
        })
        .lock()
    {
        if let Some(file) = guard.as_mut() {
            let _ = file.write_all(bytes);
        }
    }
}

#[tauri::command]
fn debug_log(level: String, message: String) {
    debug_log_internal(&level, &message);
}

#[tauri::command]
fn debug_log_batch(logs: Vec<LogItem>) {
    if !is_debug_mode() {
        return;
    }
    let mut batch_text = String::with_capacity(logs.len() * 128);
    for item in logs {
        if item.level != "error" && item.level != "warn" && !is_debug_mode() {
            continue;
        }
        let prefix = match item.level.as_str() {
            "error" => "[RUST ERROR]",
            "warn" => "[RUST WARN]",
            "stage-ok" => "[RUST STAGE✓]",
            "stage-fail" => "[RUST STAGE✗]",
            _ => "[RUST INFO]",
        };
        batch_text.push_str(prefix);
        batch_text.push(' ');
        batch_text.push_str(&item.message);
        batch_text.push('\n');
    }
    write_log_bytes_internal(batch_text.as_bytes());
}

#[tauri::command]
fn set_debug_mode(enabled: bool) {
    set_debug_mode_enabled(enabled);
}

#[tauri::command]
fn get_debug_mode() -> bool {
    is_debug_mode()
}

#[tauri::command]
fn relaunch_app(app: AppHandle) {
    app.restart();
}

#[tauri::command]
fn exit_app(app: AppHandle) {
    shutdown_application(&app);
}

pub(crate) fn shutdown_application(app: &AppHandle) {
    debug_log_internal("info", "[Shutdown] Initiating clean application shutdown");

    // 1. Hide main window immediately so UI feels instantaneous
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.hide();
    }

    // 2. Destroy secondary windows so their WebView2 processes terminate
    if let Some(overlay) = app.get_webview_window("overlay") {
        let _ = overlay.destroy();
    }
    if let Some(hud) = app.get_webview_window("hud") {
        let _ = hud.destroy();
    }

    // 3. Stop scheduler clicking loop
    if let Some(state) = app.try_state::<AppState>() {
        state.scheduler.set_active(false, None);
    }

    // 4. Stop native input hooks, recorder, and macro executor
    platform::default_input_backend_hotkey_stop();
    platform::stop_recorder_hooks();
    if let Some(exec) = crate::core::global() {
        exec.stop();
    }

    // 5. Request Tauri runtime exit
    app.exit(0);

    // 6. Watchdog: give 300ms for clean background file I/O flush, then trim working set
    // memory and guarantee complete process termination so no zombie background processes remain.
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(300));
        #[cfg(target_os = "windows")]
        unsafe {
            use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
            let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
        }
        std::process::exit(0);
    });
}

#[tauri::command]
fn toggle_hud_window(app: AppHandle, show: bool) -> Result<(), String> {
    if show {
        let already_existed = app.get_webview_window("hud").is_some();
        ensure_hud_window(&app)?;
        if let Some(win) = app.get_webview_window("hud") {
            let _ = win.set_ignore_cursor_events(true);
            // Fresh WebView: DON'T show yet — hud_ready() shows it once the DOM
            // has rendered (avoids DWM white flash). Existing: show now.
            if already_existed {
                let _ = win.show();
            }
            debug_log_internal("info", "[HUD] hud window shown (lazy)");
        }
    } else if let Some(win) = app.get_webview_window("hud") {
        let _ = win.destroy();
        debug_log_internal("info", "[HUD] hud window destroyed, WebView memory released");
    }
    Ok(())
}

/// Lazily create the floating HUD WebView (hud.html).
/// Idempotent: returns the existing window when already created.
fn ensure_hud_window(app: &AppHandle) -> Result<tauri::WebviewWindow, String> {
    if let Some(win) = app.get_webview_window("hud") {
        return Ok(win);
    }
    let win = tauri::WebviewWindowBuilder::new(
        app,
        "hud",
        tauri::WebviewUrl::App("hud.html".into()),
    )
    .title("NanoClick HUD")
    .transparent(true)
    .inner_size(140.0, 40.0)
    .always_on_top(true)
    .decorations(false)
    .shadow(false)
    .skip_taskbar(true)
    .resizable(false)
    .focused(false)
    .visible(false)
    .build()
    .map_err(|e| format!("hud create failed: {e}"))?;
    {
        use tauri::PhysicalPosition;
        let _ = win.set_position(PhysicalPosition::new(60, 60));
    }
    let _ = win.set_ignore_cursor_events(true);
    debug_log_internal("info", "[HUD] lazy-created on demand");
    Ok(win)
}

#[tauri::command]
fn hud_ready(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let cfg = state.config_manager.load();
    if let Some(win) = app.get_webview_window("hud") {
        let _ = win.set_ignore_cursor_events(true);
        if cfg.ui.show_hud {
            let _ = win.show();
            debug_log_internal("info", "[HUD] hud_ready: restored HUD visibility from config");
        }
    }
    Ok(())
}
#[derive(serde::Serialize)]
struct UpdateInfo {
    version: String,
    date: Option<String>,
    body: Option<String>,
}

#[tauri::command]
async fn check_for_updates(app: AppHandle) -> Result<Option<UpdateInfo>, String> {
    let update = app
        .updater()
        .map_err(|e| format!("updater unavailable: {}", e))?
        .check()
        .await
        .map_err(|e| format!("update check failed: {}", e))?;
    Ok(update.map(|u| UpdateInfo {
        version: u.version,
        date: u.date.map(|date| date.to_string()),
        body: u.body,
    }))
}

/// Internal helper used by both the JS-facing command and Rust-side logging
/// (hotkey listener, scheduler, etc). Writes to the same file as the JS log
/// so stage-by-stage diagnostics live in one place.
pub(crate) fn debug_log_internal(level: &str, message: &str) {
    if !is_debug_mode() && level != "error" && level != "warn" && level != "stage-fail" {
        return;
    }
    let prefix = match level {
        "error" => "[RUST ERROR]",
        "warn" => "[RUST WARN]",
        "stage-ok" => "[RUST STAGE✓]",
        "stage-fail" => "[RUST STAGE✗]",
        _ => "[RUST INFO]",
    };
    let line = format!("{} {}\n", prefix, message);
    write_log_bytes_internal(line.as_bytes());
}

/// Stage-based macro for Rust-side multi-step operations.
/// Mirrors the JS `stage()` helper — each `run` is a numbered step.
#[macro_export]
macro_rules! stage {
    ($name:expr => { $($body:tt)* }) => {{
        let op = $crate::debug_log_internal("stage-ok", &format!("[{}] starting", $name));
        let _ = op;
        $crate::debug_log_internal("stage-ok", &format!("[{}] all stages passed", $name));
        { $($body)* }
    }};
}

pub fn run() {
    crate::platform::init_dpi_awareness();

    // NOTE: no WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS here on purpose.
    // In-process GPU mode merges Chromium's GPU into OUR process: a fullscreen
    // DirectX exclusive / driver reset then hangs the Win32 thread that also
    // owns the click loop + WH_KEYBOARD_LL hook (see ZERO_JITTER_ISOLATION_PLAN).
    // Keep the GPU in its own msedgewebview2.exe process; secondary WebViews
    // (overlay/hud) are lazy-created on demand instead (cold boot = main only).

    let config_manager = Arc::new(ConfigManager::new());
    // Boot-time heal: capture the action for the one-shot startup modal.
    // `load()` runs ensure_config_file() internally (snapshot/quarantine/restore).
    let (initial_app_cfg, boot_action) = config_manager.load_with_action();
    let mut boot_notices: Vec<AppNotice> = healing_notice(&boot_action).into_iter().collect();
    // Macros store heals here too (same boot, ordered right after config).
    // Seeded ONCE: later saves refresh the snapshot directly (no boot queue).
    let (_, macros_boot_action) = crate::persistence::macros::load_macros_healed();
    if let Some(n) = macros_healing_notice(&macros_boot_action) {
        boot_notices.push(n);
    }
    let startup_notices = Mutex::new(boot_notices);
    let file_toasts = Mutex::new(Vec::<AppNotice>::new());
    let watcher = Arc::new(crate::watcher::Observer::new());
    // Register BEFORE any save can happen: config + macros, observer only.
    watcher.watch(
        "config",
        config_manager.config_path(),
        crate::watcher::config_bytes_valid,
    );
    watcher.watch(
        "macros",
        crate::persistence::macros::macros_path(),
        crate::watcher::macros_bytes_valid,
    );
    let watcher_for_setup = Arc::clone(&watcher);

    let scheduler = Arc::new(ClickScheduler::new());
    scheduler.set_config(config::Config::from(initial_app_cfg.clone()));

    let scheduler_for_setup = Arc::clone(&scheduler);
    let config_manager_arc = Arc::clone(&config_manager);

    let app_state = AppState {
        scheduler,
        config_manager,
        startup_notices,
        file_toasts,
    };

    let macro_state = commands::MacroState::default();
    let macro_executor: Arc<crate::core::ExecutorHandle> = Arc::clone(&macro_state.executor);
    crate::core::set_global((*macro_executor).clone());

    let app = tauri::Builder::default()
        .on_page_load(|webview, payload| {
            debug_log_internal(
                "info",
                &format!(
                    "[PageLoad] label={} event={:?} url={}",
                    webview.label(),
                    payload.event(),
                    webview.url().map(|u| u.to_string()).unwrap_or_else(|_| "(error)".into())
                ),
            );
        })
        // Must be registered first so a second launch is forwarded to the
        // existing process before any hooks or workers are started.
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            debug_log_internal(
                "info",
                &format!("[Lifecycle] second instance ignored: argv={argv:?} cwd={cwd}"),
            );
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(app_state)
        .manage(macro_state)
        .manage(WatcherState(watcher_for_setup.clone()))
        .setup(move |app| {
            let handle = app.handle().clone();
            crate::platform::set_global_app_handle(handle.clone());

            // **v1.0 memory-pressure workaround**: the auto-created window
            // from `tauri.conf.json` spawned 5 WebView2 child processes that
            // collectively consumed ~250 MB and tripped an "Out of Memory"
            // Chromium error page before `index.html` could finish loading.
            // The actual fix is in `src/main.js` (disable verbose debug
            // logging and wrap each init step in try/catch), so the window
            // is built here by Tauri itself using the conf entry above.
            //

            // Log WebView2 startup state for diagnostics: which data folder
            // the WebView chose, which window labels are registered, and
            // whether the main window is actually visible right after init.
            {
                if let Some(win) = app.get_webview_window("main") {
                    let url = win.url().map(|u| u.to_string()).unwrap_or_else(|_| "(error)".into());
                    let title = win.title().unwrap_or_default();
                    let pos = win.outer_position().unwrap_or_default();
                    let size = win.outer_size().unwrap_or_default();
                    debug_log_internal(
                        "info",
                        &format!(
                            "[Startup] main window registered url={} title={} pos=({},{}) size=({},{})",
                            url, title, pos.x, pos.y, size.width, size.height
                        ),
                    );
                    // Remember-window-position restore (BEHAVIOR card).
                    // Sanitizer: only inside the CURRENT virtual screen —
                    // a disconnected 2nd monitor (x=2560 on a 1920 screen)
                    // must fall back to center, never off-screen.
                    // Best-effort: all failures -> center (or conf default).
                    if initial_app_cfg.ui.remember_window_position {
                        if let (Some(x), Some(y)) =
                            (initial_app_cfg.ui.window_x, initial_app_cfg.ui.window_y)
                        {
                            let (vw, vh) = platform::get_screen_size();
                            if x >= 0 && y >= 0 && x < vw && y < vh {
                                use tauri::PhysicalPosition;
                                // A live-window move can make WebView2 reload the
                                // page; the page's beforeunload handler must not
                                // treat that as "user closed the app".
                                let _ = win.eval("window.__NANOCLICK_RELOADING__ = true;");
                                let _ = win.set_position(PhysicalPosition::new(x, y));
                                let _ = win.eval("window.__NANOCLICK_RELOADING__ = false;");
                                debug_log_internal(
                                    "info",
                                    &format!("[Startup] restored window position to ({x},{y})"),
                                );
                            } else {
                                let _ = win.center();
                                debug_log_internal(
                                    "warn",
                                    &format!("[Startup] saved pos ({x},{y}) outside {vw}x{vh}; centered"),
                                );
                            }
                        }
                    }
                    // Force the main window into the foreground at startup.
                    // Without this, the OS sometimes leaves it behind other
                    // apps that were active when we launched (especially
                    // single-instance scenarios where we are the second proc).
                    let _ = win.unminimize();
                    let _ = win.show();
                    let _ = win.set_focus();
                } else {
                    debug_log_internal("error", "[Startup] main window MISSING from registry");
                }
            }

            // ── App-profile auto-switch thread ────────────────────────
            // Every 500 ms: read the foreground window title; when it matches
            // an enabled app_profile rule whose preset differs from the last
            // applied one, emit 'app-profile-activate' with the preset id.
            {
                let cm = config_manager_arc.clone();
                let h = handle.clone();
                std::thread::spawn(move || {
                    let mut last_preset: Option<String> = None;
                    loop {
                        std::thread::sleep(std::time::Duration::from_millis(1500));
                        let profiles = {
                            let cfg = cm.load();
                            if cfg.app_profiles.is_empty() {
                                continue;
                            }
                            cfg.app_profiles
                                .into_iter()
                                .filter(|p| p.enabled && !p.title_contains.is_empty())
                                .collect::<Vec<_>>()
                        };
                        let Some(title) = platform::get_foreground_window_title() else {
                            continue;
                        };
                        let lower = title.to_lowercase();
                        for p in profiles {
                            if lower.contains(&p.title_contains.to_lowercase()) {
                                if last_preset.as_deref() != Some(p.preset_id.as_str()) {
                                    last_preset = Some(p.preset_id.clone());
                                    let _ = h.emit("app-profile-activate", p.preset_id);
                                }
                                break;
                            }
                        }
                    }
                });
            }
            // v4.2 — start hotkeys through the HotkeyBackend contract.
            let hk = platform::default_hotkey_backend(scheduler_for_setup, handle.clone());
            if let Err(e) = hk.start() {
                crate::debug_log_internal("warn", &format!("[Hotkeys] backend start failed: {e}"));
            }
            // LAZY secondary WebViews: cold boot = main window ONLY.
            // Overlay is restored here (staggered, off the critical boot path)
            // so persisted visual_ripple=true keeps working without any
            // pre-created WebView in tauri.conf.json. HUD restores itself:
            // main.js re-invokes toggle_hud_window ~150 ms after main load.
            // overlay_ready/hud_ready show the window once transparent DOM renders.
            if initial_app_cfg.ui.visual_ripple {
                let h = handle.clone();
                std::thread::spawn(move || {
                    // Let the main window finish loading first: creating two
                    // WebViews at the exact same instant doubles the startup
                    // memory spike we are trying to avoid.
                    std::thread::sleep(std::time::Duration::from_millis(2500));
                    let _ = overlay::ensure_overlay_window(&h);
                });
            }
            // ── Observer watcher thread (eyes only, hands off) ──────────
            // 1s poll, 750ms debounce, own-write grace: external edits in
            // Notepad surface as toasts, never as file rewrites. Healing is
            // boot-time only — no war with the editor mid-keystroke.
            // Heavy parse runs on THIS thread; toast push is a short lock.
            {
                let w = watcher_for_setup.clone();
                let h = handle.clone();
                std::thread::spawn(move || {
                    // Skip the boot storm (lazy overlay/HUD + first saves).
                    std::thread::sleep(std::time::Duration::from_millis(5000));
                    loop {
                        std::thread::sleep(std::time::Duration::from_millis(
                            crate::watcher::POLL_INTERVAL_MS,
                        ));
                        for (key, health) in w.poll_all() {
                            if let Some(n) = crate::file_health_notice(&key, &health) {
                                let push_ok = h.try_state::<crate::AppState>().map(|s| {
                                    if let Ok(mut q) = s.file_toasts.lock() {
                                        // Cap: drop oldest, keep the queue bounded.
                                        if q.len() >= 8 {
                                            q.remove(0);
                                        }
                                        q.push(n);
                                    }
                                });
                                let _ = push_ok;
                                crate::debug_log_internal(
                                    "info",
                                    &format!("[Watcher] {key} -> {health:?} (toast queued)"),
                                );
                            }
                        }
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_app_config,
            get_startup_notices,
            poll_file_toasts,
            save_app_config,
            toggle_mode,
            complete_onboarding,
            reset_config_to_defaults,
            get_config_path,
            get_current_mouse_pos,
            commands::get_platform_capabilities,
            commands::get_app_version,
            toggle_autoclicker,
            get_status,
            open_config_folder,
            open_external_url,
            set_windows_autostart,
            toggle_hud_window,
            commands::export_full_backup,
            commands::import_full_backup,
            commands::set_image_trigger,
            commands::pick_screen_pixel,
            commands::get_cursor_pos_now,
            commands::get_primary_screen_size,
            // v3.2 Macro Engine commands
            commands::list_macros,
            commands::save_macro,
            commands::delete_macro,
            commands::record_start,
            commands::record_stop,
            commands::record_cancel,
            commands::play_macro,
            commands::play_macro_from,
            commands::step_macro,
            commands::rewind_macro,
            commands::stop_macro,
            commands::is_macro_running,
            debug_log,
            debug_log_batch,
            set_debug_mode,
            get_debug_mode,
            check_for_updates,
            relaunch_app,
            commands::check_elevation,
            commands::restart_as_admin,
            commands::set_always_run_as_admin,
            commands::get_always_run_as_admin,
            commands::capture_foreground_app,
            commands::get_smart_guard_defaults,
            commands::get_smart_guard_status,
            commands::list_installed_and_running_apps,
            overlay::overlay_ready,
            overlay::toggle_overlay,
            exit_app,
            hud_ready,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        match event {
            RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::CloseRequested { .. },
                ..
            } => {
                if label == "main" {
                    shutdown_application(&app_handle);
                }
            }
            RunEvent::Exit => {
                // Stop native hooks before the process exits so no callback can
                // outlive its channel or application state.
                // v4.2 — stop via the HotkeyBackend contract.
                platform::default_input_backend_hotkey_stop();
                platform::stop_recorder_hooks();
                if let Some(exec) = crate::core::global() {
                    exec.stop();
                }
            }
            _ => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    static TEST_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_debug_mode_toggle() {
        let _guard = TEST_MUTEX.lock().unwrap();
        set_debug_mode(true);
        assert!(get_debug_mode());
        set_debug_mode(false);
        assert!(!get_debug_mode());
    }

    #[test]
    fn test_debug_log_batch_execution() {
        let _guard = TEST_MUTEX.lock().unwrap();
        set_debug_mode(true);
        let batch = vec![
            LogItem {
                level: "info".into(),
                message: "Test log batch item 1".into(),
            },
            LogItem {
                level: "error".into(),
                message: "Test log batch item 2".into(),
            },
        ];
        debug_log_batch(batch);
        assert!(get_debug_mode());
    }
}
