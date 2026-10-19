//! Stage D daemon supervisor: owns the UI child process.
//! UI death = respawn after 200 ms (same cadence as satellite guard).
//! Daemon death = external recovery (Task Scheduler / service) — never self.
use std::sync::atomic::{AtomicBool, Ordering};
/// Flag the daemon passes to the UI child so it boots as a thin remote.
pub const UI_CHILD_FLAG: &str = "--ui-child";
/// True when this process boots as a daemon-owned UI child.
pub fn is_ui_child(args: &[String]) -> bool {
    args.iter().skip(1).any(|a| a == UI_CHILD_FLAG)
}
static UI_STOP: AtomicBool = AtomicBool::new(false);
/// Ask the supervisor loop to stop (daemon shutdown).
pub fn stop_ui_supervisor() {
    UI_STOP.store(true, Ordering::Release);
}
/// Blocking supervisor: spawn current exe with `--ui-child`, respawn on crash.
/// Returns when `stop_ui_supervisor` fires or the child exits cleanly.
pub fn supervise_ui(on_event: impl Fn(UiExit)) {
    UI_STOP.store(false, Ordering::Release);
    loop {
        if UI_STOP.load(Ordering::Acquire) {
            break;
        }
        let exe = match std::env::current_exe() {
            Ok(e) => e,
            Err(_) => break,
        };
        let mut child = match std::process::Command::new(exe)
            .arg(UI_CHILD_FLAG)
            .stdin(std::process::Stdio::null())
            .spawn()
        {
            Ok(c) => c,
            Err(_) => {
                std::thread::sleep(std::time::Duration::from_millis(200));
                continue;
            }
        };
        match child.wait() {
            Ok(status) if status.success() => {
                on_event(UiExit::Clean);
                break;
            }
            _ => {
                on_event(UiExit::Crashed);
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        }
    }
}
/// How the UI child ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiExit {
    Clean,
    Crashed,
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ui_child_flag_parses() {
        assert!(is_ui_child(&["exe".into(), "--ui-child".into()]));
        assert!(!is_ui_child(&["exe".into()]));
    }
}
