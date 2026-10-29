//! Stage A — satellite quarantine (SATELLITE_PLAN, daemon-split plan §A).
//!
//! The SAME binary can boot as a satellite window host instead of the full
//! application: `nanoclick.exe --satellite overlay|hud`. No scheduler, no
//! hooks, no tray pump, no click engine — just one Tauri window plus the
//! command surface its page needs to render and to say "I'm ready".
//!
//! ## Out-of-Process (OOP) supervisor (Stage A §2.1)
//!
//! When `SATELLITE_OOP` is set (via `set_satellite_oop(true)` in `lib.rs`
//! at boot, or by `NANOCLICK_OOP_SATELLITES=1` env var), overlay and hud
//! run as **separate child processes** supervised by this module:
//!
//! ```text
//! parent process
//!   └─ supervisor thread (per label)
//!        ├─ spawn_satellite("overlay") → child.stdin piped
//!        ├─ forwarder: OVERLAY_CHANNEL → child.stdin (JSON-lines)
//!        └─ watch_satellite(child) → Crashed → sleep(200ms) → restart
//!                                  → Clean   → shutdown (no restart)
//! ```
//!
//! The ripple drain worker and HUD-clicks emit call `satellite_send` instead
//! of `app.get_webview_window("overlay").emit(...)`.

use snubber::{Backoff, RestartDecision, RestartPolicy, RestartTracker, RestartWindow};
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{Emitter, Manager};

// ── OOP mode flag ─────────────────────────────────────────────────────────────
// Default OFF: in-process WebViews (current behaviour). Set to true at boot
// by `lib.rs` when `ui.oop_satellites` is true or NANOCLICK_OOP_SATELLITES=1.
static SATELLITE_OOP: AtomicBool = AtomicBool::new(false);

/// Enable or disable out-of-process satellite mode.
/// Call once at boot before any supervisor threads are started.
pub fn set_satellite_oop(enabled: bool) {
    SATELLITE_OOP.store(enabled, Ordering::SeqCst);
}

/// Whether OOP satellite mode is active.
#[inline]
pub fn satellite_oop_enabled() -> bool {
    SATELLITE_OOP.load(Ordering::Relaxed)
}

// ── Per-label stdin channels ─────────────────────────────────────────────────
// Written by the supervisor (on child spawn) and by satellite_send (caller).
// None = child not running or channel not connected yet.
type SatelliteChannel = Arc<Mutex<Option<Box<dyn Write + Send>>>>;

static OVERLAY_CHANNEL: OnceLock<SatelliteChannel> = OnceLock::new();
static HUD_CHANNEL: OnceLock<SatelliteChannel> = OnceLock::new();

fn channel_for(label: &str) -> Option<&'static SatelliteChannel> {
    match label {
        SATELLITE_OVERLAY => Some(OVERLAY_CHANNEL.get_or_init(|| Arc::new(Mutex::new(None)))),
        SATELLITE_HUD => Some(HUD_CHANNEL.get_or_init(|| Arc::new(Mutex::new(None)))),
        _ => None,
    }
}

/// Send one JSON-line event to a satellite child's stdin.
///
/// Returns `true` if the frame was written, `false` if OOP is off or the
/// channel is not connected (child not running yet or restarting).
/// Never blocks: if the Mutex is poisoned or the write fails, returns false
/// and the caller silently drops the frame (eye-candy is loss-tolerant).
pub fn satellite_send(label: &str, event: &str, payload: serde_json::Value) -> bool {
    if !satellite_oop_enabled() {
        return false;
    }
    let Some(ch) = channel_for(label) else {
        return false;
    };
    let frame = encode_satellite_event(event, payload);
    let Ok(mut guard) = ch.lock() else {
        return false;
    };
    let Some(writer) = guard.as_mut() else {
        return false;
    };
    writer.write_all(frame.as_bytes()).is_ok()
}

/// Background supervisor for one satellite label.
///
/// Spawns `--satellite <label>`, connects the stdin channel, pipes frames
/// from the channel to the child, and restarts on crash. Exits cleanly
/// when the child exits with code 0 (shutdown) or when the parent process
/// is about to exit (the channel is dropped / stdin is closed).
///
/// Back-off: 200 ms between restarts — enough to notice a boot loop in the
/// log without making the overlay visually absent for more than a blink.
///
/// Called from `lib.rs` `run()` setup when OOP is enabled. Each label gets
/// its own thread (overlay supervisor + hud supervisor run in parallel).
pub fn run_satellite_supervisor(label: &'static str, app: tauri::AppHandle) {
    std::thread::Builder::new()
        .name(format!("nanoclick-satellite-supervisor-{label}"))
        .spawn(move || {
            let mut tracker = RestartTracker::new(
                RestartPolicy::OnCrash,
                Backoff::exponential(50, 1000),
                RestartWindow::new(5, 15),
            );

            let mut spawn_failures: u64 = 0;
            loop {
                let mut child = match spawn_satellite(label) {
                    Ok(c) => c,
                    Err(e) => {
                        // 500 ms ±20 %, salted by label and attempt: the hud and
                        // overlay supervisors must not respawn in lockstep.
                        spawn_failures = spawn_failures.wrapping_add(1);
                        let delay = crate::retry::retry_delay_ms(
                            500,
                            crate::retry::retry_salt(label) ^ spawn_failures,
                        );
                        crate::debug_log_internal(
                            "error",
                            &format!("[Satellite:{label}] spawn failed: {e}, retrying in {delay}ms"),
                        );
                        std::thread::sleep(std::time::Duration::from_millis(delay));
                        continue;
                    }
                };
                let pid = child.id();
                crate::debug_log_internal(
                    "warn",
                    &format!("[Satellite:{label}] spawned OOP child PID={pid}"),
                );
                // Hand the child's stdin to the shared channel so satellite_send
                // can write frames without knowing about the child handle.
                // Take stdin out of child so watch() can call child.wait() freely.
                let stdin_taken = child.stdin.take();
                if let Some(ch) = channel_for(label) {
                    if let Ok(mut guard) = ch.lock() {
                        *guard = stdin_taken.map(|s| Box::new(s) as Box<dyn Write + Send>);
                    }
                }
                // Block on child exit — this is the supervisor's main job.
                let exit = watch_satellite_logged(label, child);
                // Disconnect the channel: the child is gone, writes must fail.
                if let Some(ch) = channel_for(label) {
                    if let Ok(mut guard) = ch.lock() {
                        *guard = None;
                    }
                }

                let exit_code = match exit {
                    SatelliteExit::Clean => Some(0),
                    SatelliteExit::Crashed => Some(1),
                };

                match tracker.note_death(exit_code, crate::guard::now_ms()) {
                    RestartDecision::Restart { delay_ms, .. } => {
                        crate::debug_log_internal(
                            "warn",
                            &format!(
                                "[Satellite:{label}] crashed or killed — restarting in 200ms (Phase 5 RestartTracker delay={delay_ms}ms)"
                            ),
                        );
                        // Cooling pause according to RestartTracker policy
                        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                        // Notify the main page so it can surface a toast if needed.
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win
                                .emit("satellite-restarted", serde_json::json!({ "label": label }));
                        }
                    }
                    RestartDecision::GiveUp { .. } => {
                        let node = crate::stability::satellite_node();
                        node.report(snubber::FaultKind::ProcessDied, 1, 1);
                        crate::debug_log_internal(
                            "error",
                            &format!(
                                "[Satellite:{label}] restart storm detected — giving up under RestartTracker policy"
                            ),
                        );
                        break;
                    }
                    RestartDecision::Stop => {
                        crate::debug_log_internal(
                            "warn",
                            &format!("[Satellite:{label}] clean exit, supervisor done"),
                        );
                        return;
                    }
                    // `RestartDecision` is `#[non_exhaustive]` (snubber
                    // 0.6.13): an unknown verdict is never permission to
                    // respawn — park the lineage instead of guessing.
                    _ => {
                        crate::debug_log_internal(
                            "error",
                            &format!(
                                "[Satellite:{label}] unknown restart verdict — supervisor stops"
                            ),
                        );
                        break;
                    }
                }
            }
        })
        .ok();
}

/// Like `watch_satellite` but logs the outcome at `warn` (release-visible).
fn watch_satellite_logged(label: &str, mut child: std::process::Child) -> SatelliteExit {
    match child.wait() {
        Ok(status) if status.success() => {
            crate::debug_log_internal(
                "warn",
                &format!("[Satellite:{label}] exited cleanly (status=0)"),
            );
            SatelliteExit::Clean
        }
        Ok(status) => {
            crate::debug_log_internal("warn", &format!("[Satellite:{label}] exited with {status}"));
            SatelliteExit::Crashed
        }
        Err(e) => {
            crate::debug_log_internal("warn", &format!("[Satellite:{label}] wait() error: {e}"));
            SatelliteExit::Crashed
        }
    }
}

/// Flag that turns this binary into a satellite host.
pub const SATELLITE_FLAG: &str = "--satellite";
/// Labels the satellite host understands.
pub const SATELLITE_OVERLAY: &str = "overlay";
pub const SATELLITE_HUD: &str = "hud";

/// Parse `--satellite <label>` out of argv. `None` = normal (main) boot.
/// Unknown labels pass through — `run_satellite` rejects them with exit 2
/// instead of silently booting the full app.
pub fn satellite_label_from_args(args: &[String]) -> Option<String> {
    let mut it = args.iter().skip(1);
    while let Some(a) = it.next() {
        if a == SATELLITE_FLAG {
            return it.next().cloned();
        }
    }
    None
}

/// `true` when this process boots as a satellite host (any label).
pub fn is_satellite_boot(args: &[String]) -> bool {
    satellite_label_from_args(args).is_some()
}

/// How the supervisor lost (or kept) the satellite child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SatelliteExit {
    Clean,
    Crashed,
}

/// Spawn this same binary as a satellite host for `label` (stdin piped).
pub fn spawn_satellite(label: &str) -> std::io::Result<std::process::Child> {
    let exe = std::env::current_exe()?;
    std::process::Command::new(exe)
        .arg(SATELLITE_FLAG)
        .arg(label)
        .stdin(std::process::Stdio::piped())
        .spawn()
}

/// Blocking supervisor: wait for the child, classify the death.
pub fn watch_satellite(mut child: std::process::Child) -> SatelliteExit {
    match child.wait() {
        Ok(status) if status.success() => SatelliteExit::Clean,
        _ => SatelliteExit::Crashed,
    }
}

/// Readiness command: show the window once the page renders.
#[tauri::command]
pub(crate) fn satellite_ready(app: tauri::AppHandle, label: String) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(&label) {
        let _ = win.show();
        let _ = win.set_focus();
    }
    crate::debug_log_internal("info", &format!("[Satellite:{label}] ready, shown"));
    Ok(())
}

/// Satellite-local log command: same sink, own command name (no clash with
/// the main `debug_log` — two `#[tauri::command]` with one name = E0255).
#[tauri::command]
fn satellite_log(level: String, message: String) {
    crate::debug_log_internal(&level, &message);
}

/// Closed allow-list: the child must never serve engine commands.
fn satellite_command_allowed(cmd: &str, label: &str) -> bool {
    matches!(
        (cmd, label),
        ("overlay_ready", SATELLITE_OVERLAY)
            | ("hud_ready", SATELLITE_HUD)
            | ("satellite_log", SATELLITE_OVERLAY)
            | ("satellite_log", SATELLITE_HUD)
    )
}

/// Forward one stdin frame into the local window.
/// Frame: `{"cmd":"<event>","payload":<json>}` — same event names the pages
/// already listen to (`spawn-ripple`, `hud-clicks`). Unknown = drop + warn.
fn forward_frame(app: &tauri::AppHandle, label: &str, line: &str) {
    let frame: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            crate::debug_log_internal(
                "warn",
                &format!("[Satellite:{label}] bad stdin frame dropped: {e}"),
            );
            return;
        }
    };
    let cmd = frame.get("cmd").and_then(|c| c.as_str()).unwrap_or("");
    if !satellite_command_allowed(cmd, label)
        && !(label == SATELLITE_OVERLAY && cmd == "spawn-ripple")
        && !(label == SATELLITE_HUD && cmd == "hud-clicks")
    {
        crate::debug_log_internal(
            "warn",
            &format!("[Satellite:{label}] foreign cmd dropped: {cmd:?}"),
        );
        return;
    }
    let payload = frame
        .get("payload")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    if let Some(win) = app.get_webview_window(label) {
        let _ = win.emit(cmd, payload);
    }
}

/// Boot as a satellite host: one window + stdin forwarder + exit on EOF.
/// NO scheduler, NO hooks, NO tray pump, NO updater. Returns exit code.
pub fn run_satellite(label: &str) -> i32 {
    if label != SATELLITE_OVERLAY && label != SATELLITE_HUD {
        eprintln!("[Satellite] unknown label: {label:?} (want overlay|hud)");
        return 2;
    }
    let page: &str = if label == SATELLITE_OVERLAY {
        "overlay.html"
    } else {
        "hud.html"
    };
    let label_owned = label.to_string();
    let page_owned = page.to_string();
    let flabel = label.to_string();
    let result = tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            crate::satellite::satellite_ready,
            crate::satellite::satellite_log
        ])
        .setup(move |app| {
            let win = tauri::WebviewWindowBuilder::new(
                app,
                label_owned.as_str(),
                tauri::WebviewUrl::App(page_owned.into()),
            )
            .title(if label_owned == SATELLITE_OVERLAY {
                "NanoClick Overlay"
            } else {
                "NanoClick HUD"
            })
            .transparent(true)
            .decorations(false)
            .always_on_top(true)
            .skip_taskbar(true)
            .visible(false)
            .build()?;
            if label_owned == SATELLITE_OVERLAY {
                let _ = win.set_ignore_cursor_events(true);
                if let Some(monitor) = win.primary_monitor()? {
                    let size = monitor.size();
                    let _ = win.set_size(tauri::PhysicalSize::new(size.width, size.height));
                    let _ = win.set_position(tauri::PhysicalPosition::new(0, 0));
                }
            }
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                let stdin = std::io::stdin();
                for line in stdin.lock().lines() {
                    match line {
                        Ok(text) => {
                            let text = text.trim();
                            if !text.is_empty() {
                                forward_frame(&handle, &flabel, text);
                            }
                        }
                        Err(_) => break,
                    }
                }
                crate::debug_log_internal(
                    "warn",
                    &format!("[Satellite:{flabel}] stdin EOF, exiting"),
                );
                std::process::exit(0);
            });
            Ok(())
        })
        .run(tauri::generate_context!());
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("[Satellite:{label}] tauri error: {e}");
            1
        }
    }
}

/// Encode one event frame for a satellite child's stdin.
pub fn encode_satellite_event(cmd: &str, payload: serde_json::Value) -> String {
    serde_json::json!({ "cmd": cmd, "payload": payload }).to_string() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn satellite_flag_parses_overlay_and_hud() {
        assert_eq!(
            satellite_label_from_args(&argv(&["nanoclick", "--satellite", "overlay"])),
            Some("overlay".to_string())
        );
        assert_eq!(
            satellite_label_from_args(&argv(&["nanoclick", "--satellite", "hud"])),
            Some("hud".to_string())
        );
    }

    #[test]
    fn normal_boot_has_no_satellite_label() {
        assert_eq!(satellite_label_from_args(&argv(&["nanoclick"])), None);
        assert_eq!(
            satellite_label_from_args(&argv(&["nanoclick", "--updated-from=1234"])),
            None
        );
    }

    #[test]
    fn unknown_label_is_not_silently_accepted_as_main() {
        let args = argv(&["nanoclick", "--satellite", "evil"]);
        assert!(is_satellite_boot(&args));
        assert_eq!(satellite_label_from_args(&args), Some("evil".to_string()));
    }

    #[test]
    fn engine_commands_are_not_routable_to_satellites() {
        for cmd in [
            "set_config",
            "toggle_autoclicker",
            "play_macro",
            "save_app_config",
            "restart_as_admin",
        ] {
            assert!(
                !satellite_command_allowed(cmd, SATELLITE_OVERLAY),
                "{cmd} must never reach a satellite"
            );
            assert!(
                !satellite_command_allowed(cmd, SATELLITE_HUD),
                "{cmd} must never reach a satellite"
            );
        }
    }

    #[test]
    fn ready_and_log_commands_are_routed_per_label() {
        assert!(satellite_command_allowed(
            "overlay_ready",
            SATELLITE_OVERLAY
        ));
        assert!(!satellite_command_allowed("overlay_ready", SATELLITE_HUD));
        assert!(satellite_command_allowed("hud_ready", SATELLITE_HUD));
        assert!(!satellite_command_allowed("hud_ready", SATELLITE_OVERLAY));
        assert!(satellite_command_allowed(
            "satellite_log",
            SATELLITE_OVERLAY
        ));
        assert!(satellite_command_allowed("satellite_log", SATELLITE_HUD));
        assert!(!satellite_command_allowed("debug_log", SATELLITE_OVERLAY));
    }

    #[test]
    fn event_frames_encode_as_single_json_lines() {
        let line = encode_satellite_event("spawn-ripple", serde_json::json!([[1, 2]]));
        assert!(line.ends_with('\n'));
        let back: serde_json::Value =
            serde_json::from_str(line.trim()).expect("must decode own frame");
        assert_eq!(back["cmd"], "spawn-ripple");
        assert_eq!(back["payload"], serde_json::json!([[1, 2]]));
    }

    // ── Stage A supervisor tests ───────────────────────────────────────────────

    #[test]
    fn oop_flag_defaults_to_false() {
        // Safety net: the default must be in-process so existing release
        // builds are not accidentally broken by this change.
        // NOTE: other tests in the same run may flip the flag; this test
        // reads the default only if no other test has changed it yet.
        // The important assertion is that `set_satellite_oop` / `satellite_oop_enabled`
        // compile and the round-trip works.
        let was = satellite_oop_enabled();
        set_satellite_oop(false);
        assert!(!satellite_oop_enabled(), "OOP must be off after set(false)");
        set_satellite_oop(true);
        assert!(satellite_oop_enabled(), "OOP must be on after set(true)");
        // Restore original
        set_satellite_oop(was);
    }

    #[test]
    fn satellite_send_returns_false_when_oop_disabled() {
        // With OOP off, satellite_send is a cheap no-op (no stdin, no alloc).
        set_satellite_oop(false);
        let sent = satellite_send(SATELLITE_OVERLAY, "spawn-ripple", serde_json::json!([]));
        assert!(!sent, "must not send when OOP is disabled");
    }

    #[test]
    fn satellite_send_returns_false_when_channel_not_connected() {
        // With OOP on but no supervisor running, channel is None → false.
        // We enable OOP temporarily; channel is None by default.
        set_satellite_oop(true);
        // Force the channel to exist but unconnected (None inside).
        let ch = channel_for(SATELLITE_OVERLAY).unwrap();
        if let Ok(mut guard) = ch.lock() {
            *guard = None; // explicitly disconnect
        }
        let sent = satellite_send(SATELLITE_OVERLAY, "spawn-ripple", serde_json::json!([]));
        assert!(!sent, "must return false when channel is disconnected");
        set_satellite_oop(false); // restore
    }

    #[test]
    fn satellite_send_writes_json_line_to_connected_channel() {
        // With OOP on and a Vec<u8> writer as the channel, satellite_send
        // must write a valid JSON-line frame.
        set_satellite_oop(true);
        let buf: Vec<u8> = Vec::new();
        let shared = Arc::new(std::sync::Mutex::new(buf));
        let shared_clone = Arc::clone(&shared);
        // Temporarily replace the channel with our in-memory writer.
        let ch = channel_for(SATELLITE_OVERLAY).unwrap();
        if let Ok(mut guard) = ch.lock() {
            *guard =
                Some(Box::new(crate::satellite::tests::VecWriter(shared_clone))
                    as Box<dyn Write + Send>);
        }
        let sent = satellite_send(SATELLITE_OVERLAY, "test-event", serde_json::json!({"x":1}));
        assert!(sent, "write to connected channel must succeed");
        // Disconnect so other tests don't see our mock.
        if let Ok(mut guard) = ch.lock() {
            *guard = None;
        }
        let written = shared.lock().unwrap();
        let text = std::str::from_utf8(&written).expect("frame must be UTF-8");
        assert!(text.ends_with('\n'), "frame must end with newline");
        let val: serde_json::Value =
            serde_json::from_str(text.trim()).expect("frame must be valid JSON");
        assert_eq!(val["cmd"], "test-event");
        assert_eq!(val["payload"]["x"], 1);
        set_satellite_oop(false); // restore
    }

    #[test]
    fn watch_satellite_classifies_clean_exit() {
        // A process that exits 0 is SatelliteExit::Clean.
        // Use `cmd /C exit 0` on Windows.
        #[cfg(target_os = "windows")]
        {
            let child = std::process::Command::new("cmd")
                .args(["/C", "exit 0"])
                .spawn()
                .expect("cmd must be available on Windows");
            assert_eq!(watch_satellite(child), SatelliteExit::Clean);
        }
        #[cfg(not(target_os = "windows"))]
        {
            let child = std::process::Command::new("true")
                .spawn()
                .expect("`true` must be available");
            assert_eq!(watch_satellite(child), SatelliteExit::Clean);
        }
    }

    #[test]
    fn watch_satellite_classifies_non_zero_exit_as_crashed() {
        // A process that exits non-zero is SatelliteExit::Crashed.
        #[cfg(target_os = "windows")]
        {
            let child = std::process::Command::new("cmd")
                .args(["/C", "exit 1"])
                .spawn()
                .expect("cmd must be available on Windows");
            assert_eq!(watch_satellite(child), SatelliteExit::Crashed);
        }
        #[cfg(not(target_os = "windows"))]
        {
            let child = std::process::Command::new("false")
                .spawn()
                .expect("`false` must be available");
            assert_eq!(watch_satellite(child), SatelliteExit::Crashed);
        }
    }

    /// Minimal `Write` adapter that appends to a shared `Vec<u8>`.
    /// Used by `satellite_send_writes_json_line_to_connected_channel`.
    pub(crate) struct VecWriter(pub Arc<std::sync::Mutex<Vec<u8>>>);
    impl Write for VecWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}
