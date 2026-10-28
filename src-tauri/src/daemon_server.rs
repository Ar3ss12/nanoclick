//! Stage D/E daemon server: owns config + scheduler + tray + broadcast.
//! One thread per UI client; requests = `IpcRequest` lines, answers = lines.
//! Full E command table (state + engine + presets + diagnostics).
use crate::daemon_transport::daemon_tcp_addr;
use crate::ipc::{DaemonEvent, IpcRequest, IpcResponse, EVENT_STATUS_UPDATE};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
/// Shared daemon state: scheduler + config owner + broadcast fan-out.
pub struct DaemonState {
    pub scheduler: Arc<crate::scheduler::ClickScheduler>,
    pub config_manager: Arc<crate::config_manager::ConfigManager>,
    pub clients: Mutex<Vec<std::net::TcpStream>>,
}
impl DaemonState {
    pub fn new() -> Self {
        // One maintenance thread per engine-owning process: the daemon runs its
        // own rollback/memory/snapshot observers even when no UI is attached.
        crate::stability::start_maintenance();
        let manager = Arc::new(crate::config_manager::ConfigManager::new());
        // The daemon OWNS the file: load once, then persist every mutation.
        // Bootstrap binds the freshly loaded config to a `Protected`
        // transaction: `commit()` publishes it, and a panic in the window
        // between load and publish drops the half-built config instead of
        // arming the daemon with it. `Protected` pairs with the v0.1
        // `SnubberGuard` (the node's `TrackedGuard` settles its own books and
        // cannot be bound), so the guard is minted from the `config_save`
        // sequence and its rollback is a recorded, observable discard.
        let seq = crate::stability::config_node().next_sequence();
        let bootstrap_guard = snubber::SnubberGuard::new(seq, seq, || {
            crate::debug_log_internal(
                "error",
                "[Daemon] config bootstrap aborted before publish — config discarded",
            );
        });
        let (app_cfg, _) =
            snubber::Protected::new(manager.load_with_action(), bootstrap_guard).commit();
        let scheduler = Arc::new(crate::scheduler::ClickScheduler::new());
        scheduler.set_config(crate::config::Config::from(app_cfg));
        Self {
            scheduler,
            config_manager: manager,
            clients: Mutex::new(Vec::new()),
        }
    }

    /// Broadcast one event to every connected UI (fire-and-forget per client).
    pub fn broadcast(&self, ev: &DaemonEvent) {
        let line = ev.encode();
        if let Ok(mut g) = self.clients.lock() {
            g.retain_mut(|s| crate::daemon_transport::write_line(s, &line).is_ok());
        }
    }
}

impl Default for DaemonState {
    /// Default state: same as `new()` (loads the config file).
    fn default() -> Self {
        Self::new()
    }
}
/// Answer one request. Full E table: state queries, engine control,
/// presets, diagnostics. Unknown = Err, never a hang or panic.
pub fn handle_request(state: &DaemonState, req: &IpcRequest) -> IpcResponse {
    match req.cmd.as_str() {
        "ping" => IpcResponse::ok(req.id, serde_json::json!({"pong": true})),
        "get_state" => {
            let s = &state.scheduler;
            IpcResponse::ok(
                req.id,
                serde_json::json!({
                    "active": s.is_active(),
                    "mode": if s.is_autoclicker_mode() { "autoclicker" } else { "work" },
                    "clicks_done": s.get_clicks_done(),
                    "cps": s.get_config().cps,
                    "status_text": if s.is_active() { "RUNNING" } else { "IDLE" },
                }),
            )
        }
        "start" => match s_start(state) {
            Ok(text) => IpcResponse::ok(req.id, serde_json::json!({"status": text})),
            Err(e) => IpcResponse::err(req.id, e),
        },
        "stop" => {
            state.scheduler.set_active(false, None);
            push_status_now(state);
            IpcResponse::ok(req.id, serde_json::json!({"status": "IDLE"}))
        }
        "toggle" => {
            let active = !state.scheduler.is_active();
            match state.scheduler.set_active(active, None) {
                crate::scheduler::ToggleOutcome::Start => {
                    push_status_now(state);
                    IpcResponse::ok(req.id, serde_json::json!({"status": "RUNNING"}))
                }
                crate::scheduler::ToggleOutcome::Stop => {
                    push_status_now(state);
                    IpcResponse::ok(req.id, serde_json::json!({"status": "IDLE"}))
                }
                o => IpcResponse::err(req.id, format!("refused: {o:?}")),
            }
        }
        "toggle_mode" => {
            let mode = state.scheduler.toggle_mode(None);
            push_status_now(state);
            IpcResponse::ok(req.id, serde_json::json!({"mode": mode}))
        }
        "adjust_cps" => {
            let delta = req
                .params
                .get("delta")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let next = state.scheduler.adjust_cps(delta, None);
            IpcResponse::ok(req.id, serde_json::json!({"cps": next}))
        }
        "get_full_state" => {
            // A rollback body runs `SendInput` and can sit in UIPI; the status
            // surface names it instead of leaving "RUNNING, no clicks" unexplained.
            let (rollback_active, rollback_elapsed_ms) = crate::stability::rollback_status();
            IpcResponse::ok(
                req.id,
                serde_json::json!({
                    "active": state.scheduler.is_active(),
                    "mode": if state.scheduler.is_autoclicker_mode() { "autoclicker" } else { "work" },
                    "clicks_done": state.scheduler.get_clicks_done(),
                    "breaker_tripped": crate::stability::is_breaker_tripped(),
                    "total_faults": crate::stability::total_system_faults(),
                    "rollback_active": rollback_active,
                    "rollback_elapsed_ms": rollback_elapsed_ms,
                    "rollback_budget_ms": crate::stability::ROLLBACK_BUDGET_MS,
                }),
            )
        }
        "activate_preset" => {
            let id = req.params.get("id").and_then(|v| v.as_str()).unwrap_or("");
            match state.scheduler.activate_preset_hotkey(id, None) {
                crate::scheduler::ToggleOutcome::Start => {
                    push_status_now(state);
                    IpcResponse::ok(req.id, serde_json::json!({"status": "RUNNING"}))
                }
                crate::scheduler::ToggleOutcome::Stop => {
                    push_status_now(state);
                    IpcResponse::ok(req.id, serde_json::json!({"status": "IDLE"}))
                }
                o => IpcResponse::err(req.id, format!("refused: {o:?}")),
            }
        }
        "emergency_stop" => {
            state.scheduler.set_active(false, None);
            push_status_now(state);
            IpcResponse::ok(req.id, serde_json::json!({"status": "IDLE"}))
        }
        _ => IpcResponse::err(req.id, format!("unknown cmd `{}`", req.cmd)),
    }
}
fn s_start(state: &DaemonState) -> Result<String, String> {
    use crate::scheduler::ToggleOutcome;
    match state.scheduler.set_active(true, None) {
        ToggleOutcome::Start => {
            push_status_now(state);
            Ok("RUNNING".to_string())
        }
        ToggleOutcome::Stop => Ok("IDLE".to_string()),
        o => Err(format!("refused: {o:?}")),
    }
}
fn push_status_now(state: &DaemonState) {
    let s = &state.scheduler;
    let ev = DaemonEvent::new(
        EVENT_STATUS_UPDATE,
        serde_json::json!({
            "active": s.is_active(),
            "mode": if s.is_autoclicker_mode() { "autoclicker" } else { "work" },
            "clicks_done": s.get_clicks_done(),
            "status_text": if s.is_active() { "RUNNING" } else { "IDLE" },
        }),
    );
    state.broadcast(&ev);
}
fn serve_one(state: Arc<DaemonState>, stream: TcpStream) {
    if let Ok(peer) = stream.try_clone() {
        if let Ok(mut g) = state.clients.lock() {
            g.push(peer);
        }
    }
    crate::daemon_transport::serve_stream(&state, stream, |st, req| {
        handle_request(st.as_ref(), req)
    });
}
/// Blocking serve loop (call from `main-daemon`).
pub fn serve(state: Arc<DaemonState>) -> std::io::Result<()> {
    crate::stability::start_maintenance();
    let listener = TcpListener::bind(daemon_tcp_addr())?;
    for s in listener.incoming().flatten() {
        let st = Arc::clone(&state);
        std::thread::Builder::new()
            .name("nanoclick-daemon-client".into())
            .spawn(move || serve_one(st, s))
            .ok();
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn handle_ping_get_state_unknown() {
        let state = DaemonState::new();
        let ping = handle_request(&state, &IpcRequest::new(1, "ping", serde_json::json!({})));
        assert!(ping.ok);
        let st = handle_request(
            &state,
            &IpcRequest::new(2, "get_state", serde_json::json!({})),
        );
        assert!(st.ok);
        assert_eq!(st.payload["active"], serde_json::Value::from(false));
        let unknown = handle_request(&state, &IpcRequest::new(3, "nope", serde_json::json!({})));
        assert!(!unknown.ok);
    }
    #[test]
    fn broadcast_without_clients_is_noop() {
        let state = DaemonState::new();
        state.broadcast(&DaemonEvent::new(
            EVENT_STATUS_UPDATE,
            serde_json::json!({}),
        ));
    }
}
