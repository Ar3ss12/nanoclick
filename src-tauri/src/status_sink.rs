//! Stage B emit port: `StatusSink` abstracts WHERE status goes.
//! In-process = Tauri emit; daemon = IPC broadcast. Zero logic change.
//! This file is Tauri-free on purpose (daemon_core must compile w/o Tauri).
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusPayload {
    pub active: bool,
    pub mode: String,
    pub clicks_done: u32,
    pub cps: f64,
    pub status_text: String,
}
pub trait StatusSink: Send + Sync + 'static {
    fn push_status(&self, u: StatusPayload);
    fn push_hud(&self, total: u32);
    // Generic event leg (daemon IPC parity). The Tauri path forwards to
    // `app.emit`; test doubles record into `VecSink::frames`.
    #[allow(dead_code)]
    fn push_event(&self, e: &str, p: serde_json::Value);
}
// Test doubles for the sink parity test (`status_sink_twin_matches_tauri_payload`).
// `NullSink` is the drop-everything baseline; `VecSink` records frames.
#[allow(dead_code)]
#[derive(Default)]
pub struct NullSink;
impl StatusSink for NullSink {
    fn push_status(&self, _u: StatusPayload) {}
    fn push_hud(&self, _t: u32) {}
    fn push_event(&self, _e: &str, _p: serde_json::Value) {}
}
#[allow(dead_code)]
#[derive(Default)]
pub struct VecSink {
    pub frames: std::sync::Mutex<Vec<(String, serde_json::Value)>>,
}
impl VecSink {
    #[allow(dead_code)]
    pub fn count(&self, e: &str) -> usize {
        self.frames
            .lock()
            .map(|g| g.iter().filter(|(x, _)| x == e).count())
            .unwrap_or(0)
    }
}
impl StatusSink for VecSink {
    fn push_status(&self, u: StatusPayload) {
        if let Ok(mut g) = self.frames.lock() {
            g.push((
                crate::ipc::EVENT_STATUS_UPDATE.into(),
                serde_json::to_value(u).unwrap_or_default(),
            ));
        }
    }
    fn push_hud(&self, t: u32) {
        if let Ok(mut g) = self.frames.lock() {
            g.push((crate::ipc::EVENT_HUD_CLICKS.into(), serde_json::json!(t)));
        }
    }
    fn push_event(&self, e: &str, p: serde_json::Value) {
        if let Ok(mut g) = self.frames.lock() {
            g.push((e.to_string(), p));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vec_sink_records_both_channels() {
        let s = VecSink::default();
        s.push_status(StatusPayload {
            active: true,
            mode: "autoclicker".into(),
            clicks_done: 3,
            cps: 10.0,
            status_text: "RUNNING".into(),
        });
        s.push_hud(3);
        assert_eq!(s.count(crate::ipc::EVENT_STATUS_UPDATE), 1);
        assert_eq!(s.count(crate::ipc::EVENT_HUD_CLICKS), 1);
    }
}
