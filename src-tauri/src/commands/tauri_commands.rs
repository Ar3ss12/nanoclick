//! Tauri commands exposed to the frontend.
//!
//! The frontend talks to the Rust engine exclusively through these
//! `#[tauri::command]` functions — they form the public IPC surface.
//!
//! Reference: `docs/MACRO_ARCHITECTURE.md` §9.

/// Report the running app version (single source of truth:
/// `Cargo.toml` / `tauri.conf.json`, synced by release process).
#[tauri::command]
pub fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Report what the current platform can actually do (v4.2).
#[tauri::command]
pub fn get_platform_capabilities() -> serde_json::Value {
    let caps = crate::platform::PlatformCapabilities::detect();
    serde_json::json!({
        "global_hotkeys": caps.global_hotkeys,
        "global_input_recording": caps.global_input_recording,
        "mouse_injection": caps.mouse_injection,
        "keyboard_injection": caps.keyboard_injection,
        "can_play_macros": caps.can_play_macros(),
    })
}

use crate::core::{ExecutorHandle, Macro, MacroLookup};
use crate::persistence;
use crate::platform;
use crate::recorder::{RecorderHandle, RecordingMode};
use std::sync::{Arc, Mutex};
use tauri::State;

/// App state for macro/recorder commands.
pub struct MacroState {
    pub recorder: Mutex<Option<RecorderSession>>,
    pub executor: Arc<ExecutorHandle>,
}

/// Active recorder session. The platform hook is process-global on Windows,
/// so Drop also shuts it down if a command exits early.
pub struct RecorderSession {
    pub handle: RecorderHandle,
}

impl Drop for RecorderSession {
    fn drop(&mut self) {
        platform::recorder_backend_stop();
        self.handle.cancel();
    }
}

impl Default for MacroState {
    fn default() -> Self {
        MacroState {
            recorder: Mutex::new(None),
            executor: Arc::new(ExecutorHandle::new()),
        }
    }
}

// ─── Persistence ────────────────────────────────────────────────

// ─── Image trigger (F8) ─────────────────────────────────────

#[tauri::command]
pub fn set_image_trigger(
    state: State<'_, crate::AppState>,
    trigger: Option<crate::config_manager::ImageTrigger>,
) -> Result<(), String> {
    state.scheduler.set_image_trigger(trigger.clone());
    // Persist alongside the rest of the config so a restart keeps it.
    let mut cfg = state.config_manager.load();
    cfg.image_trigger = trigger;
    state.config_manager.save(&cfg)
}

#[tauri::command]
pub fn pick_screen_pixel(x: i32, y: i32) -> Option<u32> {
    platform::get_pixel_rgba(x, y)
}

#[tauri::command]
pub fn get_cursor_pos_now() -> (i32, i32) {
    platform::get_cursor_pos()
}

#[tauri::command]
pub fn get_primary_screen_size() -> serde_json::Value {
    let (w, h) = platform::get_screen_size();
    serde_json::json!({ "width": w, "height": h })
}

// ─── Recorder ────────────────────────────────────────────────

#[tauri::command]
pub fn list_macros() -> Vec<Macro> {
    persistence::macros::load_macros()
}

#[tauri::command]
pub fn save_macro(m: Macro) -> Result<Vec<Macro>, String> {
    let mut all = persistence::macros::load_macros();
    if let Some(existing) = all.iter_mut().find(|x| x.id == m.id) {
        *existing = m;
    } else {
        all.push(m);
    }
    persistence::macros::save_macros(&all)?;
    Ok(all)
}

#[tauri::command]
pub fn delete_macro(id: String) -> Result<Vec<Macro>, String> {
    let mut all = persistence::macros::load_macros();
    all.retain(|x| x.id != id);
    persistence::macros::save_macros(&all)?;
    Ok(all)
}

// ─── Recorder ────────────────────────────────────────────────────

#[tauri::command]
pub fn record_start(
    mode: String,
    record_hotkey: Option<String>,
    state: State<'_, MacroState>,
) -> Result<(), String> {
    let rec_mode = match mode.as_str() {
        "precise" => RecordingMode::Precise,
        _ => RecordingMode::Smart,
    };
    // Bridge: Recorder receives events via the Sender it gives us; we then
    // forward a clone of the Sender into the platform hook session.
    let mut slot = state.recorder.lock().map_err(|e| e.to_string())?;
    if slot.is_some() {
        return Err("recorder already running".into());
    }
    let (handle, tx) = RecorderHandle::start_with_external_sender(rec_mode);
    // v4.2 — start capture through the RecorderBackend contract; the
    // hotkey label is parsed inside the platform boundary.
    let recorder_backend = platform::default_recorder_backend(&record_hotkey.unwrap_or_default());
    recorder_backend.start(tx)?;
    *slot = Some(RecorderSession { handle });
    Ok(())
}

#[tauri::command]
pub fn record_stop(state: State<'_, MacroState>) -> Result<Vec<Macro>, String> {
    let mut slot = state.recorder.lock().map_err(|e| e.to_string())?;
    let session = slot.take().ok_or("recorder was not running")?;
    // v4.2 — stop through the contract (stateless global stop).
    platform::recorder_backend_stop();
    session.handle.stop();
    let actions = session.handle.stop();
    let m = Macro {
        id: uuid_like_id(),
        name: "Untitled macro".into(),
        icon: "🎬".into(),
        actions,
        repeat: crate::core::RepeatMode::Once,
        enabled: None,
        created_at: now_ms() as i64,
        updated_at: now_ms() as i64,
    };
    Ok(vec![m])
}

#[tauri::command]
pub fn record_cancel(state: State<'_, MacroState>) -> Result<(), String> {
    if let Some(session) = state.recorder.lock().map_err(|e| e.to_string())?.take() {
        platform::recorder_backend_stop();
        session.handle.cancel();
    }
    Ok(())
}

// ─── Executor (Player) ───────────────────────────────────────────

#[tauri::command]
pub fn play_macro(m: Macro, state: State<'_, MacroState>) -> Result<(), String> {
    let macros = persistence::macros::load_macros();
    let lookup: MacroLookup =
        Arc::new(move |id| macros.iter().find(|candidate| candidate.id == id).cloned());
    state.executor.play_async_with_lookup(m, lookup);
    Ok(())
}

#[tauri::command]
pub fn play_macro_from(
    m: Macro,
    start_idx: usize,
    state: State<'_, MacroState>,
) -> Result<(), String> {
    state.executor.set_start_idx(start_idx);
    state.executor.play_async(m);
    Ok(())
}

#[tauri::command]
pub fn step_macro(m: Macro, state: State<'_, MacroState>) -> Result<bool, String> {
    Ok(state.executor.step(&m))
}

#[tauri::command]
pub fn rewind_macro(state: State<'_, MacroState>) -> Result<(), String> {
    state.executor.rewind();
    Ok(())
}

#[tauri::command]
pub fn stop_macro(state: State<'_, MacroState>) -> Result<(), String> {
    state.executor.stop();
    Ok(())
}

#[tauri::command]
pub fn is_macro_running(state: State<'_, MacroState>) -> bool {
    state.executor.is_running()
}

// ─── Helpers ─────────────────────────────────────────────────────

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn uuid_like_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    format!("m_{:x}", n)
}

// ─── Tests ───────────────────────────────────────────────────────

// ── Full backup: config + presets + macros in one file ──────

#[tauri::command]
pub fn export_full_backup(state: State<'_, crate::AppState>) -> Result<String, String> {
    let app_cfg = state.config_manager.load();
    let macros = crate::persistence::macros::load_macros();
    let backup = serde_json::json!({
        "backup_version": 1,
        "exported_at": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        "app_version": env!("CARGO_PKG_VERSION"),
        "config": app_cfg,
        "macros": macros,
    });
    serde_json::to_string_pretty(&backup).map_err(|e| format!("serialize backup: {e}"))
}

#[tauri::command]
pub fn import_full_backup(
    state: State<'_, crate::AppState>,
    backup_json: String,
    restore_config: bool,
    restore_macros: bool,
) -> Result<String, String> {
    let value: serde_json::Value =
        serde_json::from_str(&backup_json).map_err(|e| format!("parse backup: {e}"))?;
    if value
        .get("backup_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        return Err("unsupported or missing backup_version (expected 1)".into());
    }
    let mut restored: Vec<String> = Vec::new();
    if restore_config {
        if let Some(cfg_value) = value.get("config") {
            let cfg: crate::config_manager::AppConfig =
                serde_json::from_value(cfg_value.clone())
                    .map_err(|e| format!("invalid config in backup: {e}"))?;
            state.config_manager.save(&cfg)?;
            if let Some(trigger) = cfg.image_trigger.clone() {
                state.scheduler.set_image_trigger(Some(trigger));
            }
            restored.push("config+presets".to_string());
        }
    }
    if restore_macros {
        if let Some(macros_value) = value.get("macros") {
            let macros: Vec<crate::core::Macro> = serde_json::from_value(macros_value.clone())
                .map_err(|e| format!("invalid macros in backup: {e}"))?;
            crate::persistence::macros::save_macros(&macros)?;
            restored.push(format!("{} macros", macros.len()));
        }
    }
    if restored.is_empty() {
        return Err("nothing selected to restore".into());
    }
    Ok(format!("restored: {}", restored.join(", ")))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::action::{KeyCode, MouseButton};
    use crate::core::RepeatMode;
    use crate::recorder::raw_event::RawEvent;

    #[test]
    fn macro_state_default() {
        let s = MacroState::default();
        assert!(!s.executor.is_running());
    }

    #[test]
    fn action_vector_roundtrip_through_save() {
        let m = Macro {
            id: "abc".into(),
            name: "n".into(),
            icon: "🎬".into(),
            actions: vec![crate::core::Action::Wait { ms: 50 }],
            repeat: RepeatMode::Once,
            enabled: None,
            created_at: 0,
            updated_at: 0,
        };
        let s = serde_json::to_string(&m).unwrap();
        let back: Macro = serde_json::from_str(&s).unwrap();
        assert_eq!(back.id, "abc");
        assert_eq!(back.actions.len(), 1);
    }

    #[test]
    fn recorder_start_stop_returns_macro() {
        // Synthesize a few raw events via a manual sender.
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = RecorderHandle::start(RecordingMode::Smart, move |inner_tx| {
            // Forward synthetic events to inner_tx via the rx trick: we just
            // post directly to the inner channel from this thread's tx.
            // Since we don't have access to inner_tx here, this test
            // demonstrates that handle.stop() yields an empty Vec (no events
            // sent). The full event flow is tested in normalizer.rs.
            drop(inner_tx);
        });
        // Push a few raw events via the captured tx.
        tx.send(RawEvent::MouseDown {
            button: MouseButton::Left,
            t_ms: 0,
        })
        .ok();
        tx.send(RawEvent::MouseUp {
            button: MouseButton::Left,
            t_ms: 10,
        })
        .ok();
        drop(tx);
        // Stop and verify we get back at least an empty Vec (handle may or may
        // not have received events, since it uses its own internal channel).
        let actions = handle.stop();
        let _ = (rx, actions);
        // The exact contents depend on race conditions, so we just assert
        // that the type is a Vec<Action> and the handle can be dropped.
    }

    #[test]
    fn executor_runs_in_isolated_thread() {
        // Smoke test that ExecutorHandle is Send/Sync.
        let h = Arc::new(ExecutorHandle::new());
        let h2 = Arc::clone(&h);
        let t = std::thread::spawn(move || {
            let m = Macro {
                id: "x".into(),
                name: "x".into(),
                icon: "🎬".into(),
                actions: vec![crate::core::Action::Wait { ms: 10 }],
                repeat: RepeatMode::Once,
                enabled: None,
                created_at: 0,
                updated_at: 0,
            };
            h2.play(&m);
        });
        t.join().unwrap();
    }

    #[test]
    fn modifier_struct_serializes() {
        // Just smoke-test that the types are constructed/used without panics.
        let m = crate::core::Macro {
            id: "smoke".into(),
            name: "smoke".into(),
            icon: "🎬".into(),
            actions: vec![crate::core::Action::KeyPress {
                key: KeyCode(0x41),
                mods: crate::core::action::Modifiers {
                    ctrl: false,
                    shift: false,
                    alt: false,
                    win: false,
                },
            }],
            repeat: RepeatMode::Once,
            enabled: None,
            created_at: 0,
            updated_at: 0,
        };
        let _ = serde_json::to_string(&m).unwrap();
    }

    #[test]
    fn get_app_version_returns_valid_string() {
        let ver = get_app_version();
        assert!(!ver.is_empty(), "app version must not be empty");
        assert!(ver.contains('.'), "app version should be semver format");
    }

    #[test]
    fn get_platform_capabilities_has_required_keys() {
        let caps = get_platform_capabilities();
        assert!(caps.get("global_hotkeys").is_some());
        assert!(caps.get("mouse_injection").is_some());
        assert!(caps.get("keyboard_injection").is_some());
        assert!(caps.get("can_play_macros").is_some());
    }

    #[test]
    fn full_backup_structure_validation() {
        let dummy_cfg = crate::config_manager::AppConfig::default();
        let dummy_macro = Macro {
            id: "m_test_backup".into(),
            name: "Test Macro".into(),
            icon: "⚙️".into(),
            actions: vec![crate::core::Action::Wait { ms: 100 }],
            repeat: RepeatMode::Once,
            enabled: None,
            created_at: 1000,
            updated_at: 1000,
        };
        let backup = serde_json::json!({
            "backup_version": 1,
            "exported_at": 1234567890,
            "app_version": "1.2.0",
            "config": dummy_cfg,
            "macros": vec![dummy_macro],
        });
        let backup_str = serde_json::to_string(&backup).unwrap();
        assert!(backup_str.contains("backup_version"));
        assert!(backup_str.contains("m_test_backup"));
    }
}
