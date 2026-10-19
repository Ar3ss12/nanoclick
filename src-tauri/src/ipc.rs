//! Stage B IPC contract: JSON-lines envelopes shared by daemon + UI.
//! Transport (Stage D pipe) owns sockets; this file owns bytes + schema.
use serde::{Deserialize, Serialize};
pub const DAEMON_PIPE_NAME: &str = r"\\.\pipe\nanoclick-daemon";
pub const EVENT_STATUS_UPDATE: &str = "status-update";
pub const EVENT_HUD_CLICKS: &str = "hud-clicks";
pub const EVENT_TRAY_ACTION_RESULT: &str = "tray-action-result";
pub const EVENT_SATELLITE_TOGGLE_RESULT: &str = "satellite-toggle-result";
pub const EVENT_FULL_STATE_SYNC: &str = "full-state-sync";
pub const EVENT_PRESET_ACTIVATED: &str = "preset-activated";
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IpcRequest {
    pub id: u64,
    pub cmd: String,
    #[serde(default)]
    pub params: serde_json::Value,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IpcResponse {
    pub id: u64,
    pub ok: bool,
    #[serde(default)]
    pub payload: serde_json::Value,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonEvent {
    pub event: String,
    #[serde(default)]
    pub payload: serde_json::Value,
}
impl IpcRequest {
    pub fn new(id: u64, cmd: impl Into<String>, params: serde_json::Value) -> Self {
        Self {
            id,
            cmd: cmd.into(),
            params,
        }
    }
    pub fn encode(&self) -> String {
        let mut s = serde_json::to_string(self)
            .unwrap_or_else(|_| format!(r#"{{"id":{},"cmd":"invalid","params":{{}}}}"#, self.id));
        s.push('\n');
        s
    }
    pub fn decode(line: &str) -> Option<Self> {
        serde_json::from_str(line.trim()).ok()
    }
}
impl IpcResponse {
    pub fn ok(id: u64, payload: serde_json::Value) -> Self {
        Self {
            id,
            ok: true,
            payload,
        }
    }
    pub fn err(id: u64, reason: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            payload: serde_json::json!({ "error": reason.into() }),
        }
    }
    pub fn encode(&self) -> String {
        let mut s = serde_json::to_string(self)
            .unwrap_or_else(|_| format!(r#"{{"id":{},"ok":false,"payload":{{}}}}"#, self.id));
        s.push('\n');
        s
    }
    pub fn decode(line: &str) -> Option<Self> {
        serde_json::from_str(line.trim()).ok()
    }
}
impl DaemonEvent {
    pub fn new(event: impl Into<String>, payload: serde_json::Value) -> Self {
        Self {
            event: event.into(),
            payload,
        }
    }
    pub fn encode(&self) -> String {
        let mut s = serde_json::to_string(self)
            .unwrap_or_else(|_| r#"{"event":"invalid","payload":{}}"#.to_string());
        s.push('\n');
        s
    }
    pub fn decode(line: &str) -> Option<Self> {
        serde_json::from_str(line.trim()).ok()
    }
    pub fn is_loss_tolerant(&self) -> bool {
        matches!(self.event.as_str(), "spawn-ripple" | "hud-clicks")
    }
}
pub fn encode_satellite_frame(event: &str, payload: serde_json::Value) -> String {
    let mut s = serde_json::json!({ "cmd": event, "payload": payload }).to_string();
    s.push('\n');
    s
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn req_round_trips_one_line() {
        let r = IpcRequest::new(7, "get_state", serde_json::json!({}));
        let l = r.encode();
        assert!(l.ends_with('\n'));
        assert_eq!(IpcRequest::decode(&l), Some(r));
    }
    #[test]
    fn resp_ok_err_round_trip() {
        let ok = IpcResponse::ok(1, serde_json::json!({"a":1}));
        assert_eq!(IpcResponse::decode(&ok.encode()), Some(ok));
        let e = IpcResponse::err(2, "busy");
        let b = IpcResponse::decode(&e.encode()).unwrap();
        assert!(!b.ok);
        assert_eq!(b.payload["error"], "busy");
    }
    #[test]
    fn bad_lines_are_none() {
        assert_eq!(IpcRequest::decode(""), None);
        assert_eq!(IpcRequest::decode("{x}\n"), None);
        assert_eq!(DaemonEvent::decode("[]\n"), None);
    }
    #[test]
    fn event_names_match_pages() {
        assert_eq!(EVENT_STATUS_UPDATE, "status-update");
        assert_eq!(EVENT_HUD_CLICKS, "hud-clicks");
        let s = DaemonEvent::new("status-update", serde_json::json!({}));
        assert!(!s.is_loss_tolerant());
        assert!(DaemonEvent::new("hud-clicks", serde_json::json!(1)).is_loss_tolerant());
    }
}
