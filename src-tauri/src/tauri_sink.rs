//! Tauri adapter for `StatusSink` (Stage B dual-write).
//! Lives in its own module: daemon_core never sees this file.
use crate::status_sink::{StatusPayload, StatusSink};
use tauri::{Emitter, Manager};
pub struct TauriSink {
    app: tauri::AppHandle,
}
impl TauriSink {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }
}
impl StatusSink for TauriSink {
    fn push_status(&self, u: StatusPayload) {
        if let Some(w) = self.app.get_webview_window("main") {
            let _ = w.emit(crate::ipc::EVENT_STATUS_UPDATE, &u);
        }
    }
    fn push_hud(&self, total: u32) {
        if let Some(w) = self.app.get_webview_window("hud") {
            let _ = w.emit(crate::ipc::EVENT_HUD_CLICKS, total);
        }
    }
    fn push_event(&self, e: &str, p: serde_json::Value) {
        let _ = self.app.emit(e, p);
    }
}
/// Global extra sink (daemon IPC broadcast in Stage D; None = in-process only).
static EXTRA_SINK: std::sync::OnceLock<std::sync::Mutex<Option<std::sync::Arc<dyn StatusSink>>>> =
    std::sync::OnceLock::new();
/// Install the daemon-side broadcast sink. Call once at boot.
pub fn set_extra_sink(s: std::sync::Arc<dyn StatusSink>) {
    if let Ok(mut g) = EXTRA_SINK
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
    {
        *g = Some(s);
    }
}
/// Mirror one status frame to the extra sink (fire-and-forget, never panics).
pub fn mirror_status(u: &StatusPayload) {
    if let Some(lock) = EXTRA_SINK.get() {
        if let Ok(g) = lock.lock() {
            if let Some(s) = g.as_ref() {
                s.push_status(u.clone());
            }
        }
    }
}
/// Mirror one HUD counter to the extra sink.
pub fn mirror_hud(total: u32) {
    if let Some(lock) = EXTRA_SINK.get() {
        if let Ok(g) = lock.lock() {
            if let Some(s) = g.as_ref() {
                s.push_hud(total);
            }
        }
    }
}
