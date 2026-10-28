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
//! It must be reachable by nothing that kills the app: Task Manager's
//! "End task" and `taskkill /T` walk live parent links, so the boot path
//! starts a one-shot launcher (`--input-watchdog-detach`) that spawns the real
//! guardian and exits immediately. The guardian's recorded parent is then a
//! pid that no longer exists, and the tree kill cannot follow the chain to it.
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

    /// argv flag for the one-shot launcher hop. The launcher spawns the real
    /// guardian and exits at once, so a tree kill cannot follow the parent
    /// chain to the guardian.
    pub const DETACH_FLAG: &str = "--input-watchdog-detach";

    /// `SYNCHRONIZE` (0x0010_0000) is all the guardian needs: wait on the
    /// process handle, wait on the event handle.
    const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;
    const INFINITE_WAIT: u32 = u32::MAX;
    const WAIT_OBJECT_0: u32 = 0;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    /// Start outside the app's job object when it is allowed: a kill-on-close
    /// job would otherwise take the guardian down with the app.
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    const CLEAN_EVENT_PREFIX: &str = r"Local\NanoClickCleanExit_";

    /// Exit codes. The guardian is detached, so these are for tests and for a
    /// future parent that wants to observe the rescue.
    pub const EXIT_CLEAN: i32 = 0;
    const EXIT_USAGE: i32 = 2;
    /// Abnormal exit, all three releases accepted by `SendInput`.
    pub const EXIT_INJECTED: i32 = 3;
    /// Abnormal exit, at least one `SendInput` rejected (UIPI, no desktop).
    pub const EXIT_INJECT_PARTIAL: i32 = 4;

    /// Test hook: set to anything to skip `SendInput` (the death path is still
    /// exercised end-to-end; only the injection is suppressed).
    const DRY_RUN_ENV: &str = "NANOCLICK_WATCHDOG_DRY_RUN";

    /// L/R/M — the buttons a stalled clicker can leave down.
    const RESCUE_BUTTONS: [MouseButton; 3] =
        [MouseButton::Left, MouseButton::Right, MouseButton::Middle];

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
        // The launcher hop: a failure here is a missing safety net, not a boot
        // failure, so no retries and no error surface.
        let _ = spawn_self(&[
            WATCHDOG_FLAG.to_string(),
            pid.to_string(),
            name,
            DETACH_FLAG.to_string(),
        ]);
    }

    /// Spawn this same binary with `args`, no console attached, and — when the
    /// enclosing job object allows it — outside that job, so a kill-on-close
    /// job cannot take the guardian down with the app. `CreateProcess` fails
    /// with `ERROR_ACCESS_DENIED` when the job forbids breakaway, hence the
    /// fallback: a guardian inside the job still beats no guardian.
    fn spawn_self(args: &[String]) -> std::io::Result<std::process::Child> {
        let exe = std::env::current_exe()?;
        let build = |flags: u32| {
            let mut cmd = std::process::Command::new(&exe);
            cmd.args(args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .creation_flags(flags);
            cmd.spawn()
        };
        build(CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB).or_else(|_| build(CREATE_NO_WINDOW))
    }

    /// Detach hop: start the real guardian and return immediately. The
    /// guardian's recorded parent then points at this process, which is already
    /// gone by the time anything walks the tree.
    fn spawn_orphan(pid: u32, name: &str) {
        let _ = spawn_self(&[WATCHDOG_FLAG.to_string(), pid.to_string(), name.to_string()]);
    }

    /// What the argv says. Pure so the parsing is testable without spawning
    /// anything (`Some(Err(()))` = the flag is present but the pid is not).
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Invocation {
        pub pid: u32,
        pub name: String,
        pub detach: bool,
    }

    pub fn parse_invocation(args: &[String]) -> Option<Result<Invocation, ()>> {
        let idx = args.iter().position(|a| a == WATCHDOG_FLAG)?;
        let detach = args[idx..].iter().any(|a| a == DETACH_FLAG);
        let positional: Vec<&String> = args[idx + 1..]
            .iter()
            .filter(|a| a.as_str() != DETACH_FLAG)
            .collect();
        let pid = match positional.first().and_then(|p| p.parse::<u32>().ok()) {
            Some(p) => p,
            None => return Some(Err(())),
        };
        let name = positional
            .get(1)
            .map(|s| (*s).clone())
            .unwrap_or_else(|| clean_event_name(pid));
        Some(Ok(Invocation { pid, name, detach }))
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

    /// Entry point for `--input-watchdog <pid> [event] [--input-watchdog-detach]`.
    /// `Some(code)` = this process is the guardian (or its launcher) and must
    /// exit with `code`.
    pub fn run_if_watchdog(args: &[String]) -> Option<i32> {
        let invocation = match parse_invocation(args)? {
            Ok(invocation) => invocation,
            Err(()) => {
                eprintln!("[guardian] usage: {WATCHDOG_FLAG} <parent-pid> [clean-event]");
                return Some(EXIT_USAGE);
            }
        };
        let Invocation { pid, name, detach } = invocation;
        if detach {
            spawn_orphan(pid, &name);
            return Some(EXIT_CLEAN);
        }
        if watch(pid, &name) == ExitVerdict::Clean {
            return Some(EXIT_CLEAN);
        }
        let inject = std::env::var_os(DRY_RUN_ENV).is_none();
        let injected = if inject { release_buttons() } else { 0 };
        let _ = std::panic::catch_unwind(|| {
            crate::debug_log_internal(
                "warn",
                &format!(
                    "[Guardian] parent pid {pid} ended abnormally (inject={inject}, \
                     injected={injected}/{})",
                    RESCUE_BUTTONS.len()
                ),
            );
        });
        if !inject {
            return Some(EXIT_CLEAN);
        }
        Some(if injected as usize == RESCUE_BUTTONS.len() {
            EXIT_INJECTED
        } else {
            EXIT_INJECT_PARTIAL
        })
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

    /// Release L/R/M and report how many events the OS actually inserted.
    /// Direct platform primitives: the guardian's own state is clean, and this
    /// path must not depend on the dead process's log/config globals.
    fn release_buttons() -> u32 {
        RESCUE_BUTTONS
            .iter()
            .filter(|button| crate::platform::windows::keyboard::mouse_up(**button))
            .count() as u32
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
                Some(EXIT_USAGE)
            );
        }

        #[test]
        fn invocation_parses_pid_name_and_detach() {
            let args: Vec<String> = ["nanoclick.exe", WATCHDOG_FLAG, "4242"]
                .iter()
                .map(|s| s.to_string())
                .collect();
            let inv = parse_invocation(&args)
                .expect("flag present")
                .expect("pid parses");
            assert_eq!(inv.pid, 4242);
            assert_eq!(inv.name, clean_event_name(4242));
            assert!(!inv.detach);

            let mut with_name = args.clone();
            with_name.push(r"Local\NanoClickCleanExit_4242".into());
            with_name.push(DETACH_FLAG.into());
            let inv = parse_invocation(&with_name)
                .expect("flag present")
                .expect("pid parses");
            assert_eq!(inv.name, r"Local\NanoClickCleanExit_4242");
            assert!(
                inv.detach,
                "the detach flag must not be read as the event name"
            );
        }

        #[test]
        fn invocation_without_the_flag_is_not_ours() {
            assert!(parse_invocation(&["nanoclick.exe".to_string()]).is_none());
            let bad: Vec<String> = ["exe", WATCHDOG_FLAG, "not-a-pid"]
                .iter()
                .map(|s| s.to_string())
                .collect();
            assert!(
                parse_invocation(&bad).expect("flag present").is_err(),
                "a non-numeric pid must be a usage error, not a default"
            );
        }

        #[test]
        fn detach_positionals_are_never_mistaken_for_the_name() {
            // The launcher hop is spawned as `<pid> <name> --detach`; a
            // regression that counted the flag as a positional would wait on
            // the wrong event and inject on every clean exit.
            let args: Vec<String> = [WATCHDOG_FLAG, "77", DETACH_FLAG]
                .iter()
                .map(|s| s.to_string())
                .collect();
            let inv = parse_invocation(&args).unwrap().unwrap();
            assert_eq!(inv.name, clean_event_name(77));
            assert!(inv.detach);
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
    clean_event_name, mark_clean_exit, parse_invocation, run_if_watchdog, spawn_guardian,
    ExitVerdict, Invocation, DETACH_FLAG, EXIT_CLEAN, EXIT_INJECTED, EXIT_INJECT_PARTIAL,
    WATCHDOG_FLAG,
};

#[cfg(not(target_os = "windows"))]
pub const WATCHDOG_FLAG: &str = "--input-watchdog";

#[cfg(not(target_os = "windows"))]
pub const DETACH_FLAG: &str = "--input-watchdog-detach";

#[cfg(not(target_os = "windows"))]
pub const EXIT_CLEAN: i32 = 0;

#[cfg(not(target_os = "windows"))]
pub const EXIT_INJECTED: i32 = 3;

#[cfg(not(target_os = "windows"))]
pub const EXIT_INJECT_PARTIAL: i32 = 4;

#[cfg(not(target_os = "windows"))]
pub fn spawn_guardian() {}

#[cfg(not(target_os = "windows"))]
pub fn mark_clean_exit() {}

#[cfg(not(target_os = "windows"))]
pub fn run_if_watchdog(_args: &[String]) -> Option<i32> {
    None
}
