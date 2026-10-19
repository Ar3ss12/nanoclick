//! Stage D transport: TCP loopback JSON-lines pipe (daemon <-> UI).
//! Named-pipe crate lands in Stage D-full; this file owns framing only:
//! read one `\n`-terminated line, write one line, flush. No threads here.
use std::io::{BufRead, Write};
/// Read one JSON line (without `\n`). `None` = EOF / dead peer.
pub fn read_line<R: BufRead>(r: &mut R, buf: &mut String) -> std::io::Result<Option<String>> {
    buf.clear();
    let n = r.read_line(buf)?;
    if n == 0 {
        return Ok(None);
    }
    Ok(Some(buf.trim_end_matches(['\r', '\n']).to_string()))
}
/// Write one JSON line + flush. Best-effort: caller counts failures.
pub fn write_line<W: Write>(w: &mut W, line: &str) -> std::io::Result<()> {
    w.write_all(line.as_bytes())?;
    if !line.ends_with('\n') {
        w.write_all(b"\n")?;
    }
    w.flush()
}
/// Loopback address for the Stage D pipe (127.0.0.1:44771).
/// Named pipe `\\.\pipe\nanoclick-daemon` in D-full; same envelopes.
pub const DAEMON_TCP_PORT: u16 = 44771;
pub fn daemon_tcp_addr() -> String {
    format!("127.0.0.1:{DAEMON_TCP_PORT}")
}
/// Serve ONE accepted stream with a request handler (shared by production
/// `daemon_server::serve` and by F-gate tests on their own loopback port).
pub fn serve_stream<S, F>(state: &S, stream: std::net::TcpStream, handle: F)
where
    F: Fn(&S, &crate::ipc::IpcRequest) -> crate::ipc::IpcResponse,
{
    use crate::ipc::{IpcRequest, IpcResponse};
    use std::io::{BufReader, BufWriter};
    let mut reader = match stream.try_clone() {
        Ok(c) => BufReader::new(c),
        Err(_) => return,
    };
    let mut writer = BufWriter::new(stream);
    let mut buf = String::new();
    while let Ok(Some(line)) = read_line(&mut reader, &mut buf) {
        let resp = match IpcRequest::decode(&line) {
            Some(req) => handle(state, &req),
            None => IpcResponse::err(0, "malformed json-line"),
        };
        if write_line(&mut writer, &resp.encode()).is_err() {
            break;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};
    #[test]
    fn line_round_trip() {
        let mut c = Cursor::new(Vec::new());
        write_line(&mut c, r#"{"id":1}"#).unwrap();
        let mut r = BufReader::new(Cursor::new(c.into_inner()));
        let mut b = String::new();
        assert_eq!(
            read_line(&mut r, &mut b).unwrap().as_deref(),
            Some(r#"{"id":1}"#)
        );
    }
    #[test]
    fn eof_is_none() {
        let mut r = BufReader::new(Cursor::new(Vec::new()));
        let mut b = String::new();
        assert_eq!(read_line(&mut r, &mut b).unwrap(), None);
    }
}
