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
mod tray;
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
            // F3: the SIZE belongs to "remember" as well. Without it a rebuild
            // (deep sleep) takes its geometry from tauri.conf.json and a window
            // the user resized snaps back to the default size.
            if let Ok(size) = win.inner_size() {
                config.ui.window_w = Some(size.width as i32);
                config.ui.window_h = Some(size.height as i32);
            }
        }
    } else {
        // Remember OFF: don't hoard stale coordinates.
        config.ui.window_x = None;
        config.ui.window_y = None;
        config.ui.window_w = None;
        config.ui.window_h = None;
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
fn toggle_mode(app_handle: AppHandle) -> String {
    // One shared implementation for the UI badge, the global hotkey and the tray
    // menu — including the config persistence, which the hotkey path used to
    // skip (the next config load then reverted the mode).
    apply_mode_toggle(&app_handle)
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
    // Report the state that EXISTS after the attempt, not the one we asked for:
    // the typing guard and Work Mode can veto a start, and answering with the
    // requested value painted a RUNNING button over an idle clicker.
    let outcome = state
        .scheduler
        .set_active(!state.scheduler.is_active(), Some(&app_handle));
    let active = state.scheduler.is_active();
    if outcome != scheduler::ToggleOutcome::Start {
        // Stop and vetoed paths emit nothing by themselves (the click worker's
        // telemetry only exists during a live run), so the UI would freeze on
        // its optimistic guess. Say what really happened.
        state
            .scheduler
            .emit_status_now(&app_handle, if active { "RUNNING" } else { "IDLE" });
    }
    active
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

/// Dump the input layer's diagnostic ring buffers into the log.
///
/// The keyboard hook and the toggle gate record every decision
/// (`fired_action=…`, `suppressed_action=…`, `reject_reason=invalid_binding`,
/// `toggle prev_active=… new_active=…`) in memory, because file I/O on those
/// threads is forbidden. Without a way to *read* that buffer, "my hotkey does
/// nothing" is undebuggable — which is exactly how a working bind starts
/// looking broken. `warn` level on purpose: release builds keep `warn`/`error`.
#[tauri::command]
fn dump_input_diagnostics() -> usize {
    let hotkeys = crate::platform::hotkey_diag_dump();
    let toggles = crate::scheduler::toggle_diag_dump();
    let total = hotkeys.len() + toggles.len();
    debug_log_internal(
        "warn",
        &format!("[Diag] input dump: hotkey_lines={} toggle_lines={total}", hotkeys.len()),
    );
    for line in hotkeys {
        debug_log_internal("warn", &format!("[Diag] hotkey: {line}"));
    }
    for line in toggles {
        debug_log_internal("warn", &format!("[Diag] toggle: {line}"));
    }
    debug_log_internal("warn", "[Diag] end of input dump");
    total
}

#[tauri::command]
fn exit_app(app: AppHandle) {
    // A page being suspended calls this from its `beforeunload` fallback. That
    // unload is deliberate (deep sleep), so the call must not kill the backend
    // we just decided to keep alive.
    if TRAY_SUSPEND_ARMED.load(Ordering::Acquire) {
        debug_log_internal("info", "[Tray] exit_app ignored: WebView suspension in flight");
        return;
    }
    shutdown_application(&app);
}

// ── Tray + process lifetime ───────────────────────────────────────────────
//
// The frontend is disposable, the backend is not. Two rules make that real:
//
// 1. Zero windows is a normal state. `ExitRequested` (which tauri raises when
//    the last window closes) is prevented unless the user really asked to quit,
//    so hooks, the scheduler and the config watcher survive a closed window.
// 2. `exit()` goes through `ExitRequested` too (tauri 2 app.rs:573), so the
//    latch must be set BEFORE `app.exit()` — otherwise `prevent_exit()` would
//    make the process impossible to terminate.
//
// `prevent_exit()` itself ignores the restart code (tauri 2 app.rs:89-93), so
// `relaunch_app` / the updater keep working without a special case here.

/// Set once a real shutdown started (tray Quit, `exit_app`, window close with
/// "minimize to tray" off). Until then the process outlives its windows.
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

/// The tray icon thread, when one is running. `Mutex` (not a channel) because
/// the only operations are "start if absent" and "stop".
static TRAY: Mutex<Option<tray::TrayHandle>> = Mutex::new(None);

/// Start the tray icon unless one is already alive. Failures are logged only:
/// no icon must never stop the app from working as a plain window.
fn start_tray_if_needed(app: &AppHandle) {
    let mut slot = match TRAY.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if slot.as_ref().map(|t| t.is_alive()).unwrap_or(false) {
        return;
    }
    *slot = tray::TrayHandle::spawn(app.clone());
}

/// Remove the icon and join the pump thread. Idempotent, never panics.
fn stop_tray() {
    let mut slot = match TRAY.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(mut handle) = slot.take() {
        handle.shutdown();
    }
}

/// Bring the main window back (tray left click, second launch, tray menu).
/// After a deep sleep there is no window to show — it is rebuilt hidden and
/// shows itself on `frontend_ready` (Zero-Flash, same rule as HUD/overlay).
pub(crate) fn restore_main_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
        // F2: the HUD/overlay were hidden together with the main window, so they
        // come back with it (the restore path is the same one for both).
        show_secondary_windows(app);
        debug_log_internal("info", "[Tray] main window restored");
        return;
    }
    if let Err(e) = rebuild_main_window(app) {
        debug_log_internal("error", &format!("[Tray] main window rebuild failed: {e}"));
    }
}

/// Rebuild the main window from `tauri.conf.json` after a deep sleep.
///
/// The conf is the single source of truth for size/title/visibility, so the
/// rebuilt window cannot drift from the one tauri creates at boot. The window is
/// created hidden and `SHOW_ON_READY` lets the fresh page reveal itself once it
/// booted — showing a cold WebView earlier paints the DWM white rectangle.
fn rebuild_main_window(app: &AppHandle) -> Result<(), String> {
    // Two fast tray clicks must not race on the label "main": tauri would refuse
    // the second window with "a window with label main already exists".
    if MAIN_REBUILDING.swap(true, Ordering::AcqRel) {
        return Err("a rebuild is already in progress".into());
    }
    let result = (|| -> Result<(), String> {
        if app.get_webview_window("main").is_some() {
            return Ok(()); // raced with another restore path; nothing to do
        }
        let conf = app
            .config()
            .app
            .windows
            .first()
            .cloned()
            .ok_or_else(|| "tauri.conf.json declares no main window".to_string())?;
        SHOW_ON_READY.store(true, Ordering::Release);
        let win = tauri::WebviewWindowBuilder::from_config(app, &conf)
            .map_err(|e| format!("builder failed: {e}"))?
            .visible(false)
            .build()
            .map_err(|e| format!("build failed: {e}"))?;
        let _ = win.set_title("NanoClick");
        // Same position restore as the boot path — the window is still hidden, so
        // nothing flickers and a "remembered" spot is not silently forgotten just
        // because the WebView was rebuilt.
        if let Some(state) = app.try_state::<AppState>() {
            let cfg = state.config_manager.load();
            if cfg.ui.remember_window_position {
                let (vw, vh) = platform::get_screen_size();
                if let (Some(x), Some(y)) = (cfg.ui.window_x, cfg.ui.window_y) {
                    if x >= 0 && y >= 0 && x < vw && y < vh {
                        let _ = win.set_position(tauri::PhysicalPosition::new(x, y));
                    }
                }
                // F3: and the size — a rebuild must not silently reset the
                // geometry the user chose.
                if let (Some(w), Some(h)) = (cfg.ui.window_w, cfg.ui.window_h) {
                    if w >= 200 && h >= 200 && w <= vw && h <= vh {
                        let _ = win.set_size(tauri::PhysicalSize::new(w as u32, h as u32));
                    }
                }
            }
        }
        spawn_frontend_watchdog(app.clone());
        debug_log_internal(
            "info",
            "[Tray] main window rebuilt (hidden until frontend_ready)",
        );
        // The overlay died with the rest. Restore it the same way the boot path
        // does: staggered, off the restore path, only if the user wants ripples.
        let ripples = app
            .try_state::<AppState>()
            .map(|s| s.config_manager.load().ui.visual_ripple)
            .unwrap_or(false);
        if ripples {
            let h = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(1200));
                let _ = overlay::ensure_overlay_window(&h);
            });
        }
        Ok(())
    })();
    MAIN_REBUILDING.store(false, Ordering::Release);
    result
}

/// Tray menu: start/stop the clicker without touching the UI.
///
/// The page owns no clicker state of its own (it re-reads `get_status` on boot
/// and follows `status-update` afterwards), so this path must **report** what
/// happened. A silent no-op on a vetoed start is indistinguishable from a dead
/// menu item — which is exactly how the tray behaved before.
pub(crate) fn toggle_clicking_from_tray(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let outcome = state
        .scheduler
        .set_active(!state.scheduler.is_active(), Some(app));
    let active = state.scheduler.is_active();
    debug_log_internal(
        "info",
        &format!("[Tray] clicking toggled -> {active} ({outcome:?})"),
    );
    // Always: the tray is the one entry point that runs while the page may not
    // even exist (deep sleep), so the state has to leave the backend here.
    state
        .scheduler
        .emit_status_now(app, if active { "RUNNING" } else { "IDLE" });
    if !matches!(
        outcome,
        scheduler::ToggleOutcome::Start | scheduler::ToggleOutcome::Stop
    ) {
        // Vetoed (Work Mode / typing guard): the clicker deliberately ignored
        // the menu item. Tell the page, so it can explain itself with a toast
        // instead of looking broken.
        let reason = match outcome {
            scheduler::ToggleOutcome::BlockedByTyping => "typing",
            scheduler::ToggleOutcome::IgnoredWorkMode => "work_mode",
            _ => "blocked",
        };
        report_tray_action(
            app,
            match reason {
                "typing" => "Start blocked: you are typing",
                "work_mode" => "Start blocked: Work Mode is active",
                _ => "Start blocked",
            },
        );
        if let Some(win) = app.get_webview_window("main") {
            let _ = win.emit(
                "tray-action-result",
                serde_json::json!({ "action": "toggle_clicking", "ok": false, "reason": reason }),
            );
        }
    } else {
        report_tray_action(
            app,
            if active {
                "Clicking started"
            } else {
                "Clicking stopped"
            },
        );
    }
}

/// Tray menu: show/hide the floating HUD and persist the new value, so the
/// BEHAVIOR checkbox and the tray menu never disagree.
pub(crate) fn toggle_hud_from_tray(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let mut cfg = state.config_manager.load();
    let show = !cfg.ui.show_hud;
    cfg.ui.show_hud = show;
    if let Err(e) = state.config_manager.save(&cfg) {
        debug_log_internal("warn", &format!("[Tray] HUD setting not persisted: {e}"));
    }
    let _ = toggle_hud_window(app.clone(), show);
    // The BEHAVIOR checkbox on the page mirrors this WITHOUT saving: the tray
    // already persisted the value, and a page-side save would write its stale
    // checkbox back to disk and silently undo the menu choice.
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.emit("tray-hud-changed", show);
    }
    report_tray_action(app, if show { "HUD shown" } else { "HUD hidden" });
    debug_log_internal("info", &format!("[Tray] HUD -> {show}"));
}

/// Flip Work / Autoclicker mode, **persist it**, and report the new mode.
///
/// Every entry point goes through here — the UI badge, the global hotkey and the
/// tray menu. The hotkey used to flip only the in-memory atomic, so the next
/// config save (`set_config` re-reads `active_mode`) silently reverted the
/// user's choice; the tray needs the persistence most, because with deep sleep
/// there is no page and the menu item is the only way to switch at all.
pub(crate) fn apply_mode_toggle(app: &AppHandle) -> String {
    let Some(state) = app.try_state::<AppState>() else {
        return "autoclicker".into();
    };
    // The scheduler emits `status-update` itself, so a live page follows along.
    let new_mode = state.scheduler.toggle_mode(Some(app));
    let mut cfg = state.config_manager.load();
    cfg.active_mode = new_mode.clone();
    if let Err(e) = state.config_manager.save(&cfg) {
        debug_log_internal("warn", &format!("[Tray] mode not persisted: {e}"));
    }
    debug_log_internal("info", &format!("[Tray] mode toggled -> {new_mode}"));
    new_mode
}

/// Tray menu: switch Work / Autoclicker mode while the UI may be asleep.
///
/// This is the only mode entry point that works with zero windows alive, so it
/// goes through the same shared helper and logs its own action name — the mode
/// menu item and the hotkey must never drift apart.
pub(crate) fn toggle_mode_from_tray(app: &AppHandle) {
    let mode = apply_mode_toggle(app);
    debug_log_internal("info", &format!("[Tray] mode menu item -> {mode}"));
    report_tray_action(
        app,
        if mode == "work" {
            "Work Mode ON — clicking blocked"
        } else {
            "Autoclicker mode ON"
        },
    );
}

/// Is Work Mode on right now?
///
/// Read from the scheduler's atomic, never from the config: the tray pump calls
/// this right before showing the menu, and that thread does no file I/O by
/// design (`TRAY_STATE` is the only state it owns).
pub(crate) fn tray_work_mode_active(app: &AppHandle) -> bool {
    app.try_state::<AppState>()
        .map(|s| !s.scheduler.is_autoclicker_mode())
        .unwrap_or(false)
}

/// Is the clicker running right now?
///
/// Same rule as `tray_work_mode_active`: the scheduler's atomic only, never the
/// config file — the tray pump calls this while opening the menu.
pub(crate) fn tray_clicking_active(app: &AppHandle) -> bool {
    app.try_state::<AppState>()
        .map(|s| s.scheduler.is_active())
        .unwrap_or(false)
}

/// Report a tray action to the user.
///
/// A live page gets its own feedback (status-update → badge, `tray-action-result`
/// → toast). With **zero windows** — deep sleep destroyed the WebView — there is
/// nothing to emit into, and a menu item that produces no visible effect is
/// indistinguishable from a broken binding. The balloon is the only channel left.
fn report_tray_action(app: &AppHandle, msg: &str) {
    if app.get_webview_window("main").is_none() {
        tray::notify_balloon("NanoClick", msg);
    }
}

/// What "Reload interface" must do.
///
/// Pure on purpose (same pattern as `deep_sleep_allowed` / `WatchdogVerdict`):
/// the decision is unit-testable without a window or an `AppHandle`, and the
/// handler cannot grow a second, drifting `if`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReloadStrategy {
    /// A live window: reload the page in place. The WebView keeps its geometry
    /// and the fresh page re-registers every handler plus the flush hook.
    ReloadPage,
    /// No window at all (deep sleep released it): build it again from
    /// `tauri.conf.json`.
    RebuildWindow,
}

fn reload_strategy(window_exists: bool) -> ReloadStrategy {
    if window_exists {
        ReloadStrategy::ReloadPage
    } else {
        ReloadStrategy::RebuildWindow
    }
}

/// Tray menu: reload the interface.
///
/// The watchdog already spends exactly one automatic reload on a silent page, so
/// this is the user's own escape hatch when the interface is broken anyway. A
/// live window reloads its page; a window released by deep sleep is rebuilt.
pub(crate) fn reload_interface_from_tray(app: &AppHandle) {
    match reload_strategy(app.get_webview_window("main").is_some()) {
        ReloadStrategy::ReloadPage => {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.eval("window.__NANOCLICK_RELOADING__ = true; location.reload();");
                // A reload resurrects the page, but the watchdog countdown belongs
                // to the previous one; a fresh watchdog keeps a second silent boot
                // honest (and still never ends the process).
                spawn_frontend_watchdog(app.clone());
                debug_log_internal("info", "[Tray] interface reload requested");
                report_tray_action(app, "Interface reloaded");
            }
        }
        ReloadStrategy::RebuildWindow => match rebuild_main_window(app) {
            Ok(()) => {
                debug_log_internal("info", "[Tray] interface rebuilt on reload request");
                report_tray_action(app, "Interface rebuilt");
            }
            Err(e) => debug_log_internal("error", &format!("[Tray] reload failed: {e}")),
        },
    }
}

/// Tray menu: restart the whole app.
///
/// A page reload cannot fix a wedged backend, and with the WebView gone the
/// user has no other way out than Quit → relaunch by hand. The tray icon is
/// removed first so Windows never has to reap a ghost one.
pub(crate) fn restart_app_from_tray(app: &AppHandle) {
    debug_log_internal("warn", "[Tray] restart requested from the menu");
    report_tray_action(app, "Restarting…");
    // Latch the shutdown flag for the teardown paths that consult it; tauri
    // ignores the restart code in `prevent_exit`, so `relaunch_app` still works.
    SHUTTING_DOWN.store(true, Ordering::Release);
    stop_tray();
    app.restart();
}

/// Hide the secondary windows together with the main one.
///
/// F2: the HUD and the ripple overlay are separate top-level windows, so hiding
/// the main window used to leave the floating click counter on screen while the
/// app claimed to be "in the tray". They are HIDDEN, not destroyed: the page is
/// still alive in this mode and would not recreate them.
fn hide_secondary_windows(app: &AppHandle) {
    for label in ["hud", "overlay"] {
        if let Some(win) = app.get_webview_window(label) {
            let _ = win.hide();
        }
    }
}

/// Bring the HUD / overlay back with the main window — only the ones the config
/// still wants (mirror of `hide_secondary_windows`).
fn show_secondary_windows(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let cfg = state.config_manager.load();
    if cfg.ui.show_hud {
        if let Some(win) = app.get_webview_window("hud") {
            let _ = win.show();
        }
    }
    if cfg.ui.visual_ripple {
        if let Some(win) = app.get_webview_window("overlay") {
            let _ = win.show();
        }
    }
}

/// Should the window's close button merely hide the window?
/// Never during a real shutdown, otherwise `Quit` could be swallowed.
fn should_minimize_to_tray(app: &AppHandle) -> bool {
    if SHUTTING_DOWN.load(Ordering::Acquire) {
        return false;
    }
    app.try_state::<AppState>()
        .map(|s| s.config_manager.load().ui.minimize_to_tray)
        .unwrap_or(false)
}

// ── Frontend watchdog (Phase E) ───────────────────────────────────────────
//
// The page is the only part of this system that a single line can brick: a
// module-level `SyntaxError` leaves the window painted with every handler
// detached (see AGENTS.md §0). The watchdog is the backend's own heartbeat
// check — `frontend_ready` is expected within a few seconds of every main-window
// creation. A silent page is logged at `error` (visible in release builds) and
// reloaded once. The backend neither dies nor blocks on it.

/// Epoch ms of the last `frontend_ready` for the CURRENT window; 0 = never.
static FRONTEND_READY_AT: AtomicU64 = AtomicU64::new(0);
/// Set when a rebuilt (deep-sleep) window must show itself on `frontend_ready`.
static SHOW_ON_READY: AtomicBool = AtomicBool::new(false);
/// How long a page may stay silent before the watchdog acts.
const FRONTEND_READY_TIMEOUT_MS: u64 = 4000;
/// Set while a rebuild of the "main" label is in flight, so two fast tray
/// clicks cannot race on the label (tauri would refuse the second window).
static MAIN_REBUILDING: AtomicBool = AtomicBool::new(false);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Pure decision for the watchdog: act once, then give up — never loop forever.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatchdogVerdict {
    /// The page reported ready: nothing to do.
    Ready,
    /// Silent so far: one reload is worth trying.
    ReloadOnce,
    /// Silent after a reload: report it and stop. A broken page must not keep
    /// the backend busy, and it must not take the process down either.
    GiveUp,
}

fn watchdog_verdict(ready: bool, reloads_done: u8) -> WatchdogVerdict {
    if ready {
        WatchdogVerdict::Ready
    } else if reloads_done == 0 {
        WatchdogVerdict::ReloadOnce
    } else {
        WatchdogVerdict::GiveUp
    }
}

/// Called by `main.js` once the module finished booting (right after the boot
/// guard flag) and by nothing else.
#[tauri::command]
fn frontend_ready(app: AppHandle) {
    FRONTEND_READY_AT.store(now_ms(), Ordering::Release);
    // A window rebuilt after deep sleep waits for this moment to become visible:
    // showing a fresh WebView earlier would paint the DWM white rectangle.
    if SHOW_ON_READY.swap(false, Ordering::AcqRel) {
        if let Some(win) = app.get_webview_window("main") {
            let _ = win.show();
            let _ = win.set_focus();
            debug_log_internal("info", "[Tray] rebuilt window shown after frontend_ready");
        }
    }
}

/// Watch the current window's heartbeat. `marker` is whatever the ready-stamp
/// held at spawn time, so a re-created window gets its own honest countdown.
fn spawn_frontend_watchdog(app: AppHandle) {
    let marker = FRONTEND_READY_AT.load(Ordering::Acquire);
    std::thread::spawn(move || {
        let mut reloads_done: u8 = 0;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(FRONTEND_READY_TIMEOUT_MS));
            let ready = FRONTEND_READY_AT.load(Ordering::Acquire) != marker;
            match watchdog_verdict(ready, reloads_done) {
                WatchdogVerdict::Ready => return,
                WatchdogVerdict::ReloadOnce => {
                    debug_log_internal(
                        "error",
                        "[Watchdog] frontend did not report ready; reloading the page once",
                    );
                    if let Some(win) = app.get_webview_window("main") {
                        let _ = win.eval("location.reload()");
                    }
                    reloads_done = 1;
                }
                WatchdogVerdict::GiveUp => {
                    // Never leave the user with an invisible app: if this window
                    // was waiting for a ready that never came, show it anyway so
                    // the boot-guard banner inside the page becomes visible.
                    if SHOW_ON_READY.swap(false, Ordering::AcqRel) {
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                    debug_log_internal(
                        "error",
                        "[Watchdog] frontend still silent: backend continues standalone \
                         (hooks, scheduler and tray stay alive)",
                    );
                    return;
                }
            }
        }
    });
}

// ── Deep sleep to tray (Phase D, opt-in) ──────────────────────────────────
//
// MEASURED 2026-09-22 (Windows 10), private working set — the metric Task Manager
// shows and the only one comparable between apps:
//   window visible  → 7 processes, 117–123 MB (host 5.3 MB + 6 Chromium helpers)
//   hidden to tray  → 7 processes, 122.8 MB  (unchanged: hide frees nothing)
//   deep sleep      → 1 process,   5.0 MB    (helpers 6 → 0, −118 MB, −96 %)
// Do NOT quote `Win32_Process.WorkingSetSize` sums here: that is the TOTAL working
// set, which counts shared pages (msedge.dll, system DLLs) once per process. The
// same PID reads ~5.0 MB private vs ~26.5 MB total, and summing it across the tree
// produced a phantom 366–383 MB next to the real 117–123 MB.
//
// Hard rules, and why they exist:
// * Never while something is in flight (clicker / macro / recorder): losing the
//   UI mid-run is not worth 200 MB, and the focus guard's baseline story would
//   change under the user's feet.
// * The page is asked to persist first (`__nanoclick_tray_flush__` →
//   `tray_flush_done`), with a bounded wait: stats are flushed to disk at most
//   every 5 s, so up to 5 s of clicks could otherwise be lost.
// * `beforeunload` fires on destroy and the page's fallback calls `exit_app`:
//   `TRAY_SUSPEND_ARMED` makes that call a no-op, and the page also gets the
//   existing `__NANOCLICK_RELOADING__` flag. Two locks, one door.
// * The rebuild goes through `WebviewWindowBuilder::from_config`, so the window
//   keeps the size/title/visible settings from `tauri.conf.json` instead of a
//   second, drift-prone copy in code.

/// Armed while the WebView is being suspended; `exit_app` ignores calls then.
static TRAY_SUSPEND_ARMED: AtomicBool = AtomicBool::new(false);
/// Set by the page once it persisted stats/config (`tray_flush_done`).
static TRAY_FLUSH_ACK: AtomicBool = AtomicBool::new(false);
/// How long the suspender waits for that ack before going ahead anyway.
const TRAY_FLUSH_WAIT_MS: u64 = 700;

/// Pure gate: destroying the WebView is only safe when nothing is mid-flight.
fn deep_sleep_allowed(
    deep_sleep_enabled: bool,
    clicking: bool,
    macro_running: bool,
    recording: bool,
) -> bool {
    deep_sleep_enabled && !clicking && !macro_running && !recording
}

/// Read the four inputs from live state. A poisoned recorder lock counts as
/// "recording" — unknown state must never destroy a window.
fn deep_sleep_allowed_now(app: &AppHandle) -> bool {
    let enabled = app
        .try_state::<AppState>()
        .map(|s| s.config_manager.load().ui.deep_sleep_to_tray)
        .unwrap_or(false);
    let clicking = app
        .try_state::<AppState>()
        .map(|s| s.scheduler.is_active())
        .unwrap_or(false);
    let macro_running = crate::core::global().map(|e| e.is_running()).unwrap_or(false);
    let recording = app
        .try_state::<crate::commands::MacroState>()
        .map(|s| s.recorder.lock().map(|slot| slot.is_some()).unwrap_or(true))
        .unwrap_or(false);
    deep_sleep_allowed(enabled, clicking, macro_running, recording)
}

/// Sent by the page when it finished persisting pending state.
#[tauri::command]
fn tray_flush_done() {
    TRAY_FLUSH_ACK.store(true, Ordering::Release);
}

/// Wait (bounded) for the page's flush ack.
///
/// Returns whether the ack arrived; the caller destroys the WebView **either
/// way**. An absent ack costs at most `timeout` — it never cancels the
/// suspension, because a wedged page must not be able to keep ~109 MB of
/// Chromium helpers alive forever. Extracted so the bounded wait is unit-testable
/// instead of only smoke-testable.
fn wait_for_flush_ack(ack: &AtomicBool, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if ack.load(Ordering::Acquire) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    ack.load(Ordering::Acquire)
}

/// Ask the page to flush, then destroy the main WebView (plus the lazy HUD and
/// overlay) from the main thread. Never blocks the tray pump: the wait happens
/// on this small worker thread.
pub(crate) fn suspend_main_webview_to_tray(app: &AppHandle) {
    if app.get_webview_window("main").is_none() {
        return;
    }
    TRAY_SUSPEND_ARMED.store(true, Ordering::Release);
    TRAY_FLUSH_ACK.store(false, Ordering::Release);
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.eval(
            "window.__NANOCLICK_RELOADING__ = true; \
             if (window.__nanoclick_tray_flush__) { window.__nanoclick_tray_flush__(); }",
        );
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let acked = wait_for_flush_ack(
            &TRAY_FLUSH_ACK,
            std::time::Duration::from_millis(TRAY_FLUSH_WAIT_MS),
        );
        let app_main = app.clone();
        let _ = app.run_on_main_thread(move || {
            for label in ["hud", "overlay"] {
                if let Some(w) = app_main.get_webview_window(label) {
                    let _ = w.destroy();
                }
            }
            if let Some(w) = app_main.get_webview_window("main") {
                let _ = w.destroy();
            }
            TRAY_SUSPEND_ARMED.store(false, Ordering::Release);
            debug_log_internal(
                "info",
                &format!("[Tray] deep sleep: WebViews destroyed (flush acked: {acked})"),
            );
        });
    });
}

pub(crate) fn shutdown_application(app: &AppHandle) {
    debug_log_internal("info", "[Shutdown] Initiating clean application shutdown");

    // Latch first: `app.exit(0)` below raises ExitRequested, and an unlatched
    // prevent_exit() would make this shutdown a no-op.
    SHUTTING_DOWN.store(true, Ordering::Release);

    // 0. Drop the tray icon before the windows go away, so Windows never has to
    //    reap a ghost icon and the pump thread is joined while state is alive.
    stop_tray();

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
            // Second launch means "show me that window", not "start another
            // instance" — the plugin forwards it here.
            restore_main_window(app);
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
                        // F3: the SIZE comes back with the position, same
                        // sanitizer — a size that does not fit the current
                        // virtual screen is ignored (conf default wins).
                        if let (Some(w), Some(h)) =
                            (initial_app_cfg.ui.window_w, initial_app_cfg.ui.window_h)
                        {
                            let (vw, vh) = platform::get_screen_size();
                            if w >= 200 && h >= 200 && w <= vw && h <= vh {
                                let _ = win.set_size(tauri::PhysicalSize::new(w as u32, h as u32));
                            }
                        }
                    }
                    // Force the main window into the foreground at startup.
                    // Without this, the OS sometimes leaves it behind other
                    // apps that were active when we launched (especially
                    // single-instance scenarios where we are the second proc).
                    let _ = win.unminimize();
                    if initial_app_cfg.ui.start_minimized {
                        // "Start minimized to tray": the window exists (the
                        // backend needs it for HUD/position prefs) but stays
                        // hidden until the tray icon is clicked. `visible: false`
                        // in tauri.conf.json means nothing was painted before
                        // this point, so there is no flash.
                        let _ = win.hide();
                        debug_log_internal(
                            "info",
                            "[Startup] start_minimized: window stays in the tray",
                        );
                    } else {
                        let _ = win.show();
                        let _ = win.set_focus();
                    }
                } else {
                    debug_log_internal("error", "[Startup] main window MISSING from registry");
                }
            }

            // ── System tray (native Win32; no tray-icon/muda crate) ────────
            // Only when the feature is reachable: with both "close to tray" and
            // "start minimized" off, the app never hides, so an icon would be
            // dead weight (one more window handle + one more thread).
            if initial_app_cfg.ui.minimize_to_tray || initial_app_cfg.ui.start_minimized {
                start_tray_if_needed(&handle);
            }

            // Backend-side heartbeat check for THIS window (Phase E): a page that
            // never reports ready is logged and reloaded once, and a window that
            // is still hidden gets shown so the in-page boot banner is visible.
            spawn_frontend_watchdog(handle.clone());

            // "Start minimized" + deep sleep is the whole point of autostart: be
            // weightless. Wait for the page to report ready (its flush hook must
            // exist before the WebView can be torn down), then let it go.
            if initial_app_cfg.ui.start_minimized && initial_app_cfg.ui.deep_sleep_to_tray {
                let h = handle.clone();
                std::thread::spawn(move || {
                    for _ in 0..60 {
                        if FRONTEND_READY_AT.load(Ordering::Acquire) != 0 {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    if deep_sleep_allowed_now(&h) {
                        suspend_main_webview_to_tray(&h);
                    }
                });
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
            dump_input_diagnostics,
            frontend_ready,
            tray_flush_done,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        match event {
            RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } => {
                if label == "main" {
                    if should_minimize_to_tray(&app_handle) {
                        api.prevent_close();
                        // Two ways to live in the tray: plain hide (instant and,
                        // measured, free — private working set stays ~120 MB) or
                        // deep sleep (destroys the WebView, rebuilds it on the
                        // next tray click: 1 process / 5.0 MB). Deep sleep is
                        // refused whenever something is in flight.
                        if deep_sleep_allowed_now(&app_handle) {
                            start_tray_if_needed(&app_handle);
                            suspend_main_webview_to_tray(&app_handle);
                            debug_log_internal("info", "[Tray] close requested -> deep sleep");
                        } else {
                            if let Some(win) = app_handle.get_webview_window("main") {
                                let _ = win.hide();
                            }
                            hide_secondary_windows(&app_handle);
                            start_tray_if_needed(&app_handle);
                            debug_log_internal("info", "[Tray] close requested -> hidden to tray");
                        }
                    } else {
                        shutdown_application(&app_handle);
                    }
                }
            }
            RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::Resized(_),
                ..
            } => {
                // F1: a tray app must not "minimize to the taskbar". The `_`
                // button has to behave exactly like the close button, otherwise
                // the window looks lost in the taskbar while the tray icon still
                // claims the app lives there.
                if label == "main"
                    && app_handle
                        .get_webview_window("main")
                        .and_then(|w| w.is_minimized().ok())
                        .unwrap_or(false)
                    && should_minimize_to_tray(&app_handle)
                {
                    if deep_sleep_allowed_now(&app_handle) {
                        start_tray_if_needed(&app_handle);
                        suspend_main_webview_to_tray(&app_handle);
                        debug_log_internal("info", "[Tray] minimize requested -> deep sleep");
                    } else {
                        if let Some(win) = app_handle.get_webview_window("main") {
                            let _ = win.hide();
                        }
                        hide_secondary_windows(&app_handle);
                        start_tray_if_needed(&app_handle);
                        debug_log_internal("info", "[Tray] minimize requested -> hidden to tray");
                    }
                }
            }
            RunEvent::ExitRequested { api, .. } => {
                // No windows left is a normal state for us (tray-only mode).
                // Only a real shutdown may end the process. `prevent_exit`
                // ignores the restart code, so the updater still works.
                if !SHUTTING_DOWN.load(Ordering::Acquire) {
                    api.prevent_exit();
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
                // Which is reached even when the exit was not requested through
                // `shutdown_application` (e.g. the OS ending the session).
                stop_tray();
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

/// Pure decisions behind the tray lifetime (frontend watchdog + deep sleep).
/// No Win32 and no `AppHandle` here on purpose: these must run on any platform,
/// in microseconds, and they are the parts that decide whether a WebView dies.
#[cfg(test)]
mod tray_lifetime_tests {
    use super::*;

    #[test]
    fn watchdog_waits_then_reloads_once_then_gives_up() {
        assert_eq!(watchdog_verdict(true, 0), WatchdogVerdict::Ready);
        assert_eq!(watchdog_verdict(true, 1), WatchdogVerdict::Ready);
        assert_eq!(watchdog_verdict(false, 0), WatchdogVerdict::ReloadOnce);
        assert_eq!(watchdog_verdict(false, 1), WatchdogVerdict::GiveUp);
        // Never loops: one reload is the whole repertoire.
        assert_eq!(watchdog_verdict(false, 9), WatchdogVerdict::GiveUp);
    }

    #[test]
    fn deep_sleep_needs_the_flag_and_an_idle_backend() {
        // Enabled + idle is the only combination allowed to destroy a WebView.
        assert!(deep_sleep_allowed(true, false, false, false));
        // Disabled flag: never, whatever else is going on.
        assert!(!deep_sleep_allowed(false, false, false, false));
        // Any single in-flight state blocks it.
        assert!(!deep_sleep_allowed(true, true, false, false), "clicker running");
        assert!(!deep_sleep_allowed(true, false, true, false), "macro playing");
        assert!(!deep_sleep_allowed(true, false, false, true), "recording");
        assert!(!deep_sleep_allowed(true, true, true, true));
    }

    /// "Reload interface" must pick exactly one path per window state. The
    /// decision lives in `reload_strategy` (pure), never inline in the handler,
    /// so it cannot drift from this contract.
    #[test]
    fn reload_strategy_follows_the_window_presence() {
        assert_eq!(reload_strategy(true), ReloadStrategy::ReloadPage);
        assert_eq!(reload_strategy(false), ReloadStrategy::RebuildWindow);
    }

    #[test]
    fn flush_wait_stays_short_and_bounded() {
        // The user is waiting for a tray interaction to feel instant; the wait
        // exists only to let the page persist pending stats.
        assert!(TRAY_FLUSH_WAIT_MS <= 1000, "keep the tray snappy");
    }

    #[test]
    fn watchdog_timeout_is_human_scale() {
        // Long enough for a cold WebView2 boot on a slow disk, short enough that
        // a dead page is noticed while the user is still looking at it.
        assert!((2000..=8000).contains(&FRONTEND_READY_TIMEOUT_MS));
    }

    #[test]
    fn flush_ack_wait_detects_the_ack_and_stays_bounded() {
        use std::sync::atomic::AtomicBool as Flag;
        use std::time::{Duration, Instant};

        // Already acked (or a very fast page): return at once, do not sit out the
        // timeout — the user is waiting for a tray click to feel instant.
        let acked = Flag::new(true);
        let t0 = Instant::now();
        assert!(wait_for_flush_ack(&acked, Duration::from_millis(200)));
        assert!(
            t0.elapsed() < Duration::from_millis(100),
            "an existing ack must not wait for the timeout"
        );

        // Dead page: give up after the timeout and report `false`. The caller
        // destroys the WebView anyway (that is the whole point of the bound), it
        // only loses the unflushed tail.
        let silent = Flag::new(false);
        let t0 = Instant::now();
        assert!(!wait_for_flush_ack(&silent, Duration::from_millis(60)));
        assert!(
            t0.elapsed() >= Duration::from_millis(50),
            "a missing ack must be honoured as a bounded wait, not as an instant give-up"
        );

        // A page that answers late is still honoured.
        let late = std::sync::Arc::new(Flag::new(false));
        let armer = std::sync::Arc::clone(&late);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            armer.store(true, Ordering::Release);
        });
        assert!(wait_for_flush_ack(&late, Duration::from_millis(400)));
    }
}
