//! # Out-of-process input rescue for exits that cannot unwind.
//!
//! In-process RAII (`MouseHoldGuard`, snubber's erased-guard rollback) releases
//! a held button on every *unwinding* exit. It cannot run when the process is
//! aborted, killed from Task Manager, taken down by OOM or crashed below Rust
//! (Tauri → wry → WebView2 COM). The OS keeps the injected button down and no
//! code of ours is left to let it go.
//!
//! Only another process can fix that, so every process that can inject input
//! spawns a guardian of itself at boot:
//!
//! ```text
//! <exe> --input-watchdog <parent-pid> <clean-event-name>
//! ```
//!
//! The guardian opens the parent process, blocks on it, and when it ends checks
//! a manual-reset event that a *deliberate* shutdown sets. Anything else —
//! abort, kill, crash, a panic that escaped to the top — releases L/R/M with
//! `SendInput` before the guardian exits.
//!
//! Cost: one blocked process per engine process (no window, no Tauri, no
//! WebView — the flag is handled before `run()`).

#[cfg(target_os = "windows")]
mod imp {
    use crate::core::action::MouseButton;
    use std::os::windows::process::CommandExt;
    use std::sync::OnceLock;

    // Raw kernel32 declarations, same style as `uipi.rs`. The guardian has to
    // keep working when everything else about the process is broken, so it
    // depends on nothing but the OS: no win32 type wrappers, no log state.
    #[link(name = "kernel32")]
    extern "system" {
        fn CreateEventW(
            lp_event_attributes: *const core::ffi::c_void,
            b_manual_reset: i32,
            b_initial_state: i32,
            lp_name: *const u16,
        ) -> isize;
        fn OpenEventW(dw_desired_access: u32, b_inherit_handle: i32, lp_name: *const u16) -> isize;
        fn SetEvent(h_event: isize) -> i32;
        fn OpenProcess(dw_desired_access: u32, b_inherit_handle: i32, dw_process_id: u32) -> isize;
        fn WaitForSingleObject(h_handle: isize, dw_milliseconds: u32) -> u32;
        fn CloseHandle(h_object: isize) -> i32;
    }

    /// argv flag that turns this binary into a guardian host.
    pub const WATCHDOG_FLAG: &str = "--input-watchdog";

    /// `SYNCHRONIZE` (0x0010_0000) is all the guardian needs: wait on the
    /// process handle, wait on the event handle.
    const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
    const INFINITE_WAIT: u32 = u32::MAX;
    const WAIT_OBJECT_0: u32 = 0;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CLEAN_EVENT_PREFIX: &str = r"Local\NanoClickCleanExit_";

    /// Test hook: set to anything to skip `SendInput` (the death path is still
    /// exercised end-to-end; only the injection is suppressed).
    const DRY_RUN_ENV: &str = "NANOCLICK_WATCHDOG_DRY_RUN";

    static CLEAN_EVENT: OnceLock<isize> = OnceLock::new();

    /// Per-process event name. `Local\` keeps it inside the logon session.
    pub fn clean_event_name(pid: u32) -> String {
        format!("{CLEAN_EVENT_PREFIX}{pid}")
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ExitVerdict {
        Clean,
        Abnormal,
    }

    /// The whole decision: a clean exit is "the process ended AND the deliberate
    /// shutdown marker was set". Every other combination is abnormal.
    pub fn verdict(process_ended: bool, event_signaled: bool) -> ExitVerdict {
        if process_ended && event_signaled {
            ExitVerdict::Clean
        } else {
            ExitVerdict::Abnormal
        }
    }

    fn wide(name: &str) -> Vec<u16> {
        name.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Arm the guardian for the current process. Called once at engine boot,
    /// after the single-instance gate (a forwarded second instance must not
    /// inject a release on its way out).
    pub fn spawn_guardian() {
        let pid = std::process::id();
        let name = clean_event_name(pid);
        let event_name = wide(&name);
        let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, event_name.as_ptr()) };
        if handle == 0 {
            return;
        }
        let _ = CLEAN_EVENT.set(handle);
        let exe = match std::env::current_exe() {
            Ok(e) => e,
            Err(_) => return,
        };
        // Detached, no console: the guardian must not outlive the parent by
        // more than one syscall, so no retries — a failure here is a missing
        // safety net, not a boot failure.
        let _ = std::process::Command::new(exe)
            .arg(WATCHDOG_FLAG)
            .arg(pid.to_string())
            .arg(&name)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }

    /// Mark this process as deliberately ending. The guardian sees the event
    /// and injects nothing. Safe to call more than once.
    pub fn mark_clean_exit() {
        if let Some(raw) = CLEAN_EVENT.get() {
            unsafe {
                SetEvent(*raw);
            }
        }
    }

    /// Entry point for `--input-watchdog <pid> [event]`. `Some(code)` = this
    /// process is the guardian and must exit with `code`.
    pub fn run_if_watchdog(args: &[String]) -> Option<i32> {
        let idx = args.iter().position(|a| a == WATCHDOG_FLAG)?;
        let pid = match args.get(idx + 1).and_then(|p| p.parse::<u32>().ok()) {
            Some(p) => p,
            None => {
                eprintln!("[guardian] usage: {WATCHDOG_FLAG} <parent-pid> [clean-event]");
                return Some(2);
            }
        };
        let name = args
            .get(idx + 2)
            .cloned()
            .unwrap_or_else(|| clean_event_name(pid));
        let inject = std::env::var_os(DRY_RUN_ENV).is_none();
        if watch(pid, &name) == ExitVerdict::Abnormal {
            if inject {
                release_buttons();
            }
            let _ = std::panic::catch_unwind(|| {
                crate::debug_log_internal(
                    "warn",
                    &format!("[Guardian] parent pid {pid} ended abnormally (inject={inject})"),
                );
            });
        }
        Some(0)
    }

    /// Block until `pid` ends, then classify the exit.
    fn watch(pid: u32, event_name: &str) -> ExitVerdict {
        let process = unsafe { OpenProcess(SYNCHRONIZE_ACCESS, 0, pid) };
        if process == 0 {
            // Gone before we could open it (already reaped) — nothing to wait
            // on, and no marker can be trusted. Abnormal.
            return ExitVerdict::Abnormal;
        }
        // Open the marker while the parent is still alive and HOLD it. The
        // parent's own handle dies with it, and a named object with no open
        // handles is destroyed — a late `OpenEventW` would fail and report
        // every clean exit as abnormal, injecting a release for nothing.
        // A manual-reset event keeps its signaled state while we hold it.
        let marker_name = wide(event_name);
        let marker = unsafe { OpenEventW(SYNCHRONIZE_ACCESS, 0, marker_name.as_ptr()) };
        let waited = unsafe { WaitForSingleObject(process, INFINITE_WAIT) };
        unsafe {
            CloseHandle(process);
        }
        let ended = waited == WAIT_OBJECT_0;
        let signaled = ended && handle_signaled(marker);
        if marker != 0 {
            unsafe {
                CloseHandle(marker);
            }
        }
        verdict(ended, signaled)
    }

    fn handle_signaled(handle: isize) -> bool {
        if handle == 0 {
            return false;
        }
        unsafe { WaitForSingleObject(handle, 0) == WAIT_OBJECT_0 }
    }

    #[cfg(test)]
    fn event_signaled(name: &str) -> bool {
        let event_name = wide(name);
        let handle = unsafe { OpenEventW(SYNCHRONIZE_ACCESS, 0, event_name.as_ptr()) };
        let signaled = handle_signaled(handle);
        if handle != 0 {
            unsafe {
                CloseHandle(handle);
            }
        }
        signaled
    }

    fn release_buttons() {
        // Direct platform primitives: the guardian's own state is clean, and
        // this path must not depend on the dead process's log/config globals.
        crate::platform::windows::keyboard::mouse_up(MouseButton::Left);
        crate::platform::windows::keyboard::mouse_up(MouseButton::Right);
        crate::platform::windows::keyboard::mouse_up(MouseButton::Middle);
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn normal_boot_is_not_a_guardian() {
            assert_eq!(run_if_watchdog(&["nanoclick.exe".into()]), None);
        }

        #[test]
        fn guardian_rejects_missing_pid() {
            assert_eq!(
                run_if_watchdog(&["nanoclick.exe".into(), WATCHDOG_FLAG.into()]),
                Some(2)
            );
        }

        #[test]
        fn clean_event_name_is_process_scoped() {
            assert_ne!(clean_event_name(1), clean_event_name(2));
            assert!(clean_event_name(7).ends_with("_7"));
        }

        #[test]
        fn verdict_matrix() {
            assert_eq!(verdict(true, true), ExitVerdict::Clean);
            assert_eq!(verdict(true, false), ExitVerdict::Abnormal);
            assert_eq!(verdict(false, true), ExitVerdict::Abnormal);
            assert_eq!(verdict(false, false), ExitVerdict::Abnormal);
        }

        #[test]
        fn clean_marker_roundtrip_through_win32_event() {
            // Not a real pid: the test must never collide with a live process.
            let name = clean_event_name(u32::MAX - 3);
            let event_name = wide(&name);
            let ev = unsafe { CreateEventW(std::ptr::null(), 1, 0, event_name.as_ptr()) };
            assert_ne!(ev, 0, "CreateEventW failed");
            assert!(
                !event_signaled(&name),
                "a fresh manual-reset event must read as unsignaled"
            );
            unsafe {
                SetEvent(ev);
            }
            assert!(
                event_signaled(&name),
                "SetEvent must be observable through a second handle"
            );
            unsafe {
                CloseHandle(ev);
            }
        }

        #[test]
        fn dead_process_without_marker_is_abnormal() {
            let mut child = std::process::Command::new("cmd")
                .args(["/C", "exit", "3"])
                .spawn()
                .expect("spawn cmd");
            let pid = child.id();
            let _ = child.wait();
            // Reaped child: either OpenProcess fails or the wait completes with
            // no marker set. Both must classify as Abnormal.
            assert_eq!(watch(pid, &clean_event_name(pid)), ExitVerdict::Abnormal);
        }

        #[test]
        fn watch_opens_the_marker_before_it_waits() {
            // Structural guard for the bug the handle-lifetime test above
            // describes: if `watch` opened the marker only AFTER the parent
            // ended, the named object would already be gone.
            let src = include_str!("input_guardian.rs");
            let body = &src[src
                .find("fn watch(pid: u32, event_name: &str)")
                .expect("watch must exist")..];
            let open = body
                .find("OpenEventW(SYNCHRONIZE_ACCESS, 0, marker_name.as_ptr())")
                .expect("watch must open the marker");
            let wait = body
                .find("WaitForSingleObject(process, INFINITE_WAIT)")
                .expect("watch must wait on the process");
            assert!(
                open < wait,
                "the marker must be opened while the parent still owns it: \
                 opening after the wait lets the object die and reports every \
                 clean exit as abnormal"
            );
        }

        #[test]
        fn held_marker_handle_keeps_the_state_alive_after_the_setter_dies() {
            // The regression this guards: the guardian must open the marker
            // BEFORE the parent ends. Its own handle is what keeps the named
            // object alive after the setter's handles close; opening later
            // fails, and every clean exit becomes a needless injection.
            let name = clean_event_name(u32::MAX - 4);
            let event_name = wide(&name);
            let setter = unsafe { CreateEventW(std::ptr::null(), 1, 0, event_name.as_ptr()) };
            assert_ne!(setter, 0, "CreateEventW failed");
            let guardian = unsafe { OpenEventW(SYNCHRONIZE_ACCESS, 0, event_name.as_ptr()) };
            assert_ne!(guardian, 0, "the guardian must see the marker at boot");
            unsafe {
                SetEvent(setter);
                // "Parent" ends: its handle closes, the object survives only
                // because the guardian holds one.
                CloseHandle(setter);
            }
            assert!(
                handle_signaled(guardian),
                "a held handle must still read the signaled state after the setter died"
            );
            unsafe {
                CloseHandle(guardian);
            }
            assert!(
                !event_signaled(&name),
                "closing the last handle destroys the name — a late open cannot work"
            );
        }
    }
}

#[cfg(target_os = "windows")]
pub use imp::{
    clean_event_name, mark_clean_exit, run_if_watchdog, spawn_guardian, ExitVerdict, WATCHDOG_FLAG,
};

#[cfg(not(target_os = "windows"))]
pub const WATCHDOG_FLAG: &str = "--input-watchdog";

#[cfg(not(target_os = "windows"))]
pub fn spawn_guardian() {}

#[cfg(not(target_os = "windows"))]
pub fn mark_clean_exit() {}

#[cfg(not(target_os = "windows"))]
pub fn run_if_watchdog(_args: &[String]) -> Option<i32> {
    None
}
