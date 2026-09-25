//! Focus-loss watcher — instant stop when the foreground app changes mid-run.
//!
//! The click loop sleeps most of its life inside `wait_until`, so a focus
//! check that lives only inside the loop reaches the user late. This watcher
//! runs on its own thread for a guarded run: every [`FOCUS_WATCH_MS`] it reads
//! the foreground DIRECTLY (no TTL cache) and, on a change, flips `active`
//! off, wakes the sleeper via the shared stop event, and records the thief.
//!
//! Zero-jitter: the watcher NEVER touches the click hot path — only atomics.
//! The click thread owns the single `focus-loss-paused` emit (or the watcher
//! emits it when it wins the exactly-once claim). Guard OFF = no thread.

use super::FOCUS_WATCH_MS;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Shared stop signal written by the watcher, read by the click loop.
///
/// `claimed` guarantees exactly-once notify: whoever first flips it
/// false→true owns the `focus-loss-paused` emit.
pub struct FocusWatchStop {
    /// Set when focus left the session exe.
    pub requested: AtomicBool,
    /// Exactly-once emit claim (watcher vs click loop race).
    pub claimed: AtomicBool,
    /// The exe that stole focus (toast payload).
    pub thief_exe: Mutex<Option<String>>,
    /// Wakes a click thread sleeping in `wait_until` immediately.
    /// `None` while no run owns it; set at run start, never cleared mid-run
    /// (only `reset()` clears the stop flags, not the wake handle).
    /// NOTE: on Windows `NativeEventHandle = Arc<AtomicBool>` (not Option),
    /// so this holds `Some(handle)`; on linux/macos the alias itself is
    /// `Option<Arc<AtomicBool>>`, hence the double Option nesting.
    pub wake: Mutex<Option<crate::platform::NativeEventHandle>>,
}

impl FocusWatchStop {
    pub fn new() -> Self {
        FocusWatchStop {
            requested: AtomicBool::new(false),
            claimed: AtomicBool::new(false),
            thief_exe: Mutex::new(None),
            wake: Mutex::new(None),
        }
    }

    /// Reset for the next run (run start, before arming).
    pub fn reset(&self) {
        self.requested.store(false, Ordering::Relaxed);
        self.claimed.store(false, Ordering::Relaxed);
        if let Ok(mut g) = self.thief_exe.lock() {
            *g = None;
        }
    }

    /// Record a focus-loss event. Returns `true` for the single caller that
    /// owns the `focus-loss-paused` emit (first claim wins).
    pub fn request_stop(&self, thief: Option<String>) -> bool {
        if let Ok(mut g) = self.thief_exe.lock() {
            *g = thief;
        }
        self.requested.store(true, Ordering::Release);
        // Wake the sleeper: a click thread parked in `wait_until` sees the
        // flag on its next 1 ms poll and unwinds to the choke point.
        // `wake` holds Option<NativeEventHandle>; on linux/macos the alias
        // itself is Option<Arc> (double nesting), on Windows it is Arc.
        if let Ok(g) = self.wake.lock() {
            match g.as_ref() {
                Some(h) => {
                    #[cfg(target_os = "windows")]
                    h.store(true, Ordering::Release);
                    #[cfg(not(target_os = "windows"))]
                    if let Some(h) = h {
                        h.store(true, Ordering::Release);
                    }
                }
                None => {}
            }
        }
        !self.claimed.swap(true, Ordering::AcqRel)
    }

    /// Non-owning poll for the click loop: is a stop pending?
    pub fn is_requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }

    /// Take the recorded thief exe (payload for the UI toast).
    pub fn take_thief(&self) -> Option<String> {
        self.thief_exe.lock().ok().and_then(|mut g| g.take())
    }
}

impl Default for FocusWatchStop {
    fn default() -> Self {
        Self::new()
    }
}

/// One watcher decision, kept pure and separate from the OS.
///
/// `fg` is the foreground image name for this tick (`None` = unknown: lock
/// screen, elevated window, or a failed lookup). Exposed so tests never have to
/// read live Win32 state — a test that drives the REAL probe asserts a property
/// of the developer's desktop, not of this code (under `cargo test` the
/// foreground window is the console, never the test binary).
pub fn focus_lost(focus_guard: &crate::scheduler::FocusGuard, fg: Option<&str>) -> bool {
    focus_guard.is_enabled() && focus_guard.should_pause(fg)
}

/// Spawn the watcher thread for one guarded run.
///
/// * `focus_guard` — armed session baseline (read-only from here).
/// * `active` — run flag; the watcher clears it on focus loss.
/// * `stop` — shared [`FocusWatchStop`] (wake handle must be set already).
/// * `app_handle` — used ONLY to emit `focus-loss-paused` when this thread
///   wins the exactly-once claim; `None` in tests.
/// * `should_exit` — set by the click thread on run end so the watcher never
///   outlives the run.
pub fn spawn_focus_watcher(
    focus_guard: Arc<crate::scheduler::FocusGuard>,
    active: Arc<AtomicBool>,
    stop: Arc<FocusWatchStop>,
    app_handle: Option<tauri::AppHandle>,
    should_exit: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    spawn_focus_watcher_with(
        focus_guard,
        active,
        stop,
        app_handle,
        should_exit,
        crate::platform::get_foreground_process_name,
    )
}

/// [`spawn_focus_watcher`] with the foreground read injected.
///
/// The run path passes the real probe: a direct lookup, deliberately NOT the
/// TTL cache, because staleness here equals reaction latency. Tests pass a stub
/// so own / foreign / unknown foreground is exercised deterministically. The
/// production entry point above stays a one-liner, so the shipped behaviour is
/// untouched.
pub fn spawn_focus_watcher_with(
    focus_guard: Arc<crate::scheduler::FocusGuard>,
    active: Arc<AtomicBool>,
    stop: Arc<FocusWatchStop>,
    app_handle: Option<tauri::AppHandle>,
    should_exit: Arc<AtomicBool>,
    probe: fn() -> Option<String>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        loop {
            if should_exit.load(Ordering::Relaxed) || !active.load(Ordering::Relaxed) {
                break;
            }
            // Guard OFF costs no syscall at all: the probe is only read while
            // the guard is armed.
            let fg = if focus_guard.is_enabled() {
                probe()
            } else {
                None
            };
            if focus_lost(&focus_guard, fg.as_deref()) {
                active.store(false, Ordering::Relaxed);
                if stop.request_stop(fg) {
                    // Won the claim: notify now, don't wait for unwind.
                    if let Some(ref app) = app_handle {
                        use tauri::Emitter;
                        let _ = app.emit("focus-loss-paused", stop.take_thief());
                    }
                }
                break;
            }
            thread::sleep(Duration::from_millis(FOCUS_WATCH_MS));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::FocusGuard;

    #[test]
    fn request_stop_claim_is_exactly_once() {
        let s = FocusWatchStop::new();
        assert!(!s.is_requested());
        assert!(s.request_stop(Some("game.exe".into())));
        assert!(s.is_requested());
        assert!(!s.request_stop(Some("other.exe".into())));
        assert_eq!(s.take_thief().as_deref(), Some("other.exe"));
    }

    #[test]
    fn reset_clears_for_next_run() {
        let s = FocusWatchStop::new();
        assert!(s.request_stop(Some("game.exe".into())));
        s.reset();
        assert!(!s.is_requested());
        assert!(s.take_thief().is_none());
        assert!(s.request_stop(None));
    }

    /// Run one watcher for a few ticks with an injected foreground probe and
    /// return `(active, stop)` after the thread was told to exit and joined.
    fn run_watcher(
        probe: fn() -> Option<String>,
        enabled: bool,
        baseline: &str,
    ) -> (bool, Arc<FocusWatchStop>) {
        let guard = Arc::new(FocusGuard::new(enabled));
        guard.arm(Some(baseline.to_string()));
        let active = Arc::new(AtomicBool::new(true));
        let stop = Arc::new(FocusWatchStop::new());
        let exit = Arc::new(AtomicBool::new(false));
        let handle = spawn_focus_watcher_with(
            Arc::clone(&guard),
            Arc::clone(&active),
            Arc::clone(&stop),
            None,
            Arc::clone(&exit),
            probe,
        );
        // Four ticks at the production cadence: enough for several probes,
        // small enough to keep the suite fast.
        thread::sleep(Duration::from_millis(FOCUS_WATCH_MS * 4));
        exit.store(true, Ordering::Relaxed);
        let _ = handle.join();
        (active.load(Ordering::Relaxed), stop)
    }

    #[test]
    fn focus_lost_reads_the_guard_not_the_os() {
        let guard = FocusGuard::new(true);
        guard.arm(Some("baseline.exe".into()));
        assert!(!focus_lost(&guard, Some("baseline.exe")), "unchanged focus keeps running");
        assert!(!focus_lost(&guard, None), "an unknown foreground fails open");
        assert!(focus_lost(&guard, Some("intruder.exe")), "a foreign foreground stops the run");
        if let Some(own) = crate::platform::own_exe_name() {
            assert!(!focus_lost(&guard, Some(own.as_str())), "our own image is exempt");
        }
        let off = FocusGuard::new(false);
        off.arm(Some("baseline.exe".into()));
        assert!(!focus_lost(&off, Some("intruder.exe")), "guard OFF never stops");
    }

    /// Regression guard for a test-side trap, not a product bug: this test used
    /// to drive the PRODUCTION watcher, which reads the live foreground window.
    /// Under `cargo test` that window is the console — never the test binary —
    /// so the assertion failed on every machine while the shipped behaviour (a
    /// foreign window stops the run) was correct. The probe is injected now.
    #[test]
    fn watcher_ignores_own_foreground() {
        let (active, stop) = run_watcher(crate::platform::own_exe_name, true, "baseline.exe");
        assert!(active, "our own window must never stop the run");
        assert!(!stop.is_requested());
    }

    #[test]
    fn watcher_stops_on_foreign_foreground() {
        let (active, stop) = run_watcher(|| Some("intruder.exe".into()), true, "baseline.exe");
        assert!(!active, "a foreign foreground must stop the run");
        assert!(stop.is_requested());
        assert_eq!(stop.take_thief().as_deref(), Some("intruder.exe"));
    }

    #[test]
    fn watcher_fails_open_on_unknown_foreground() {
        let (active, stop) = run_watcher(|| None, true, "baseline.exe");
        assert!(active, "an unknown foreground (lock screen) must never stop the run");
        assert!(!stop.is_requested());
    }

    #[test]
    fn disabled_guard_never_touches_the_run() {
        let (active, stop) = run_watcher(|| Some("intruder.exe".into()), false, "baseline.exe");
        assert!(active, "guard OFF must leave the run alone");
        assert!(!stop.is_requested());
    }
}
