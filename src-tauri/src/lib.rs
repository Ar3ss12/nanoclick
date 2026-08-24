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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{AppHandle, Manager, RunEvent, State};
use tauri_plugin_updater::UpdaterExt;

static DEBUG_LOG_FILE: OnceLock<Mutex<Option<File>>> = OnceLock::new();
static DEBUG_LOG_BYTES: AtomicU64 = AtomicU64::new(0);
const DEBUG_LOG_MAX_BYTES: u64 = 2 * 1024 * 1024;

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

// ── DEBUG: capture JS console output via invoke ──
#[tauri::command]
fn debug_log(level: String, message: String) {
    debug_log_internal(&level, &message);
}


#[tauri::command]
fn relaunch_app(app: AppHandle) {
    use tauri::Manager;
    app.restart();
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
    if !cfg!(debug_assertions) && level == "info" {
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
    let path = std::env::temp_dir().join("nanoclick_web.log");
    let should_rotate = DEBUG_LOG_BYTES.fetch_add(line.len() as u64, Ordering::Relaxed)
        + line.len() as u64
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
        DEBUG_LOG_BYTES.store(line.len() as u64, Ordering::Relaxed);
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
            let _ = file.write_all(line.as_bytes());
        }
    }
    eprint!("{}", line);
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
    let config_manager = Arc::new(ConfigManager::new());
    let initial_app_cfg = config_manager.load();

    let scheduler = Arc::new(ClickScheduler::new());
    scheduler.set_config(config::Config::from(initial_app_cfg.clone()));

    let scheduler_for_setup = Arc::clone(&scheduler);

    let app_state = AppState {
        scheduler,
        config_manager,
    };

    let macro_state = commands::MacroState::default();
    let macro_executor: Arc<crate::core::ExecutorHandle> = Arc::clone(&macro_state.executor);
    crate::core::set_global((*macro_executor).clone());

    let app = tauri::Builder::default()
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
            platform::spawn_global_hotkey_listener(scheduler_for_setup, handle);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_app_config,
            save_app_config,
            toggle_mode,
            complete_onboarding,
            get_config_path,
            get_current_mouse_pos,
            toggle_autoclicker,
            get_status,
            open_config_folder,
            set_windows_autostart,
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
            check_for_updates,
        relaunch_app,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|_app_handle, event| {
        if matches!(event, RunEvent::Exit) {
            // Stop native hooks before the process exits so no callback can
            // outlive its channel or application state.
            platform::shutdown_global_hotkey_listener();
            platform::stop_recorder_hooks();
            if let Some(exec) = crate::core::global() {
                exec.stop();
            }
        }
    });
}
