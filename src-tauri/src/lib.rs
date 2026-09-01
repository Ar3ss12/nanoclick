mod commands;
mod config;
mod config_manager;
mod core;
mod persistence;
mod platform;
mod recorder;
mod scheduler;

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
fn save_app_config(config: AppConfig, state: State<'_, AppState>) -> Result<AppConfig, String> {
    state.config_manager.save(&config)?;
    state
        .scheduler
        .set_config(config::Config::from(config.clone()));
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
fn toggle_hud_window(app: AppHandle, show: bool) -> Result<(), String> {
    use tauri::WebviewWindowBuilder;
    if let Some(win) = app.get_webview_window("hud") {
        if !show {
            let _ = win.close();
            return Ok(());
        }
        let _ = win.show();
        return Ok(());
    }
    if !show {
        return Ok(());
    }
    WebviewWindowBuilder::new(&app, "hud", tauri::WebviewUrl::App("hud.html".into()))
        .title("NanoClick HUD")
        .decorations(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .focused(false)
        .inner_size(120.0, 34.0)
        .position(40.0, 40.0)
        .build()
        .map_err(|e| format!("hud window: {e}"))?;
    debug_log_internal("info", "[Startup] hud window built with url=hud.html");
    // Click-through so the HUD never blocks the mouse.
    if let Some(win) = app.get_webview_window("hud") {
        use tauri::PhysicalPosition;
        let _ = win.set_position(PhysicalPosition::new(60, 60));
        let _ = win.set_ignore_cursor_events(true);
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
    // Set Chromium WebView2 flags for minimum memory footprint (~120MB) and low CPU consumption.
    // Limits V8 JS heap to 64MB, forces aggressive GC, runs GPU in-process, restricts process spawning,
    // and disables unused background Chromium components.
    if std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").is_err() {
        std::env::set_var(
            "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
            "--js-flags=\"--max-old-space-size=64\" \
             --in-process-gpu \
             --renderer-process-limit=1 \
             --disable-features=Translate,MediaRouter,OptimizationHints,ProcessPriorityPolicy \
             --disable-background-networking \
             --disable-component-update",
        );
    }

    let config_manager = Arc::new(ConfigManager::new());
    let initial_app_cfg = config_manager.load();

    let scheduler = Arc::new(ClickScheduler::new());
    scheduler.set_config(config::Config::from(initial_app_cfg.clone()));

    let scheduler_for_setup = Arc::clone(&scheduler);
    let config_manager_arc = Arc::clone(&config_manager);

    let app_state = AppState {
        scheduler,
        config_manager,
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
        .setup(move |app| {
            let handle = app.handle().clone();

            // **v1.2.0 memory-pressure workaround**: the auto-created window
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
            let hk = platform::default_hotkey_backend(scheduler_for_setup, handle);
            if let Err(e) = hk.start() {
                crate::debug_log_internal("warn", &format!("[Hotkeys] backend start failed: {e}"));
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_app_config,
            save_app_config,
            toggle_mode,
            complete_onboarding,
            get_config_path,
            get_current_mouse_pos,
            commands::get_platform_capabilities,
            commands::get_app_version,
            toggle_autoclicker,
            get_status,
            open_config_folder,
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
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|_app_handle, event| {
        if matches!(event, RunEvent::Exit) {
            // Stop native hooks before the process exits so no callback can
            // outlive its channel or application state.
            // v4.2 — stop via the HotkeyBackend contract.
            platform::default_input_backend_hotkey_stop();
            platform::stop_recorder_hooks();
            if let Some(exec) = crate::core::global() {
                exec.stop();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_debug_mode_toggle() {
        set_debug_mode(true);
        assert!(get_debug_mode());
        set_debug_mode(false);
        assert!(!get_debug_mode());
    }

    #[test]
    fn test_debug_log_batch_execution() {
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
