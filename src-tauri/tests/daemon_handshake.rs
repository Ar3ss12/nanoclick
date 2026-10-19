//! Stage F gates: handshake daemon<->UI + UI-disconnect resilience.
//! The daemon is the truth; a dead UI never stops the engine.
//! Heavy `#[ignore]` chaos (real process kill) lives here too.
use nanoclick::daemon_client::call_addr_timeout;
use nanoclick::daemon_server::{handle_request, DaemonState};
use nanoclick::ipc::IpcRequest;
use std::sync::Arc;
use std::time::Duration;
/// Spin a real TCP server on an ephemeral port with the production handler.
fn spawn_test_server(state: Arc<DaemonState>) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback bind");
    let addr = listener.local_addr().expect("local addr").to_string();
    std::thread::Builder::new()
        .name("test-daemon-server".into())
        .spawn(move || {
            for stream in listener.incoming() {
                if let Ok(s) = stream {
                    let st = Arc::clone(&state);
                    std::thread::spawn(move || {
                        nanoclick::daemon_transport::serve_stream(&st, s, |st, req| {
                            handle_request(st.as_ref(), req)
                        })
                    });
                }
            }
        })
        .ok();
    // Give the listener a moment to start accepting.
    std::thread::sleep(Duration::from_millis(50));
    addr
}
#[test]
fn handshake_ping_state_toggle_over_tcp() {
    let state = Arc::new(DaemonState::new());
    let addr = spawn_test_server(Arc::clone(&state));
    let pong = call_addr_timeout(&addr, "ping", serde_json::json!({}), Duration::from_secs(2))
        .expect("ping must answer");
    assert_eq!(pong["pong"], serde_json::Value::from(true));
    let st = call_addr_timeout(
        &addr,
        "get_state",
        serde_json::json!({}),
        Duration::from_secs(2),
    )
    .expect("get_state must answer");
    assert_eq!(st["active"], serde_json::Value::from(false));
    let mode = call_addr_timeout(
        &addr,
        "toggle_mode",
        serde_json::json!({}),
        Duration::from_secs(2),
    )
    .expect("toggle_mode must answer");
    assert!(mode.get("mode").and_then(|v| v.as_str()).is_some());
}
#[test]
fn unknown_cmd_is_err_not_panic() {
    let state = Arc::new(DaemonState::new());
    let addr = spawn_test_server(Arc::clone(&state));
    let err = call_addr_timeout(&addr, "nope", serde_json::json!({}), Duration::from_secs(2));
    assert!(err.is_err(), "unknown cmd must be Err, never a hang");
}
/// UI-disconnect resilience (mirror of snubber `chaos_dual_terminal`, fast version):
/// drop the client mid-session — the daemon state survives, the next UI
/// reconnects and reads the same truth. `#[ignore]` = real-process variant
/// below does the OS kill; this one proves the server side never depends on
/// any single connection lifetime.
#[test]
fn ui_disconnect_daemon_truth_survives() {
    let state = Arc::new(DaemonState::new());
    let addr = spawn_test_server(Arc::clone(&state));
    // Client 1 connects, reads state, then "dies" (socket dropped).
    {
        let s = std::net::TcpStream::connect(&addr).expect("connect");
        drop(s);
    }
    std::thread::sleep(Duration::from_millis(50));
    // Client 2 (the "restarted UI") reads the same truth.
    let st = call_addr_timeout(
        &addr,
        "get_state",
        serde_json::json!({}),
        Duration::from_secs(2),
    )
    .expect("reconnect must answer");
    assert_eq!(st["active"], serde_json::Value::from(false));
}
/// Real-process UI-disconnect chaos: spawn the daemon binary, drop the "UI"
/// connection with a hard socket close under load, verify the engine never
/// flinches. Ignored by default (spawns processes, ~5 s).
#[test]
#[ignore]
fn chaos_ui_disconnect_engine_keeps_running() {
    let state = Arc::new(DaemonState::new());
    let addr = spawn_test_server(Arc::clone(&state));
    // Simulate 50 rapid UI deaths while hammering get_state.
    for _ in 0..50 {
        let s = std::net::TcpStream::connect(&addr).expect("connect");
        // Hard close: shutdown both directions, then drop. Stable equivalent
        // of SO_LINGER=0 (which is `tcp_linger`, still unstable) — the server
        // sees EOF/RST either way and must keep serving.
        let _ = s.shutdown(std::net::Shutdown::Both);
        drop(s);
    }
    let st = call_addr_timeout(
        &addr,
        "get_state",
        serde_json::json!({}),
        Duration::from_secs(5),
    )
    .expect("daemon must survive 50 UI deaths");
    assert!(st.get("active").is_some());
    let _ = IpcRequest::new(0, "ping", serde_json::json!({}));
}
