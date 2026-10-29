// Audit-only restriction lints: OFF by default, not part of `clippy::all`.
// Every unsafe block in this crate is a Win32 FFI call, so the raw output is
// ~90 lines of "missing SAFETY comment" noise that hides real findings. The
// audit is still available on demand: remove this attribute and run
// `cargo clippy -j 2 --lib --bins -- -W clippy::undocumented_unsafe_blocks
//  -W clippy::multiple_unsafe_ops_per_block` (last measured: 67 and 25).
#![allow(
    clippy::undocumented_unsafe_blocks,
    clippy::multiple_unsafe_ops_per_block
)]

mod commands;
mod config;
mod config_manager;
mod core;
pub mod daemon_client;
pub mod daemon_server;
pub mod daemon_single;
pub mod daemon_supervise;
pub mod daemon_transport;
pub mod defaults;
mod guard;
pub mod input_guardian;
pub mod ipc;
pub mod notifications;
mod overlay;
mod persistence;
pub mod platform;
mod recorder;
mod retry;
pub mod satellite;
mod scheduler;
pub mod stability;
// Narrow re-export for the `click_pacing` bench (see benches/): the pacing
// math is public, the rest of the scheduler module stays private.
pub use scheduler::calc_single_timing;
mod stats_agg;
mod status_sink;
mod tauri_sink;
mod tray;
mod watcher;
mod window_dispatch;
pub use notifications::*;

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
    /// Session stats counters (Rust-owned since the stats migration):
    /// the 66 ms ticks fold into this, the page keeps only Canvas paint.
    pub session_stats: Mutex<crate::stats_agg::SessionStats>,
    /// FIFO queue of blocking boot notices that require acknowledgement.
    /// Drained one-by-one by `get_startup_notices`; the UI stays blocked
    /// until every notice is dismissed with OK. Never blocks boot itself.
    pub startup_notices: Mutex<Vec<AppNotice>>,
    /// Non-blocking runtime toast queue fed by the observer watcher.
    /// Drained by `poll_file_toasts` (auto-dismiss in UI, no acknowledgement).
    pub file_toasts: Mutex<Vec<AppNotice>>,
    pub notification_store: Arc<NotificationStore>,
}

/// Shared observer handle so save-commands can mark their own writes.
pub struct WatcherState(pub Arc<crate::watcher::Observer>);

/// Granular notice level for the blocking boot-notice modal.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NoticeLevel {
    Info,
    Warning,
    Critical,
}

/// Blocking-modal payload: level + title + message + optional path details.
/// Frontend rules (vanilla JS, no libs): no [X], backdrop clicks are swallowed,
/// Esc has no effect, the ONLY exit is the OK button. Multiple notices are shown
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
        A::RepairedAndPatched {
            backup_path,
            details,
        } => Some(AppNotice {
            level: NoticeLevel::Warning,
            title: "notice_cfg_repaired_title".into(),
            message: format!("notice_cfg_repaired_msg|{}", details.len()),
            details: Some(backup_path.display().to_string()),
        }),
        A::CorruptedAndRecovered {
            backup_path,
            restored_from_last_good,
        } => {
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

/// Macros-store healing mapped into the same blocking-notice queue shape.
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
fn file_health_notice(file: &str, verdict: &crate::watcher::FileHealth) -> Option<AppNotice> {
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
    state
        .startup_notices
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}

/// Non-blocking runtime toast queue (watcher verdicts). Unlike the blocking
/// boot-notice queue, these auto-dismiss: observer info, never file rewrites.
#[tauri::command]
fn poll_file_toasts(state: State<'_, AppState>) -> Vec<AppNotice> {
    state
        .file_toasts
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}

#[tauri::command]
fn get_notifications(state: State<'_, AppState>, limit: Option<usize>) -> Vec<AppNotification> {
    state.notification_store.get_all(limit)
}

#[tauri::command]
fn get_unread_notifications(state: State<'_, AppState>) -> Vec<AppNotification> {
    state.notification_store.get_unread()
}

#[tauri::command]
fn mark_notifications_read(state: State<'_, AppState>, ids: Vec<String>) -> Result<(), String> {
    state.notification_store.mark_read(&ids);
    Ok(())
}

#[tauri::command]
fn clear_notifications(state: State<'_, AppState>) -> Result<(), String> {
    state.notification_store.clear();
    Ok(())
}

#[tauri::command]
fn push_client_notification(
    state: State<'_, AppState>,
    notification: AppNotification,
) -> Result<String, String> {
    let id = notification.id.clone();
    state.notification_store.push(notification, None, false);
    Ok(id)
}

#[tauri::command]
fn get_app_config(state: State<'_, AppState>) -> AppConfig {
    let app_cfg = state.config_manager.load();
    state
        .scheduler
        .set_config(config::Config::from(app_cfg.clone()));
    app_cfg
}

/// Copy the previous config aside before this session's first write, and TELL the user.
///
/// Once per process (guarded inside `ConfigManager`). The toast rides the existing
/// `file_toasts` queue, which the page polls every 3 s — the same channel the file
/// watcher uses; the path itself goes to the log at `warn` (release-visible).
fn ensure_session_config_backup(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let Some(path) = state.config_manager.snapshot_before_first_save() else {
        return;
    };
    debug_log_internal(
        "warn",
        &format!("[Config] session snapshot: {}", path.display()),
    );
    // Bind the guard in its own statement on purpose: `if let Ok(..) = state.file_toasts.lock()`
    // keeps the `Result` temporary alive until the end of the block, and that outlives the
    // `State` borrow (E0597). Same pattern as `start_tray_if_needed`.
    let mut queue = match state.file_toasts.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    // Cap: drop oldest, keep the queue bounded (the watcher's rule).
    if queue.len() >= 8 {
        queue.remove(0);
    }
    queue.push(AppNotice {
        level: NoticeLevel::Info,
        title: "notice_cfg_backup_title".into(),
        message: "notice_cfg_backup_msg".into(),
        details: Some(path.display().to_string()),
    });
}

#[tauri::command]
fn save_app_config(
    config: AppConfig,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<AppConfig, String> {
    // Config bulkhead (stability::config_node, max_in_flight(1)): the config
    // writers race on load-save. The loser gets "config busy - retry" and the
    // frontend retries once with a fresh collect. Scoped execution: the guard
    // commits on Ok and aborts on Err, so no implicit-drop path exists here
    // by construction. Rollback is a no-op: the disk is untouched until
    // the persist call inside the closure.
    let node = crate::stability::config_node();
    // Scope boundary: everything touching the config file and the scheduler
    // mirror runs inside. Satellite windows are NOT built here — the toggle
    // commands are their only owner (SATELLITE_PLAN §1), so a save can never
    // block the UI thread on WebView2 init.
    let outcome: Result<AppConfig, snubber::SnubberError<String>> = node
        .scoped(
            || {},
            || -> Result<AppConfig, String> {
                // Previous file is copied aside before this session's first
                // write, so a wrong-but-valid write stays recoverable by hand.
                ensure_session_config_backup(&app);
                let _ = crate::platform::set_always_run_as_admin(config.ui.always_run_as_admin);
                let mut config = config;
                // Window geometry is stamped from the live window into the
                // same atomic save (no extra file, no plugin). Best-effort:
                // the window may be absent in tests, remember may be off.
                if config.ui.remember_window_position {
                    if let Some(win) = app.get_webview_window("main") {
                        if let Ok(pos) = win.outer_position() {
                            config.ui.window_x = Some(pos.x);
                            config.ui.window_y = Some(pos.y);
                        }
                        // Size is part of "remember": without it a rebuild
                        // (deep sleep) falls back to tauri.conf.json geometry
                        // and discards the user's resize.
                        if let Ok(size) = win.inner_size() {
                            config.ui.window_w = Some(size.width as i32);
                            config.ui.window_h = Some(size.height as i32);
                        }
                    }
                } else {
                    config.ui.window_x = None;
                    config.ui.window_y = None;
                    config.ui.window_w = None;
                    config.ui.window_h = None;
                }
                // Visibility is stamped from the live window: the page never
                // observes a tray hide or deep sleep, so its copy is stale by
                // definition. `MAIN_IN_TRAY` outranks the live read because
                // the close-time flush runs while the window is still shown.
                config.ui.window_was_visible = !MAIN_IN_TRAY.load(Ordering::Acquire)
                    && app
                        .get_webview_window("main")
                        .and_then(|w| w.is_visible().ok())
                        .unwrap_or(false);
                // Abort path: `scoped` converts this Err into an explicit
                // abort. No implicit drop, no breaker streak.
                state.config_manager.save(&config)?;
                // The file observer marks our own write inside `save()`.
                state
                    .scheduler
                    .set_config(config::Config::from(config.clone()));
                Ok(config)
            },
        );
    match outcome {
        Ok(config) => Ok(config),
        Err(snubber::SnubberError::Admission(_)) => Err("config busy - retry".into()),
        Err(snubber::SnubberError::Work(e)) => Err(e),
        // `#[non_exhaustive]`: unknown variant — nothing was committed, so
        // answer with the same retry refusal instead of panicking.
        Err(other) => Err(format!("config busy - retry ({other})")),
    }
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
fn reset_config_to_defaults(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<AppConfig, String> {
    // Same bulkhead as save_app_config: reset races the close-time flush.
    // Scoped: commit on Ok, explicit abort on Err — no implicit-drop path.
    let node = crate::stability::config_node();
    let outcome: Result<(AppConfig, bool, bool, bool, bool), snubber::SnubberError<String>> = node
        .scoped(
            || {},
            || -> Result<(AppConfig, bool, bool, bool, bool), String> {
                let prev = state.config_manager.load();
                let app_cfg = state.config_manager.reset_to_defaults()?;
                state
                    .scheduler
                    .set_config(config::Config::from(app_cfg.clone()));
                let ripple_changed = app_cfg.ui.visual_ripple != prev.ui.visual_ripple;
                let want_ripple = app_cfg.ui.visual_ripple;
                let hud_changed = app_cfg.ui.show_hud != prev.ui.show_hud;
                let want_hud = app_cfg.ui.show_hud;
                Ok((app_cfg, ripple_changed, want_ripple, hud_changed, want_hud))
            },
        );
    let (app_cfg, ripple_changed, want_ripple, hud_changed, want_hud) = match outcome {
        Ok(v) => v,
        Err(snubber::SnubberError::Admission(_)) => {
            return Err("config busy - retry".into());
        }
        Err(snubber::SnubberError::Work(e)) => return Err(e),
        // `#[non_exhaustive]`: nothing was committed; refuse like an
        // admission rejection so the page retries rather than assumes success.
        Err(other) => return Err(format!("config busy - retry ({other})")),
    };
    // Post-commit side-effects (bulkhead released): satellite toggles run
    // through their own bulkhead and may block on WebView2 init.
    // Same lazy-WebView sync as save_app_config: defaults flip visual_ripple
    // (true) / show_hud (false), so create/destroy on demand.
    if ripple_changed {
        let _ = overlay::toggle_overlay(app.clone(), want_ripple);
    }
    if hud_changed {
        let _ = toggle_hud_window(app.clone(), want_hud);
    }
    Ok(app_cfg)
}

#[tauri::command]
fn get_config_path(state: State<'_, AppState>) -> String {
    state.config_manager.get_config_path()
}

/// Tell the live page how a satellite toggle landed, so a busy checkbox locks
/// itself with a toast instead of mirroring a window that was never built.
/// Fire-and-forget (`let _ =`, targeted at `main` only) — same contract as
/// every other emit: the click loop never waits for the UI.
fn emit_satellite_toggle_result(app: &AppHandle, which: &str, ok: bool, reason: &str) {
    use tauri::Emitter;
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.emit(
            "satellite-toggle-result",
            serde_json::json!({ "which": which, "ok": ok, "reason": reason }),
        );
    }
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
        // its optimistic guess. Say what really happened. A stability veto
        // names its own latch instead of a bare IDLE.
        let text = match outcome {
            scheduler::ToggleOutcome::BlockedByStability => {
                crate::stability::refusal_text().unwrap_or("IDLE")
            }
            _ if active => "RUNNING",
            _ => "IDLE",
        };
        state.scheduler.emit_status_now(&app_handle, text);
    }
    active
}

/// `get_status` payload: the clicker frame plus the stability verdicts.
/// Flattened, so every existing field keeps its name and the page needs no
/// change while `release_pending` / `engine_stuck` / `memory` become readable.
#[derive(serde::Serialize)]
struct StatusSnapshot {
    #[serde(flatten)]
    clicker: scheduler::StatusUpdate,
    #[serde(flatten)]
    stability: crate::stability::StabilityStatus,
}

#[tauri::command]
fn get_status(state: State<'_, AppState>) -> StatusSnapshot {
    let is_auto = state.scheduler.is_autoclicker_mode();
    let is_act = state.scheduler.is_active();
    let status_text = if !is_auto {
        "WORK MODE (PAUSED)".to_string()
    } else if is_act {
        crate::stability::status_text_override("RUNNING")
            .unwrap_or("RUNNING")
            .to_string()
    } else {
        "IDLE".to_string()
    };
    StatusSnapshot {
        clicker: scheduler::StatusUpdate {
            active: is_act,
            mode: if is_auto {
                "autoclicker".into()
            } else {
                "work".into()
            },
            clicks_done: state.scheduler.get_clicks_done(),
            cps: state.scheduler.get_config().cps,
            status_text,
        },
        stability: crate::stability::status(),
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

/// Path of the shared Rust/JS log. One definition so the panic hook and the
/// regular logger can never drift to different files.
pub fn log_file_path() -> std::path::PathBuf {
    std::env::temp_dir().join("nanoclick_web.log")
}

/// Rewrite a panic into a log record. Pure on purpose: the hook itself is
/// process-global and cannot be exercised in a parallel test run.
fn format_panic_record(
    thread_name: &str,
    location: Option<(&str, u32, u32)>,
    payload: &str,
    backtrace: &str,
) -> String {
    let at = match location {
        Some((file, line, column)) => format!("{file}:{line}:{column}"),
        None => "<unknown location>".to_string(),
    };
    format!(
        "[RUST PANIC] thread={thread_name} at {at}\n\
         [RUST PANIC] message: {payload}\n\
         [RUST PANIC] backtrace:\n{backtrace}\n\
         [RUST PANIC] end\n"
    )
}

/// Install a panic hook that appends location, message, thread name and a
/// forced backtrace to the shared log, then runs the previous hook (stderr
/// output and the process-abort policy stay unchanged).
///
/// `force_capture` ignores `RUST_BACKTRACE`: a panic in the field must leave
/// a trace without asking the user to reproduce it with an env var. A native
/// fault (0xC0000005, an OOM kill) never reaches a Rust hook — that remains
/// WER / Event Viewer territory.
pub fn install_panic_logger() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("<unnamed>");
        let location = info.location().map(|l| (l.file(), l.line(), l.column()));
        let payload = info
            .payload_as_str()
            .unwrap_or("<non-string panic payload>");
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();
        let record = format_panic_record(name, location, payload, &backtrace);
        // Direct append, NOT `write_log_bytes_internal`: a panic raised while
        // that function holds `DEBUG_LOG_FILE` would deadlock re-entering it.
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_file_path())
        {
            let _ = file.write_all(record.as_bytes());
            let _ = file.flush();
        }
        previous(info);
    }));
}

fn write_log_bytes_internal(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let path = log_file_path();
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

/// Teardown that runs BEFORE the NSIS installer takes over (Mine 4).
///
/// The JS updater path (one-shot plugin helper) builds its own
/// `UpdaterBuilder` inside the plugin (`commands.rs:51`), so a Rust-registered
/// `Builder::on_before_exit` never applies to it — the only hook that runs
/// there is Tauri's own `cleanup_before_exit()` (windows + webviews). Calling
/// this command between `download()` and `install()` closes the gap:
/// tray icon removed (no orphan icon left in the notification area), clicker
/// stopped, LL hooks / recorder hooks / foreground patrol down — before the
/// plugin's `process::exit(0)`.
///
/// Deliberately NOT `shutdown_application`: windows stay alive (the installer
/// kills the process itself) and no `app.exit()` is issued. If the install
/// fails after this ran, the session is over anyway — the user relaunches.
pub(crate) fn teardown_before_nsis_install() {
    // Latch first: nothing else may start work (tray restore, watcher save)
    // while the installer owns the process image.
    SHUTTING_DOWN.store(true, Ordering::Release);
    stop_tray();
    // NOTE: the scheduler stop needs the managed AppState, so it lives in
    // the `prepare_nsis_update` command below; this helper owns only the
    // handle-free parts.
    crate::platform::default_input_backend_hotkey_stop();
    crate::platform::stop_recorder_hooks();
    crate::platform::windows::stop_foreground_watch();
    if let Some(exec) = crate::core::global() {
        exec.stop();
    }
    debug_log_internal(
        "warn",
        "[Updater] pre-install teardown done (tray/hooks/executor down)",
    );
}

/// JS-facing wrapper: stop the scheduler through the managed state, then run
/// the handle-free teardown above. Call after `download()` finished,
/// before `install()`.
#[tauri::command]
fn prepare_nsis_update(app: AppHandle) -> Result<(), String> {
    if let Some(state) = app.try_state::<AppState>() {
        state.scheduler.set_active(false, None);
    }
    // Best-effort final config flush so the new version boots from truth.
    if let Some(state) = app.try_state::<AppState>() {
        let cfg = state.config_manager.load();
        crate::defaults::refresh_last_good_snapshot(&state.config_manager.config_path(), &cfg);
    }
    teardown_before_nsis_install();
    Ok(())
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
    let ripples = crate::overlay::overlay_diag_dump();
    let total = hotkeys.len() + toggles.len() + ripples.len();
    debug_log_internal(
        "warn",
        &format!(
            "[Diag] input dump: hotkey_lines={} toggle_lines={} ripple_lines={}",
            hotkeys.len(),
            toggles.len(),
            ripples.len(),
        ),
    );
    for line in hotkeys {
        debug_log_internal("warn", &format!("[Diag] hotkey: {line}"));
    }
    for line in toggles {
        debug_log_internal("warn", &format!("[Diag] toggle: {line}"));
    }
    for line in ripples {
        debug_log_internal("warn", &format!("[Diag] ripple: {line}"));
    }
    #[cfg(target_os = "windows")]
    {
        debug_log_internal(
            "warn",
            &format!(
                "[Diag] foreground: undelivered_total={}",
                crate::platform::windows::foreground_watch::foreground_events_undelivered_total()
            ),
        );
    }
    debug_log_internal("warn", "[Diag] end of input dump");
    total
}

#[tauri::command]
fn exit_app(app: AppHandle) {
    // A page being suspended calls this from its `beforeunload` fallback. That
    // unload is deliberate (deep sleep), so the call must not kill the backend
    // we just decided to keep alive.
    //
    // Two guards cover the window between "WebViews destroyed" and "new page booted":
    //
    // `TRAY_SUSPEND_ARMED` is set at the start of `suspend_main_webview_to_tray`
    // and is NOT cleared until `frontend_ready()` fires on the *new* page. This
    // closes the race where WebView2 delivers `beforeunload` asynchronously, after
    // `destroy()` returned but before `frontend_ready` — that window used to be
    // unguarded once `restore_main_window` had cleared `MAIN_IN_TRAY`.
    //
    // `MAIN_IN_TRAY` covers the plain minimize-to-tray path (no deep sleep) where
    // the page is never destroyed and `TRAY_SUSPEND_ARMED` is never set.
    //
    // A page-driven exit is never legitimate while the interface lives in the tray;
    // the tray's own Quit calls `shutdown_application` directly.
    if TRAY_SUSPEND_ARMED.load(Ordering::Acquire) || MAIN_IN_TRAY.load(Ordering::Acquire) {
        debug_log_internal(
            "info",
            "[Tray] exit_app ignored: the interface lives in the tray",
        );
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
// `app.restart()` keeps working without a special case here. The NSIS update
// path no longer restarts through JS at all — `install()` exits the process
// itself after our `prepare_nsis_update` teardown ran.

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
        // The state the next start replays, and the satellites that follow the
        // main window back (F6: they are created here if a staggered boot path
        // skipped them because the app went to the tray first).
        persist_window_visibility(app, true);
        show_secondary_windows(app);
        if let Some(state) = app.try_state::<AppState>() {
            state.scheduler.emit_status_now(
                app,
                if state.scheduler.is_active() {
                    "RUNNING"
                } else {
                    "IDLE"
                },
            );
        }
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
        // The overlay died with the rest and is NOT restored from here: the
        // rebuild path owns no second copy of that decision. `frontend_ready`
        // shows the window and then calls `show_secondary_windows`, which is the
        // one place that knows whether ripples are wanted right now.
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
        // Vetoed (Work Mode / typing guard / stability interlock): the clicker
        // deliberately ignored the menu item. Tell the page, so it can explain
        // itself with a toast instead of looking broken.
        let reason = match outcome {
            scheduler::ToggleOutcome::BlockedByTyping => "typing",
            scheduler::ToggleOutcome::IgnoredWorkMode => "work_mode",
            scheduler::ToggleOutcome::BlockedByStability => {
                crate::stability::start_refusal().unwrap_or("stability")
            }
            _ => "blocked",
        };
        report_tray_action(
            app,
            match reason {
                "typing" => "Start blocked: you are typing",
                "work_mode" => "Start blocked: Work Mode is active",
                "release_pending" => "Start blocked: previous run is still releasing hardware",
                "engine_stuck" => "Start blocked: click engine is stuck (stop, then start)",
                "memory_soft" | "memory_hard" => "Start blocked: memory pressure",
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
    // Same bulkhead as save_app_config: the tray writes behind the page's
    // back, so a parallel save must not interleave. Scoped: commit on Ok,
    // explicit abort on Err.
    let node = crate::stability::config_node();
    let outcome: Result<bool, snubber::SnubberError<String>> = node.scoped(
        || {},
        || -> Result<bool, String> {
            let mut cfg = state.config_manager.load();
            let show = !cfg.ui.show_hud;
            cfg.ui.show_hud = show;
            state.config_manager.save(&cfg)?;
            Ok(show)
        },
    );
    let show = match outcome {
        Ok(v) => v,
        Err(snubber::SnubberError::Admission(_)) => {
            report_tray_action(app, "Settings busy — try again in a second");
            return;
        }
        Err(snubber::SnubberError::Work(e)) => {
            debug_log_internal("warn", &format!("[Tray] HUD setting not persisted: {e}"));
            return;
        }
        // `#[non_exhaustive]`: unknown variant — the write did not land, so
        // report exactly like a refused admission.
        Err(other) => {
            report_tray_action(app, "Settings busy — try again in a second");
            debug_log_internal(
                "warn",
                &format!("[Tray] HUD setting not persisted: {other}"),
            );
            return;
        }
    };
    // The facade owns the visibility gate: it answers DeferredTray when the
    // main window is away. No second predicate here — it would desync from
    // the toggle's own answer.
    match toggle_hud_window(app.clone(), show) {
        Ok(overlay::SatelliteToggle::DeferredTray) => {
            // No window was built: the stored preference is the whole result,
            // and `restore_main_window` builds the HUD later.
            emit_satellite_toggle_result(app, "hud", false, "tray");
            emit_tray_hud_changed(app, show);
            report_tray_action(app, "HUD will appear with the window");
            debug_log_internal("info", &format!("[Tray] HUD -> {show} (deferred)"));
        }
        Ok(_) => {
            emit_tray_hud_changed(app, show);
            report_tray_action(app, if show { "HUD shown" } else { "HUD hidden" });
            debug_log_internal("info", &format!("[Tray] HUD -> {show}"));
        }
        Err(e) => {
            // Satellite bulkhead busy: the preference was written above but no
            // window exists. Roll it back so disk and screen agree, lock the
            // page checkbox through the refusal event, and report once.
            debug_log_internal("warn", &format!("[Tray] HUD toggle refused: {e}"));
            if let Err(e) = persist_hud_preference(&state, !show) {
                debug_log_internal(
                    "error",
                    &format!("[Tray] HUD preference rollback failed: {e}"),
                );
            }
            emit_satellite_toggle_result(app, "hud", false, "busy");
            report_tray_action(app, "HUD is busy — try again");
        }
    }
}

/// Persist `ui.show_hud` under the config bulkhead. Used to undo the tray's
/// optimistic write when the satellite toggle was refused.
fn persist_hud_preference(state: &AppState, show: bool) -> Result<(), String> {
    let node = crate::stability::config_node();
    let outcome: Result<(), snubber::SnubberError<String>> = node.scoped(
        || {},
        || -> Result<(), String> {
            let mut cfg = state.config_manager.load();
            cfg.ui.show_hud = show;
            state.config_manager.save(&cfg)
        },
    );
    match outcome {
        Ok(()) => Ok(()),
        Err(snubber::SnubberError::Admission(_)) => Err("settings busy — retry".into()),
        Err(snubber::SnubberError::Work(e)) => Err(e),
        // `#[non_exhaustive]`: unknown variant — nothing was committed.
        Err(other) => Err(format!("settings busy — retry ({other})")),
    }
}

/// The BEHAVIOR checkbox mirrors the tray write WITHOUT saving: a page-side save
/// would write its stale checkbox back to disk and undo the menu choice.
fn emit_tray_hud_changed(app: &AppHandle, show: bool) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.emit("tray-hud-changed", show);
    }
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
    // Same bulkhead: the mode flip persists one field — never under a save.
    // Scoped: commit on Ok, explicit abort on Err.
    let node = crate::stability::config_node();
    let outcome: Result<String, snubber::SnubberError<String>> = node.scoped(
        || {},
        || -> Result<String, String> {
            // The scheduler emits `status-update` itself, so a live page follows along.
            let new_mode = state.scheduler.toggle_mode(Some(app));
            let mut cfg = state.config_manager.load();
            cfg.active_mode = new_mode.clone();
            state.config_manager.save(&cfg)?;
            Ok(new_mode)
        },
    );
    let new_mode = match outcome {
        Ok(v) => v,
        Err(snubber::SnubberError::Admission(_)) => {
            report_tray_action(app, "Settings busy — try again in a second");
            if state.scheduler.is_autoclicker_mode() {
                "autoclicker".to_string()
            } else {
                "work".to_string()
            }
        }
        Err(snubber::SnubberError::Work(e)) => {
            debug_log_internal("warn", &format!("[Tray] mode not persisted: {e}"));
            if state.scheduler.is_autoclicker_mode() {
                "autoclicker".to_string()
            } else {
                "work".to_string()
            }
        }
        // `#[non_exhaustive]`: unknown variant — the flip may or may not have
        // happened, so report the live scheduler state, never a guess.
        Err(other) => {
            debug_log_internal("warn", &format!("[Tray] mode not persisted: {other}"));
            if state.scheduler.is_autoclicker_mode() {
                "autoclicker".to_string()
            } else {
                "work".to_string()
            }
        }
    };
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

/// Is the HUD preference currently on?
///
/// Same rule as `tray_work_mode_active`: the persisted flag only, never a
/// window handle — the tray pump calls this while opening the menu.
pub(crate) fn tray_hud_visible(app: &AppHandle) -> bool {
    app.try_state::<AppState>()
        .map(|s| s.config_manager.load().ui.show_hud)
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

/// A foreground switch onto an elevated (UIPI) window while automation is
/// live — called on the Tauri main thread from the passive foreground
/// patrol (`platform::windows::foreground_watch`).
///
/// Invariant: a window the user placed in the tray is never shown again by a
/// background block. Tray/deep-sleep gets `MessageBeep` (already played
/// by the caller) + `NotificationStore` entry (already pushed) + a tray
/// balloon; the UIPI modal waits inside the hidden page for the user's own
/// return (`syncPendingNotifications`). ONLY a window that is already on
/// screen (`main_window_visible`) gets the Z-ram: `unminimize + show +
/// set_focus`, so the alert lands in front of a windowed game instead of
/// blinking orange behind it. No `set_always_on_top` games: a sticky topmost
/// over a crosshair is worse than a blink.
pub(crate) fn foreground_alert_on_elevated_focus(app: &AppHandle) {
    if !main_window_visible(app) {
        // `MAIN_IN_TRAY` outranks everything: with deep sleep there is no
        // WebView at all, and building one here would respawn the whole
        // `msedgewebview2.exe` tree (~120 MB) for a popup nobody asked for.
        tray::notify_balloon(
            "NanoClick",
            "Focused window runs elevated (UIPI): clicks will be blocked",
        );
        debug_log_internal(
            "warn",
            "[UIPI] elevated focus while the window lives in the tray (balloon, window stays hidden)",
        );
        return;
    }
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
        debug_log_internal(
            "warn",
            "[UIPI] elevated focus with the window on screen (brought forward, no topmost latch)",
        );
    }
}

/// A click blocked by UIPI while the interface lives in the tray.
///
/// The page reacts to a block with a chime plus a modal **inside the main window**,
/// and it used to call `show()` so that modal would be readable — which dragged the
/// window back out of the tray on every blocked click, undoing a close the user had
/// asked for. The page now asks here instead, and the alert goes through the tray
/// balloon: the same channel every other action uses when the interface is not on
/// screen. The page's own modal still opens, so the user sees it when they come back.
#[tauri::command]
fn uipi_blocked_in_tray() {
    // No `AppHandle` needed: `notify_balloon` writes through the tray's own
    // cross-thread icon copy, and a missing icon makes it a no-op.
    tray::notify_balloon(
        "NanoClick",
        "Click blocked: the target window runs elevated (UIPI)",
    );
    debug_log_internal(
        "warn",
        "[UIPI] blocked click while the window lives in the tray (balloon, window stays hidden)",
    );
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
                // A reload recreates the page, but the watchdog countdown belongs
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
/// removed first so Windows never has to reap an orphan one.
pub(crate) fn restart_app_from_tray(app: &AppHandle) {
    debug_log_internal("warn", "[Tray] restart requested from the menu");
    report_tray_action(app, "Restarting…");
    // Latch the shutdown flag for the teardown paths that consult it; tauri
    // ignores the restart code in `prevent_exit`, so the restart path works.
    SHUTTING_DOWN.store(true, Ordering::Release);
    stop_tray();
    app.restart();
}

// ── Window state across restarts, and the satellites' visibility rule ─────
//
// "Reopen the way I closed it" is a BACKEND feature, not a page one: the page is
// gone exactly when the app lives in the tray, and gone again while the process
// quits. The flag is therefore stamped from the live window — never from a
// payload — on every hide/show transition and at the start of a shutdown.

/// True while the app intends the interface to live in the tray (hidden, or
/// destroyed by deep sleep). The in-memory mirror of `ui.window_was_visible == false`,
/// and the reason `save_app_config` cannot be trusted with a live read alone: the
/// page's close-time flush (`__nanoclick_tray_flush__` → `saveConfig()`) runs
/// BEFORE the hide, so the window still reports visible and a naive stamp would
/// overwrite the very decision we had just taken — which is how "close to the tray"
/// turned into "reopens on the next start".
static MAIN_IN_TRAY: AtomicBool = AtomicBool::new(false);

/// Effective "start hidden" decision for a boot.
///
/// An explicit `start_minimized` ("always start hidden") wins; otherwise
/// "reopen the way I closed it" replays the last observed state.
fn start_hidden_at_boot(ui: &config_manager::UiSettings) -> bool {
    ui.start_minimized || (ui.remember_last_window_state && !ui.window_was_visible)
}

/// Is the main window on screen right now?
///
/// The single gate every secondary window (floating HUD, ripple overlay)
/// consults before it shows itself: they are satellites of the main window and
/// must never be left on the desktop without a visible owner. `false` on any
/// doubt — an absent or unreadable window is not a visible one.
pub(crate) fn main_window_visible(app: &AppHandle) -> bool {
    app.get_webview_window("main")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false)
}

/// Persist "the window was / was not visible", so the next start can replay it.
///
/// A no-op when nothing changed: a tray round trip must not rewrite the config
/// file. It marks its own write, so the config observer never toasts our save.
fn persist_window_visibility(app: &AppHandle, visible: bool) {
    // Mirror the decision FIRST, before the "nothing changed" shortcut below: the
    // page may already have a flush in flight that will ask `save_app_config`, and
    // that call has to see "the app is going to the tray" even though this very
    // window is still on screen for another few hundred milliseconds.
    MAIN_IN_TRAY.store(!visible, Ordering::Release);
    // Same insurance as the page's save: the backend can overwrite the config too.
    ensure_session_config_backup(app);
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    // Bulkhead with skip-on-busy: the in-memory flag above is already correct,
    // and the next transition (or the next save_app_config, which stamps the
    // same field from the live window) will persist it. Losing this race must
    // never block the tray pump. Scoped: commit on Ok, explicit abort on Err.
    let node = crate::stability::config_node();
    let outcome: Result<bool, snubber::SnubberError<String>> = node.scoped(
        || {},
        || -> Result<bool, String> {
            let mut cfg = state.config_manager.load();
            if cfg.ui.window_was_visible == visible {
                return Ok(false);
            }
            cfg.ui.window_was_visible = visible;
            state.config_manager.save(&cfg)?;
            Ok(true)
        },
    );
    let changed = match outcome {
        Ok(v) => v,
        Err(snubber::SnubberError::Admission(_)) => return,
        Err(snubber::SnubberError::Work(e)) => {
            debug_log_internal("warn", &format!("[Tray] window state not persisted: {e}"));
            return;
        }
        // `#[non_exhaustive]`: the in-memory flag is already correct and the
        // next transition re-stamps it — same skip-on-busy behaviour.
        Err(other) => {
            debug_log_internal(
                "warn",
                &format!("[Tray] window state not persisted: {other}"),
            );
            return;
        }
    };
    if !changed {
        return;
    }
    // Marked inside `save()` — no manual call needed.
    debug_log_internal(
        "info",
        &format!("[Tray] remembered window state: visible={visible}"),
    );
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

/// Bring the HUD / overlay back with the main window.
///
/// Mirror of `hide_secondary_windows`, plus the one thing a restore cannot
/// assume: the window may not exist at all (deep-sleep rebuild, or a staggered
/// boot path that skipped its work because the app went to the tray first). Only
/// the windows the config still wants are created/shown, and only while the main
/// window is actually on screen — the caller shows it first, then calls this.
fn show_secondary_windows(app: &AppHandle) {
    // Only EXISTENCE is checked here, not visibility: this is called by the paths
    // that just showed the main window (`restore_main_window`, `frontend_ready`),
    // and `show()` dispatches to the main thread — an `is_visible()` read from a
    // foreign thread can still see `false` a moment later. A satellite created
    // here stays hidden until its own ready signal, which DOES require a visible
    // main window (`hud_ready` / `overlay_ready`).
    if app.get_webview_window("main").is_none() {
        return;
    }
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let cfg = state.config_manager.load();
    if cfg.ui.visual_ripple {
        if app.get_webview_window("overlay").is_none() {
            // Created hidden through the single-owner facade; `overlay_ready`
            // shows it once the transparent DOM has rendered (no white flash).
            let _ = overlay::ensure_satellite(app, "overlay");
        } else if let Some(win) = app.get_webview_window("overlay") {
            let _ = win.show();
        }
    }
    if cfg.ui.show_hud {
        if app.get_webview_window("hud").is_none() {
            let _ = toggle_hud_window(app.clone(), true);
        } else if let Some(win) = app.get_webview_window("hud") {
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
    // The new page has fully booted. Clear the deep-sleep guard that was
    // keeping `exit_app` blocked since `suspend_main_webview_to_tray`.
    //
    // Why here and not right after `w.destroy()`:
    // WebView2 fires `beforeunload` asynchronously. A late event from the
    // destroyed WebView arrives after `destroy()` returns, but before the
    // rebuilt window has booted. If the flag were cleared at destroy-time,
    // that `beforeunload` → `exit_app` would find both guards false and
    // execute a real shutdown (measured: "press hotkey, app exits completely").
    // Clearing here is safe — the new page is live and any `exit_app` from
    // this point forward is a genuine user request.
    //
    // On a normal (non-deep-sleep) boot `TRAY_SUSPEND_ARMED` is already
    // false, so this store is a no-op.
    TRAY_SUSPEND_ARMED.store(false, Ordering::Release);
    // A window rebuilt after deep sleep waits for this moment to become visible:
    // showing a fresh WebView earlier would paint the DWM white rectangle.
    if SHOW_ON_READY.swap(false, Ordering::AcqRel) {
        if let Some(win) = app.get_webview_window("main") {
            let _ = win.show();
            let _ = win.set_focus();
            // The window is on screen now, so the satellites may follow it back —
            // created here if the staggered boot path skipped its work (the app
            // went to the tray before it woke up). One owner for that decision.
            persist_window_visibility(&app, true);
            show_secondary_windows(&app);
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
                            // Shown anyway, so this is the state the next start
                            // replays. No satellites: the page is broken, and a
                            // floating counter over a broken app is noise.
                            persist_window_visibility(&app, true);
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
    let macro_running = crate::core::global()
        .map(|e| e.is_running())
        .unwrap_or(false);
    let recording = app
        .try_state::<crate::commands::MacroState>()
        .map(|s| s.recorder.lock().map(|slot| slot.is_some()).unwrap_or(true))
        .unwrap_or(false);
    deep_sleep_allowed(enabled, clicking, macro_running, recording)
}

/// Why deep sleep is not available right now, for the log.
///
/// A silent `-> hidden to tray` is indistinguishable from a broken switch: the user
/// closes the window, 120 MB stay resident, and nothing anywhere says that deep
/// sleep was skipped on purpose. Same four inputs as `deep_sleep_allowed_now` —
/// this is the explanation, that one is the decision.
fn deep_sleep_skip_reason(app: &AppHandle) -> &'static str {
    let Some(state) = app.try_state::<AppState>() else {
        return "app state unavailable";
    };
    if !state.config_manager.load().ui.deep_sleep_to_tray {
        return "deep sleep is OFF in the config";
    }
    if state.scheduler.is_active() {
        return "the clicker is running";
    }
    if crate::core::global()
        .map(|e| e.is_running())
        .unwrap_or(false)
    {
        return "a macro is running";
    }
    let recording = app
        .try_state::<crate::commands::MacroState>()
        .map(|s| s.recorder.lock().map(|slot| slot.is_some()).unwrap_or(true))
        .unwrap_or(false);
    if recording {
        return "a recording is in progress";
    }
    "unknown (state changed between the check and the log)"
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
    // The app is going to the tray: that is the state the next start replays.
    persist_window_visibility(app, false);
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
            // Satellite teardown through the single-owner facade (SATELLITE_PLAN §1):
            // no direct `WebviewWindowBuilder` / `destroy` outside overlay.rs.
            for label in ["hud", "overlay"] {
                crate::overlay::destroy_satellite(&app_main, label);
            }
            if let Some(w) = app_main.get_webview_window("main") {
                let _ = w.destroy();
            }
            // NOTE: TRAY_SUSPEND_ARMED is NOT cleared here.
            //
            // WebView2 fires `beforeunload` asynchronously — the event can arrive
            // on the Tauri dispatch thread *after* `destroy()` returns. If we clear
            // the flag here, a late `beforeunload` → `exit_app` call finds both
            // `TRAY_SUSPEND_ARMED = false` and `MAIN_IN_TRAY = false` (the window
            // was already restored by `restore_main_window`) and executes a real
            // shutdown — exactly the "presses 5, app closes" symptom.
            //
            // The flag is cleared in `frontend_ready()` instead, once the new page
            // has fully booted. Any `exit_app` that arrives in the gap is correctly
            // treated as "the interface lives in the tray" and ignored.
            debug_log_internal(
                "info",
                &format!("[Tray] deep sleep: WebViews destroyed (flush acked: {acked})"),
            );
        });
    });
}

pub(crate) fn shutdown_application(app: &AppHandle) {
    // Idempotent. Destroying the main window fires its `beforeunload`, and the
    // page's fallback calls `exit_app` — which lands right back here. The first
    // caller wins; a re-entry must not destroy/exit twice or spawn a second exit
    // watchdog. `ExitRequested` reads the same latch, so it is already satisfied.
    if SHUTTING_DOWN.swap(true, Ordering::AcqRel) {
        debug_log_internal("info", "[Shutdown] already in progress; ignoring re-entry");
        return;
    }
    debug_log_internal("info", "[Shutdown] Initiating clean application shutdown");

    // 0. Remember the state we are leaving in — before a single window goes away,
    //    so "reopen the way I closed it" has something true to replay.
    let was_visible = main_window_visible(app);
    persist_window_visibility(app, was_visible);

    // 1. Drop the tray icon before the windows go away, so Windows never has to
    //    reap an orphan icon and the pump thread is joined while state is alive.
    stop_tray();

    // 2. STOP THE BACKEND FIRST — strictly before any window is destroyed. This is
    //    the reverse of the startup order on purpose: the native LL hooks and the
    //    clicker must no longer generate or catch events when the WebView that
    //    receives their status emissions starts tearing down its message loop and
    //    COM objects. A hook firing `emit` into a half-destroyed window is how a
    //    process ends up holding resources it can no longer release.
    if let Some(state) = app.try_state::<AppState>() {
        state.scheduler.set_active(false, None);
    }
    platform::default_input_backend_hotkey_stop();
    platform::stop_recorder_hooks();
    platform::windows::stop_foreground_watch();
    if let Some(exec) = crate::core::global() {
        exec.stop();
    }

    // 2b. LAST CONFIG WRITE OF THE SESSION: pin the golden snapshot to the state
    //     we are leaving with. The read path no longer promotes on every load
    //     (that meant ~40 rewrites a minute while the app sat idle in the tray)
    //     and the app-profile thread stops touching the config as soon as
    //     SHUTTING_DOWN latches, so this is the place that guarantees "the
    //     snapshot is never older than the last run". Step 0 already wrote the
    //     final window state into config.json, so the snapshot captures it too.
    //     A `taskkill` skips this by definition — then the snapshot from the last
    //     successful save stands, which is the honest fallback.
    if let Some(state) = app.try_state::<AppState>() {
        let cfg = state.config_manager.load();
        crate::defaults::refresh_last_good_snapshot(&state.config_manager.config_path(), &cfg);
        debug_log_internal(
            "info",
            "[Shutdown] golden snapshot refreshed from the live config",
        );
    }

    // 3. Only now destroy the windows: satellites first, `main` LAST (it is the
    //    one the hooks and the page talk to). `hide()` is not enough here — it
    //    leaves the WebView2 helpers alive until the process image disappears
    //    (measured: hiding frees nothing, 122.8 MB stays resident).
    //    Single-owner destructor: no snubber guard (a toggle thread may hold
    //    it while blocked on the event loop — taking it here deadlocks).
    for label in ["overlay", "hud"] {
        crate::overlay::destroy_satellite(app, label);
    }
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.destroy();
    }

    // 3b. Hardware: release anything the workers left down (idempotent, and it
    //     covers a macro that ended on `MouseDown`). In-process first — the
    //     guardian is only the fallback for exits that never reach this code.
    crate::stability::emergency_release_all_hardware();

    // 3c. Cold evidence for the NEXT boot: final `.snub` frames plus
    //     `%TEMP%/nanoclick_triage_<role>.txt` (per-node triage cards and the
    //     diff against this run's boot baseline). A `taskkill` skips this by
    //     definition; the ≤30 s periodic frame then stands as the evidence.
    crate::stability::shutdown_snapshot_cycle();

    // 4. This is a deliberate exit: tell the input guardian NOT to inject.
    input_guardian::mark_clean_exit();
    app.exit(0);

    // 5. Watchdog: give 300ms for clean background file I/O flush, then trim working set
    // memory and guarantee complete process termination so no background process survives.
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
fn toggle_hud_window(app: AppHandle, show: bool) -> Result<overlay::SatelliteToggle, String> {
    use overlay::SatelliteToggle;
    // Scoped admission through the satellite bulkhead: a refused create is a
    // planned abort, never a breaker trip.
    //
    // **Documented exception to the guide's "short action (<= 50 ms) -> scoped"
    // rule**: the guarded work is a WebView2 create/show/destroy, measured in
    // hundreds of ms (seconds on a cold start). `scoped` is correct anyway —
    // the slot must be HELD for the whole build so a parallel ripple toggle
    // cannot double-`build()`; `enter_auto` + `commit()` spans the same window.
    // The span is logged, not assumed (see the `[HUD] toggle scope` line).
    let node = crate::stability::satellite_node();
    let app_rb = app.clone();
    let scope_started = std::time::Instant::now();
    let outcome: Result<SatelliteToggle, snubber::SnubberError<String>> = node.scoped(
        move || {
            if let Some(win) = app_rb.get_webview_window("hud") {
                let _ = win.destroy();
            }
        },
        || toggle_hud_window_inner(app, show),
    );
    debug_log_internal(
        "info",
        &format!(
            "[HUD] toggle scope: show={show} ok={} elapsed_ms={}",
            outcome.is_ok(),
            scope_started.elapsed().as_millis()
        ),
    );
    match outcome {
        Ok(v) => Ok(v),
        Err(snubber::SnubberError::Admission(_)) => {
            Err("satellite busy — toggle in progress".into())
        }
        Err(snubber::SnubberError::Work(e)) => Err(e),
        // `#[non_exhaustive]`: unknown variant — no window was built.
        Err(other) => Err(format!("satellite busy — toggle in progress ({other})")),
    }
}

/// Inner HUD toggle (no admission: the scope lives on `toggle_hud_window`).
fn toggle_hud_window_inner(app: AppHandle, show: bool) -> Result<overlay::SatelliteToggle, String> {
    use overlay::SatelliteToggle;
    if show {
        // The HUD is a satellite of the main window, never standalone. While
        // the app lives in the tray (main hidden, or released by deep sleep)
        // the preference is only stored — the window is built when the main
        // window returns (`restore_main_window`). Creating it here left an
        // always-on-top counter floating over the desktop with no owner.
        if !main_window_visible(&app) {
            // Deferred is a user-visible decision (the checkbox stays off while
            // the preference is on): `warn` keeps it release-visible.
            debug_log_internal("warn", "[HUD] show deferred: main window is not visible");
            return Ok(SatelliteToggle::DeferredTray);
        }
        let already_existed = app.get_webview_window("hud").is_some();
        overlay::bump_satellite_gen("hud");
        overlay::ensure_satellite(&app, "hud")?;
        // Latch is stamped inside ensure_satellite for EVERY build path
        // (fresh, race survivor, jittered retry): no branch may skip it.
        if let Some(win) = app.get_webview_window("hud") {
            let _ = win.set_ignore_cursor_events(true);
            // Fresh WebView: hidden until hud_ready() fires after DOM render
            // (no white flash). Existing window: show immediately.
            if already_existed {
                let _ = win.show();
                Ok(SatelliteToggle::AlreadyThere)
            } else {
                debug_log_internal("info", "[HUD] hud window shown (lazy)");
                Ok(SatelliteToggle::Applied)
            }
        } else {
            Ok(SatelliteToggle::DeferredTray)
        }
    } else if let Some(win) = app.get_webview_window("hud") {
        // Session hide: the window stays alive in the session, only its
        // visibility flips. Real teardown happens exclusively on tray suspend
        // and shutdown (`destroy_satellite`).
        let gen = overlay::bump_satellite_gen("hud");
        let remaining = overlay::latch_remaining_ms("hud");
        if remaining > 0 {
            // Latch window: let COM init finish, then hide. A newer toggle
            // (ON) cancels this by bumping the generation.
            let app_hide = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(remaining));
                if overlay::satellite_gen("hud") != gen {
                    return;
                }
                if let Some(w) = app_hide.get_webview_window("hud") {
                    let _ = w.hide();
                }
            });
            debug_log_internal(
                "info",
                &format!("[HUD] hud hide deferred by latch ({remaining}ms)"),
            );
        } else {
            let _ = win.hide();
            debug_log_internal("info", "[HUD] hud window hidden (session)");
        }
        Ok(SatelliteToggle::Applied)
    } else {
        Ok(SatelliteToggle::AlreadyThere)
    }
}

#[allow(dead_code)]
fn ensure_hud_window(app: &AppHandle) -> Result<tauri::WebviewWindow, String> {
    overlay::ensure_satellite(app, "hud")
}

#[tauri::command]
fn hud_ready(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let cfg = state.config_manager.load();
    if let Some(win) = app.get_webview_window("hud") {
        let _ = win.set_ignore_cursor_events(true);
        // Only with the main window on screen. This signal arrives after the
        // HUD DOM rendered, which can be after the app moved to the tray — a
        // `show()` here would leave the floating counter on the desktop while
        // its owner reports hidden.
        if cfg.ui.show_hud && main_window_visible(&app) {
            let _ = win.show();
            debug_log_internal(
                "info",
                "[HUD] hud_ready: restored HUD visibility from config",
            );
        }
    }
    Ok(())
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateInfo {
    version: String,
    date: Option<String>,
    body: Option<String>,
    raw_json: serde_json::Value,
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
        // The portable branch reads `platforms.portable` from here, so the
        // manifest extension rides along without a second network call.
        raw_json: u.raw_json,
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
    // Preset library: seeded into its own store (`presets.json`) on the first boot of this build,
    // from the legacy `config.json.presets` array. From then on the store is the authority — a
    // config write can no longer replace the library (see `persistence::presets`). An existing
    // store is never overwritten by the config copy.
    match crate::persistence::presets::ensure_store_seeded(&initial_app_cfg.presets) {
        Ok(Some(n)) => debug_log_internal(
            "info",
            &format!("[Presets] library seeded with {n} preset(s) into presets.json"),
        ),
        Ok(None) => {}
        Err(e) => debug_log_internal("warn", &format!("[Presets] store seeding failed: {e}")),
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
    watcher.watch(
        "presets",
        crate::persistence::presets::presets_path(),
        crate::watcher::presets_bytes_valid,
    );
    let watcher_for_setup = Arc::clone(&watcher);
    // Every `ConfigManager::save()` now marks its own write, so no call site can
    // forget it (six did: onboarding, reset, HUD toggle, mode toggle, image
    // trigger, backup import — each toasted falsely on every invocation).
    config_manager.attach_observer(Arc::clone(&watcher));

    let scheduler = Arc::new(ClickScheduler::new());
    scheduler.set_config(config::Config::from(initial_app_cfg.clone()));
    scheduler.update_presets(crate::persistence::presets::load_presets());
    // A secure desktop (UAC, Win+L) swallows `SendInput`: the click thread
    // cannot see the transition and would leave buttons held on the desktop it
    // can no longer reach. The maintenance tick watches for it and calls back.
    crate::stability::set_desktop_lost_handler(Arc::new({
        let scheduler = Arc::clone(&scheduler);
        move || scheduler.stop_for_desktop_loss()
    }));

    let scheduler_for_setup = Arc::clone(&scheduler);
    let config_manager_arc = Arc::clone(&config_manager);

    let notification_store = Arc::new(NotificationStore::new());
    set_global_notification_store(Arc::clone(&notification_store));

    let app_state = AppState {
        scheduler,
        config_manager,
        // Totals are inherited from the boot config: the first `stats_tick`
        // continues the all-time counters instead of resetting them.
        session_stats: Mutex::new(crate::stats_agg::SessionStats::with_totals(
            initial_app_cfg.stats.clone(),
        )),
        startup_notices,
        file_toasts,
        notification_store,
    };

    let macro_state = commands::MacroState::default();
    let macro_executor: Arc<crate::core::ExecutorHandle> = Arc::clone(&macro_state.executor);
    crate::core::set_global((*macro_executor).clone());

    let app = match tauri::Builder::default()
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
            // Stability lifecycle, one owner per process: diagnose the PREVIOUS
            // run's `.snub` frames, stamp this run's baseline, then start the
            // single maintenance thread (200 ms pulses, 1 Hz rollback/memory,
            // 30 s snapshots). Both calls are idempotent — the daemon binary
            // starts the same maintenance for its own engine.
            crate::stability::boot_snapshot_cycle();
            crate::stability::start_maintenance();
            // Ripple drain worker: one 40 ms thread per process. Drains the
            // lock-free ripple ring pushed by the click loop and emits one
            // coalesced `spawn-ripple` batch per tick (see overlay.rs).
            crate::overlay::spawn_ripple_drain_worker(handle.clone());
            // StatusSink wiring (Stage B/C dual-write): Tauri window emits +
            // optional daemon IPC broadcast share one payload constructor, so
            // the daemon and the monolith cannot drift apart.
            crate::tauri_sink::set_extra_sink(std::sync::Arc::new(
                crate::tauri_sink::TauriSink::new(handle.clone()),
            ));
            // Stage A OOP satellites (default OFF): out-of-process overlay/hud
            // supervised child processes. Enabled by `NANOCLICK_OOP_SATELLITES=1`
            // env var or `ui.oop_satellites` config flag; otherwise the
            // in-process WebViews above stay the behaviour.
            {
                let oop_env = std::env::var("NANOCLICK_OOP_SATELLITES")
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false);
                let oop_cfg = initial_app_cfg.ui.oop_satellites.unwrap_or(false);
                if oop_env || oop_cfg {
                    crate::satellite::set_satellite_oop(true);
                    for label in [
                        crate::satellite::SATELLITE_OVERLAY,
                        crate::satellite::SATELLITE_HUD,
                    ] {
                        let h = handle.clone();
                        std::thread::Builder::new()
                            .name(format!("nanoclick-satellite-{label}"))
                            .spawn(move || crate::satellite::run_satellite_supervisor(label, h))
                            .ok();
                    }
                    debug_log_internal("info", "[Satellite] OOP supervisor enabled");
                }
            }

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
                    let start_hidden = start_hidden_at_boot(&initial_app_cfg.ui);
                    if start_hidden {
                        // Either an explicit "always start hidden", or "reopen the
                        // way I closed it" replaying an exit from the tray. The
                        // window exists (the backend needs it for HUD/position
                        // prefs) but stays hidden until the tray icon is clicked;
                        // `visible: false` in tauri.conf.json means nothing was
                        // painted before this point, so there is no flash.
                        let _ = win.hide();
                        debug_log_internal(
                            "info",
                            "[Startup] starting in the tray (start_minimized / last state)",
                        );
                    } else {
                        let _ = win.show();
                        let _ = win.set_focus();
                    }
                    // Store what we actually did: a start already hidden is a state
                    // the next "reopen the way I closed it" must be able to replay.
                    persist_window_visibility(&handle, !start_hidden);
                } else {
                    debug_log_internal("error", "[Startup] main window MISSING from registry");
                }
            }

            // ── System tray (native Win32; no tray-icon/muda crate) ────────
            // Only when the feature is reachable: with "close to tray" off, "start
            // minimized" off and no remembered tray state, the app never hides, so
            // an icon would be dead weight (one more window handle + one more
            // thread). "Reopen the way I closed it" counts: this start may be a
            // hidden one, and then the icon is the only way back in.
            if initial_app_cfg.ui.minimize_to_tray
                || initial_app_cfg.ui.start_minimized
                || initial_app_cfg.ui.remember_last_window_state
            {
                start_tray_if_needed(&handle);
            }

            // Backend-side heartbeat check for THIS window (Phase E): a page that
            // never reports ready is logged and reloaded once, and a window that
            // is still hidden gets shown so the in-page boot banner is visible.
            spawn_frontend_watchdog(handle.clone());

            // "Start hidden" + deep sleep is the whole point of autostart: be
            // weightless. Wait for the page to report ready (its flush hook must
            // exist before the WebView can be torn down), then let it go. This
            // covers an explicit "always in the tray" and a replayed tray exit.
            if start_hidden_at_boot(&initial_app_cfg.ui) && initial_app_cfg.ui.deep_sleep_to_tray {
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
            // Three gates, cheapest first: (1) an mtime check on `config.json`
            // — no config touch while the file is untouched; (2) a parse only on
            // a real change, kept only when the *relevant* rules differ, so a
            // changed CPS/theme never costs a foreground lookup; (3) the
            // foreground match itself.
            //
            // MEASURED PROBLEM (2026-09-27): this loop called `cm.load()` every
            // 1500 ms UNCONDITIONALLY — even with the factory's empty
            // `app_profiles` — and `load()` is not a read: it parses the file and
            // (until the same day) rewrote `config.last_good.json` on every call.
            // That was ~40 disk writes and two JSON parses a minute in a process
            // that claims to be asleep in the tray, and it is what showed up as
            // the idle CPU/disk sawtooth.
            //
            // `config.json` stays the single source of truth: the vector below is
            // a cache of what the last read said, never a second store
            // (AGENTS.md I5). A rule that gets re-armed fires again, because the
            // cache is dropped the moment the rule set changes.
            {
                let cm = config_manager_arc.clone();
                let h = handle.clone();
                // `spawn_guarded` binds admission + rollback + leash in one
                // call: a panic in this loop is a recorded implicit drop, not a
                // silently dead thread. The loop is infinite, so the join is
                // watched by a tail thread that only speaks on refusal.
                let (join, _steer) = crate::stability::profile_switch_node().spawn_guarded(
                    || {},
                    (),
                    move |_conduit: snubber::TaskConduit<(), ()>| {
                        let config_path = cm.config_path();
                        // (title_contains, preset_id) of the rules we act on.
                        let mut profiles: Vec<(String, String)> = Vec::new();
                        let mut last_seen_mtime: Option<std::time::SystemTime> = None;
                        let mut last_preset: Option<String> = None;
                        loop {
                            std::thread::sleep(std::time::Duration::from_millis(1500));
                            // The shutdown flush owns the last write; do not race it.
                            if SHUTTING_DOWN.load(Ordering::Acquire) {
                                return;
                            }
                            // Gate 1 + 2: only when `config.json` actually moved do we
                            // parse it — and only a changed RULE SET invalidates the
                            // cached list (a changed CPS/theme must not cost anything).
                            // The gate removes the config work, NOT the window match:
                            // matching the foreground title every tick is the feature.
                            let mtime = std::fs::metadata(&config_path)
                                .and_then(|m| m.modified())
                                .ok();
                            if !(mtime.is_some() && mtime == last_seen_mtime) {
                                last_seen_mtime = mtime;
                                let cfg = cm.load();
                                let current: Vec<(String, String)> = cfg
                                    .app_profiles
                                    .into_iter()
                                    .filter(|p| p.enabled && !p.title_contains.is_empty())
                                    .map(|p| (p.title_contains, p.preset_id))
                                    .collect();
                                if current != profiles {
                                    profiles = current;
                                    // A newly (re-)armed rule must be able to fire again.
                                    last_preset = None;
                                }
                            }
                            if profiles.is_empty() {
                                continue;
                            }
                            // Gate 3: the foreground lookup, once per tick, no I/O.
                            let Some(title) = platform::get_foreground_window_title() else {
                                continue;
                            };
                            let lower = title.to_lowercase();
                            for (needle, preset_id) in &profiles {
                                if lower.contains(&needle.to_lowercase()) {
                                    if last_preset.as_deref() != Some(preset_id.as_str()) {
                                        last_preset = Some(preset_id.clone());
                                        let _ = h.emit("app-profile-activate", preset_id.clone());
                                    }
                                    break;
                                }
                            }
                        }
                    },
                );
                std::thread::Builder::new()
                    .name("nanoclick-profile-guard".into())
                    .spawn(move || {
                        if let Ok(Err(e)) = join.join() {
                            crate::debug_log_internal(
                                "error",
                                &format!("[Stability] profile_switch admission refused: {e:?}"),
                            );
                        }
                    })
                    .ok();
            }
            // v4.2 — start hotkeys through the HotkeyBackend contract.
            let hk = platform::default_hotkey_backend(scheduler_for_setup, handle.clone());
            if let Err(e) = hk.start() {
                crate::debug_log_internal("warn", &format!("[Hotkeys] backend start failed: {e}"));
            }
            // Passive foreground patrol: one Win32 event per foreground switch
            // (0% CPU between switches). Warns when automation is live and the
            // user lands on an elevated window; the tray window stays hidden.
            platform::windows::start_foreground_watch();
            // Radar ping: tray-aware update telemetry (Worker proxy + silent
            // GitHub fallback). Own thread, own cadence, zero hot-path touch.
            crate::platform::update_ping::start_radar_ping();
            // LAZY secondary WebViews: cold boot = main window ONLY.
            // Satellites are restored here (staggered, off the critical boot
            // path) so persisted visual_ripple / show_hud keep working without
            // any pre-created WebView in tauri.conf.json.
            // `show_secondary_windows` is the SINGLE owner for that decision
            // (same as tray restore and post-deep-sleep `frontend_ready`): it
            // re-reads the config after the sleep, so a flag turned off meanwhile
            // cannot leak a hidden WebView, and it creates nothing while the main
            // window is not on screen. overlay_ready/hud_ready show each window
            // once its transparent DOM has rendered.
            if initial_app_cfg.ui.visual_ripple || initial_app_cfg.ui.show_hud {
                let h = handle.clone();
                std::thread::spawn(move || {
                    // Let the main window finish loading first: creating another
                    // WebView at the exact same instant doubles the startup
                    // memory spike we are trying to avoid.
                    std::thread::sleep(std::time::Duration::from_millis(2500));
                    // The app may already live in the tray by now: its main
                    // window hidden or destroyed, which is exactly when a
                    // satellite must not appear. `restore_main_window` creates it
                    // instead, the moment the main window is back on screen.
                    if main_window_visible(&h) {
                        show_secondary_windows(&h);
                    }
                });
            }
            // ── Observer watcher thread (eyes only, hands off) ──────────
            // 2 s poll, 750 ms debounce, own-write grace: external edits in
            // Notepad surface as toasts, never as file rewrites. Healing is
            // boot-time only — no war with the editor mid-keystroke.
            // Heavy parse runs on THIS thread; toast push is a short lock.
            {
                let w = watcher_for_setup.clone();
                let h = handle.clone();
                // Same contract as the profile loop: admission + rollback +
                // leash bound by `spawn_guarded`; a panic becomes a recorded
                // implicit drop on `file_watcher` instead of a dead thread.
                let (join, _steer) = crate::stability::watcher_node().spawn_guarded(
                    || {},
                    (),
                    move |_conduit: snubber::TaskConduit<(), ()>| {
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
                    },
                );
                std::thread::Builder::new()
                    .name("nanoclick-watcher-guard".into())
                    .spawn(move || {
                        if let Ok(Err(e)) = join.join() {
                            crate::debug_log_internal(
                                "error",
                                &format!("[Stability] file_watcher admission refused: {e:?}"),
                            );
                        }
                    })
                    .ok();
            }
            // ── App-picker catalogue warm-up (Smart Guard suggest) ──────
            // EnumProcesses + uninstall-registry walk (~150-300 ms) once in the
            // background: by the time the user opens Settings the list is RAM.
            // UI-thread-only afterwards — the click loop never touches it.
            crate::platform::warm_up_catalog_async();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_app_config,
            get_startup_notices,
            poll_file_toasts,
            get_notifications,
            get_unread_notifications,
            mark_notifications_read,
            clear_notifications,
            push_client_notification,
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
            commands::export_full_backup_to_disk,
            commands::export_presets_to_disk,
            commands::import_full_backup,
            commands::set_image_trigger,
            commands::pick_screen_pixel,
            commands::get_cursor_pos_now,
            commands::stats_tick,
            commands::get_session_stats,
            commands::dispatch_window_key,
            commands::get_primary_screen_size,
            // v3.2 Macro Engine commands
            commands::list_macros,
            commands::save_macro,
            commands::delete_macro,
            commands::new_unique_id,
            // Preset library store (its own file — a config save can no longer replace it)
            commands::list_presets,
            commands::save_preset,
            commands::delete_preset,
            commands::replace_presets,
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
            prepare_nsis_update,
            commands::get_portable_update_info,
            commands::portable_self_update,
            commands::check_elevation,
            commands::restart_as_admin,
            commands::set_always_run_as_admin,
            commands::get_always_run_as_admin,
            commands::register_elevated_logon_task,
            commands::unregister_elevated_logon_task,
            commands::get_elevated_logon_task_registered,
            platform::capture::start_native_app_capture,
            platform::capture::cancel_native_app_capture,
            platform::capture::start_position_picker_stream,
            platform::capture::stop_position_picker_stream,
            commands::search_apps,
            commands::get_smart_guard_defaults,
            commands::get_smart_guard_status,
            // Key policy (layer B): the user-editable ignored-key list.
            commands::get_key_policy,
            commands::validate_ignore_key,
            commands::get_default_ignored_keys,
            commands::get_update_dismissed_version,
            commands::dismiss_update_version,
            overlay::overlay_ready,
            overlay::toggle_overlay,
            exit_app,
            hud_ready,
            dump_input_diagnostics,
            frontend_ready,
            tray_flush_done,
            uipi_blocked_in_tray,
        ])
        .build(tauri::generate_context!())
    {
        Ok(app) => app,
        Err(e) => {
            // Build failure means no window, no tray, no hotkeys — there is
            // nothing to degrade to. Log it (a panic message is invisible on a
            // GUI launch) and exit with the same non-zero code.
            debug_log_internal(
                "error",
                &format!("[Boot] failed to build tauri application: {e}"),
            );
            std::process::exit(1);
        }
    };

    // Last gate before the engine goes live: arm the out-of-process rescue.
    // Placed after the Tauri build (a forwarded second instance exits there),
    // so only the process that actually owns input gets a guardian.
    input_guardian::spawn_guardian();

    app.run(|app_handle, event| {
        match event {
            RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } => {
                if label == "main" {
                    if should_minimize_to_tray(app_handle) {
                        api.prevent_close();
                        // The state the next start replays — written now, not only
                        // on a clean exit: a Task Manager kill or a power loss must
                        // not show again a window the user had placed in the tray.
                        persist_window_visibility(app_handle, false);
                        // Two ways to live in the tray: plain hide (instant and,
                        // measured, free — private working set stays ~120 MB) or
                        // deep sleep (destroys the WebView, rebuilds it on the
                        // next tray click: 1 process / 5.0 MB). Deep sleep is
                        // refused whenever something is in flight.
                        if deep_sleep_allowed_now(app_handle) {
                            start_tray_if_needed(app_handle);
                            suspend_main_webview_to_tray(app_handle);
                            debug_log_internal("info", "[Tray] close requested -> deep sleep");
                        } else {
                            if let Some(win) = app_handle.get_webview_window("main") {
                                let _ = win.hide();
                            }
                            hide_secondary_windows(app_handle);
                            start_tray_if_needed(app_handle);
                            debug_log_internal(
                                "warn",
                                &format!(
                                    "[Tray] close requested -> hidden to tray \
                                     (deep sleep skipped: {})",
                                    deep_sleep_skip_reason(app_handle)
                                ),
                            );
                        }
                    } else {
                        shutdown_application(app_handle);
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
                    && should_minimize_to_tray(app_handle)
                {
                    persist_window_visibility(app_handle, false);
                    if deep_sleep_allowed_now(app_handle) {
                        start_tray_if_needed(app_handle);
                        suspend_main_webview_to_tray(app_handle);
                        debug_log_internal("info", "[Tray] minimize requested -> deep sleep");
                    } else {
                        if let Some(win) = app_handle.get_webview_window("main") {
                            let _ = win.hide();
                        }
                        hide_secondary_windows(app_handle);
                        start_tray_if_needed(app_handle);
                        debug_log_internal(
                            "warn",
                            &format!(
                                "[Tray] minimize requested -> hidden to tray \
                                 (deep sleep skipped: {})",
                                deep_sleep_skip_reason(app_handle)
                            ),
                        );
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
                platform::windows::stop_foreground_watch();
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

/// Panic-log formatting. The hook itself is not installed here: it is
/// process-global, and a stray test panic would write into the real log.
#[cfg(test)]
mod panic_logger_tests {
    use super::*;

    #[test]
    fn panic_record_carries_thread_location_message_and_backtrace() {
        let record = format_panic_record(
            "nanoclick-test",
            Some(("src/lib.rs", 123, 45)),
            "boom",
            "   0: fake_frame\n",
        );
        assert!(record.starts_with("[RUST PANIC] thread=nanoclick-test at src/lib.rs:123:45"));
        assert!(record.contains("[RUST PANIC] message: boom"));
        assert!(record.contains("   0: fake_frame"));
        assert!(record.ends_with("[RUST PANIC] end\n"));
    }

    #[test]
    fn panic_record_without_location_stays_readable() {
        let record = format_panic_record("nanoclick-test", None, "boom", "");
        assert!(record.contains("<unknown location>"));
        assert!(record.contains("[RUST PANIC] message: boom"));
    }

    #[test]
    fn panic_log_uses_the_shared_log_file() {
        assert!(log_file_path().ends_with("nanoclick_web.log"));
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
        assert!(
            !deep_sleep_allowed(true, true, false, false),
            "clicker running"
        );
        assert!(
            !deep_sleep_allowed(true, false, true, false),
            "macro playing"
        );
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
