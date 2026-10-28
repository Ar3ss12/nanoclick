//! End-to-end proof for the out-of-process input guardian — the two claims the
//! unit tests cannot make:
//!
//! 1. `SendInput` L/R/M really injects from the guardian's own process. The
//!    only observable proof is the count `SendInput` returns, surfaced as the
//!    exit code (`3` = all three accepted, `4` = partial).
//! 2. The launcher hop survives a tree kill. `taskkill /F /T` follows live
//!    parent links, so the guardian is started by a one-shot launcher whose
//!    pid is already dead by the time the tree is walked.
//!
//! Both tests touch the desktop with three button-UP events. That is a no-op
//! unless a button is genuinely held down, which is exactly what the rescue
//! exists for. The guardian is the real `nanoclick-daemon` binary: the
//! watchdog flag is handled before any daemon boot, so nothing else starts.
#![cfg(windows)]

use nanoclick::input_guardian::{clean_event_name, DETACH_FLAG, EXIT_INJECTED, WATCHDOG_FLAG};
use std::io::{Read, Seek, SeekFrom};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DAEMON: &str = env!("CARGO_BIN_EXE_nanoclick-daemon");

fn log_len() -> u64 {
    std::fs::metadata(nanoclick::log_file_path())
        .map(|m| m.len())
        .unwrap_or(0)
}

fn log_tail_from(offset: u64) -> String {
    let mut file = match std::fs::File::open(nanoclick::log_file_path()) {
        Ok(file) => file,
        Err(_) => return String::new(),
    };
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return String::new();
    }
    let mut bytes = Vec::new();
    let _ = file.read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).into_owned()
}

fn wait_for_log(offset: u64, needle: &str, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    loop {
        let tail = log_tail_from(offset);
        if tail.contains(needle) {
            return tail;
        }
        if Instant::now() >= deadline {
            panic!("no log line containing {needle:?} within {timeout:?}; tail:\n{tail}");
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn wait_for_file(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("{} never appeared", path.display());
}

/// Gap 1: the guardian watches a real process, that process ends without
/// setting the clean-exit marker, and the guardian's own `SendInput` calls are
/// accepted by the OS.
#[test]
fn guardian_injects_three_releases_and_reports_exit_3() {
    let mut target = Command::new("ping")
        .args(["-n", "5", "127.0.0.1"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn the process the guardian will watch");
    let target_pid = target.id();
    // Nobody ever creates this event, so the exit is abnormal by definition.
    let marker = clean_event_name(target_pid);
    let offset = log_len();

    let status = Command::new(DAEMON)
        .arg(WATCHDOG_FLAG)
        .arg(target_pid.to_string())
        .arg(&marker)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .expect("the guardian must run to completion");

    let _ = target.wait();
    assert_eq!(
        status.code(),
        Some(EXIT_INJECTED),
        "the guardian must report all three SendInput events accepted \
         (4 = the OS rejected at least one, 0 = it misread the exit as clean)"
    );
    let tail = wait_for_log(
        offset,
        &format!("parent pid {target_pid} ended abnormally"),
        Duration::from_secs(5),
    );
    assert!(
        tail.contains("injected=3/3"),
        "the rescue must log the injected count; tail:\n{tail}"
    );
}

/// Gap 2: the app spawns a launcher, the launcher spawns the guardian and
/// exits. A `taskkill /F /T` on the app then cannot reach the guardian, which
/// still notices the app's death and injects.
///
/// PowerShell stands in for the app because it can hand its own pid to the
/// launcher (`$PID`), which is what makes the chain app -> launcher -> guardian
/// the real shape rather than a synthetic one.
#[test]
fn guardian_survives_a_tree_kill_of_its_parent() {
    let marker_file = std::env::temp_dir().join(format!(
        "nanoclick_guardian_e2e_{}.txt",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&marker_file);
    let offset = log_len();
    let script = format!(
        "$p = $PID; \
         Start-Process -FilePath '{DAEMON}' \
             -ArgumentList '{WATCHDOG_FLAG}', $p, '{DETACH_FLAG}' \
             -WindowStyle Hidden; \
         Set-Content -Path '{marker}' -Value ('launched ' + $p); \
         ping -n 120 127.0.0.1 > $null",
        marker = marker_file.display()
    );

    let mut app = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .expect("powershell must start");
    let app_pid = app.id();

    // The marker is written the moment the launcher exists. The launcher only
    // spawns the guardian and returns, so one more second guarantees its pid is
    // dead — and a dead pid is what breaks the tree walk.
    wait_for_file(&marker_file, Duration::from_secs(20));
    std::thread::sleep(Duration::from_millis(1000));

    let killed = Command::new("taskkill")
        .args(["/F", "/T", "/PID", &app_pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .expect("taskkill must run");
    assert!(
        killed.success(),
        "taskkill /F /T must find the parent {app_pid} — otherwise the tree was \
         empty and this test proves nothing"
    );
    let _ = app.wait();
    let _ = std::fs::remove_file(&marker_file);

    let tail = wait_for_log(
        offset,
        &format!("parent pid {app_pid} ended abnormally"),
        Duration::from_secs(20),
    );
    assert!(
        tail.contains("injected=3/3"),
        "the detached guardian must outlive the tree kill and inject all three \
         releases; tail:\n{tail}"
    );
}
