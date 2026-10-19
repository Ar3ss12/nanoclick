//! Stage E UI proxy: every tauri command becomes an IPC request.
//! Timeout 800 ms (the page already shows `busy` that long + toasts).
use crate::daemon_transport::{daemon_tcp_addr, read_line, write_line};
use crate::ipc::{IpcRequest, IpcResponse};
use std::io::{BufReader, BufWriter};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
pub const IPC_TIMEOUT: Duration = Duration::from_millis(800);
/// Send one command, wait for the matching `id` (blocking, caller thread).
pub fn call(cmd: &str, params: serde_json::Value) -> Result<serde_json::Value, String> {
    call_timeout(cmd, params, IPC_TIMEOUT)
}
pub fn call_timeout(
    cmd: &str,
    params: serde_json::Value,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    call_addr_timeout(&daemon_tcp_addr(), cmd, params, timeout)
}
/// Same as `call_timeout` but against an explicit address (tests bind their
/// own loopback port so they never touch the production daemon).
pub fn call_addr_timeout(
    addr: &str,
    cmd: &str,
    params: serde_json::Value,
    timeout: Duration,
) -> Result<serde_json::Value, String> {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let req = IpcRequest::new(id, cmd, params);
    let stream =
        std::net::TcpStream::connect(addr).map_err(|e| format!("daemon unreachable: {e}"))?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();
    let mut reader = BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
    let mut writer = BufWriter::new(stream);
    write_line(&mut writer, &req.encode()).map_err(|e| format!("ipc write: {e}"))?;
    let mut buf = String::new();
    let line = read_line(&mut reader, &mut buf).map_err(|e| format!("ipc read: {e}"))?;
    let line = line.ok_or_else(|| "daemon closed connection".to_string())?;
    let resp = IpcResponse::decode(&line).ok_or_else(|| "malformed response".to_string())?;
    if resp.id != id {
        return Err(format!("id mismatch {} != {id}", resp.id));
    }
    if resp.ok {
        Ok(resp.payload)
    } else {
        Err(resp
            .payload
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("daemon error")
            .to_string())
    }
}
