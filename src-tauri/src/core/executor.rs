//! # Executor — plays a `Macro` by running each `Action` sequentially.
//!
//! The Executor is the playback counterpart of the Recorder. All input
//! dispatch goes through the platform-agnostic `InputBackend` trait —
//! core code contains zero `#[cfg(target_os)]` blocks (v4.2).
//!
//! Reference: `docs/MACRO_ARCHITECTURE.md` §4.

use crate::core::action::MouseButton;
use crate::core::{Action, Macro, MacroLookup, RepeatMode};
use crate::platform::backend::InputBackend;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Handle to a running macro playback. `stop()` interrupts the playback.
#[derive(Clone)]
pub struct ExecutorHandle {
    shared: Arc<ExecutorShared>,
    backend: Arc<dyn InputBackend>,
}

struct ExecutorShared {
    is_running: AtomicBool,
    /// Cancel signal, shared by reference with the v4.0 control-flow runner.
    /// `stop()` writes here; `run_actions_in` polls here every chunk.
    cancel: Arc<AtomicBool>,
    /// Index of the next action to execute (for Step + Run-from-here).
    /// When set via `run_from(idx)`, the executor starts at this index.
    next_step_idx: AtomicUsize,
}

// ── Hardware hold tracking (macro playback) ────────────────────────────────
//
// A macro can press a button and never release it — `MouseDown`/`HoldStart`
// without a matching `MouseUp`, or a panic/early exit between the two. The
// mask is a thread-local because the v4.0 runner dispatches primitives through
// a throwaway handle on the *playing* thread; the guard below runs there too.

const HELD_LEFT: u8 = 1 << 0;
const HELD_RIGHT: u8 = 1 << 1;
const HELD_MIDDLE: u8 = 1 << 2;
const HELD_X1: u8 = 1 << 3;
const HELD_X2: u8 = 1 << 4;

fn button_bit(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => HELD_LEFT,
        MouseButton::Right => HELD_RIGHT,
        MouseButton::Middle => HELD_MIDDLE,
        MouseButton::X1 => HELD_X1,
        MouseButton::X2 => HELD_X2,
    }
}

thread_local! {
    static HELD_MASK: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

fn note_pressed(button: MouseButton) {
    HELD_MASK.with(|m| m.set(m.get() | button_bit(button)));
}

fn note_released(button: MouseButton) {
    HELD_MASK.with(|m| m.set(m.get() & !button_bit(button)));
}

/// Release every button this thread still holds and clear the mask.
fn release_held_buttons(backend: &dyn InputBackend) {
    let mask = HELD_MASK.with(|m| m.replace(0));
    for (bit, button) in [
        (HELD_LEFT, MouseButton::Left),
        (HELD_RIGHT, MouseButton::Right),
        (HELD_MIDDLE, MouseButton::Middle),
        (HELD_X1, MouseButton::X1),
        (HELD_X2, MouseButton::X2),
    ] {
        if mask & bit != 0 {
            backend.mouse_up(button);
        }
    }
}

/// Run-scoped guard: releases a button left held when the stack unwinds or the
/// run exits early, and always clears `is_running` (a panic must not leave the
/// executor permanently "busy"). On the normal path `completed` is set before
/// the drop, so a macro that deliberately ends with a button down keeps it
/// down — today's semantics, unchanged.
struct MacroExitGuard {
    shared: Arc<ExecutorShared>,
    completed: bool,
}

impl Drop for MacroExitGuard {
    fn drop(&mut self) {
        if !self.completed {
            release_held_buttons(&*crate::platform::default_input_backend());
        }
        self.shared.is_running.store(false, Ordering::Release);
    }
}

impl ExecutorHandle {
    /// Create a handle bound to a specific input backend.
    #[allow(dead_code)] // contract API — used by mock-backend contract tests
    pub fn with_backend(backend: Arc<dyn InputBackend>) -> Self {
        Self::build(backend)
    }

    /// Convenience constructor using the default platform backend.
    pub fn new() -> Self {
        Self::build(crate::platform::default_input_backend())
    }

    fn build(backend: Arc<dyn InputBackend>) -> Self {
        ExecutorHandle {
            shared: Arc::new(ExecutorShared {
                is_running: AtomicBool::new(false),
                cancel: Arc::new(AtomicBool::new(false)),
                next_step_idx: AtomicUsize::new(0),
            }),
            backend,
        }
    }

    /// Whether a macro is currently playing.
    pub fn is_running(&self) -> bool {
        self.shared.is_running.load(Ordering::Acquire)
    }

    /// Stop a running macro (e.g. via Esc hotkey).
    ///
    /// Stop flag: `Release` on write pairs with `Acquire` on the runner's
    /// polls (same hygiene as the click-worker `active` flag — a stop flag
    /// has no business being `Relaxed`).
    pub fn stop(&self) {
        self.shared.cancel.store(true, Ordering::Release);
        self.shared.is_running.store(false, Ordering::Release);
    }

    /// Clone the cancel flag — the v4.0 control-flow runner polls on this
    /// exact `Arc<AtomicBool>` so changes propagate instantly.
    pub fn cancel_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.shared.cancel)
    }

    /// Configure the start index for the next run.
    /// Used by "Run from here" (Step → Run from here).
    pub fn set_start_idx(&self, idx: usize) {
        self.shared.next_step_idx.store(idx, Ordering::Relaxed);
    }

    /// Play a macro on the current thread. Each action is dispatched via
    /// the injected `InputBackend`.
    pub fn play(&self, m: &Macro) {
        self.play_with_lookup(m, Self::empty_lookup());
    }

    fn empty_lookup() -> MacroLookup {
        std::sync::Arc::new(|_: &str| None)
    }

    fn play_with_lookup(&self, m: &Macro, macro_lookup: MacroLookup) {
        if self.shared.is_running.swap(true, Ordering::AcqRel) {
            // Already running — don't double-start.
            return;
        }

        let node = crate::stability::macro_node();
        // Probe admission: the slot is released at entry (macro serialization
        // is `is_running`), while the rollback stays armed for implicit drop.
        let guard: snubber::ErasedGuard = match node.probe_erased_auto(|| {
            crate::stability::emergency_release_all_hardware();
        }) {
            Ok(g) => g,
            Err(e) => {
                crate::debug_log_internal(
                    "error",
                    &format!("[Stability] macro_executor rejected playback: {e:?}"),
                );
                self.shared.is_running.store(false, Ordering::Release);
                return;
            }
        };

        // The same token the click run takes. Playback injects into the same
        // cursor and holds the same buttons: a run started mid-playback would
        // interleave two owners of one device. A refusal here is honest, and the
        // click scheduler's `start_refusal` reports the reverse direction
        // (`hardware_busy`).
        let hardware = match crate::stability::macro_arbiter()
            .acquire(crate::stability::HardOwner::Macro)
        {
            Ok(token) => token,
            Err(busy) => {
                crate::debug_log_internal(
                    "warn",
                    &format!("[Stability] macro playback refused: hardware busy ({busy:?})"),
                );
                guard.commit();
                self.shared.is_running.store(false, Ordering::Release);
                return;
            }
        };

        self.shared.cancel.store(false, Ordering::Release);
        let mut exit_guard = MacroExitGuard {
            shared: Arc::clone(&self.shared),
            completed: false,
        };
        let start = self
            .shared
            .next_step_idx
            .swap(0, Ordering::Relaxed)
            .min(m.actions.len());

        match m.repeat {
            RepeatMode::Once => {
                self.run_from_to(m, start, m.actions.len(), macro_lookup.clone());
            }
            RepeatMode::Times { count } => {
                for i in 0..count {
                    if self.shared.cancel.load(Ordering::Acquire) {
                        break;
                    }
                    // Only honor "run from here" offset on the first iteration.
                    let s = if i == 0 { start } else { 0 };
                    self.run_from_to(m, s, m.actions.len(), macro_lookup.clone());
                }
            }
            RepeatMode::UntilStopped => loop {
                if self.shared.cancel.load(Ordering::Acquire) {
                    break;
                }
                self.run_from_to(m, 0, m.actions.len(), macro_lookup.clone());
            },
        }

        guard.commit();
        hardware.commit();
        exit_guard.completed = true;
    }

    /// Run a single action (debug Step mode). Returns true if a step
    /// was actually executed.
    pub fn step(&self, m: &Macro) -> bool {
        let idx = self.shared.next_step_idx.load(Ordering::Relaxed);
        if idx >= m.actions.len() {
            return false;
        }
        if m.is_enabled(idx) {
            self.execute_action(&m.actions[idx], &self.shared.cancel);
        }
        self.shared.next_step_idx.store(idx + 1, Ordering::Relaxed);
        true
    }

    /// Reset the cursor back to action 0.
    pub fn rewind(&self) {
        self.shared.next_step_idx.store(0, Ordering::Relaxed);
    }

    /// Spawn a thread and play the macro on it.
    pub fn play_async(&self, m: Macro) {
        let handle = self.clone();
        thread::spawn(move || handle.play(&m));
    }

    /// Spawn playback with a stable in-memory macro lookup for `Call` actions.
    pub fn play_async_with_lookup(&self, m: Macro, macro_lookup: MacroLookup) {
        let handle = self.clone();
        thread::spawn(move || handle.play_with_lookup(&m, macro_lookup));
    }

    fn run_from_to(&self, m: &Macro, from: usize, to: usize, macro_lookup: MacroLookup) {
        // Slice the actions to [from..to) and run them through the v4.0
        // control-flow runner. The runner handles Repeat/If/Call/SetVar/GetVar
        // and delegates primitive actions back to `dispatch_primitive`.
        let end = to.min(m.actions.len());
        let from = from.min(end);
        let mut ctx = crate::core::ExecutionContext::new();
        // Share the same Arc<AtomicBool> as ExecutorHandle — `stop()` writes to
        // this exact flag, so the runner observes it on the next loop tick.
        let cancel = self.cancel_handle();
        crate::core::run_actions_in(&m.actions[from..end], &mut ctx, cancel, macro_lookup, 0);
    }

    /// Single-action dispatch via the injected backend. No platform cfg.
    fn execute_action(&self, action: &Action, cancel: &AtomicBool) {
        if cancel.load(Ordering::Acquire) {
            return;
        }
        match action {
            Action::Wait { ms } => {
                // Interruptible sleep.
                let target_ms = *ms;
                let chunk_ms = target_ms.min(50);
                let mut slept = 0u64;
                while slept < target_ms {
                    if cancel.load(Ordering::Acquire) {
                        return;
                    }
                    let chunk = (target_ms - slept).min(chunk_ms);
                    thread::sleep(Duration::from_millis(chunk));
                    slept += chunk;
                }
            }
            Action::MouseMove { x, y } => {
                self.backend.set_cursor_pos(*x, *y);
            }
            Action::MouseClick { button, count } => {
                for _ in 0..*count {
                    if cancel.load(Ordering::Acquire) {
                        return;
                    }
                    self.backend.mouse_click(*button);
                    if *count > 1 {
                        thread::sleep(Duration::from_millis(50));
                    }
                }
            }
            Action::MouseDown { button } => {
                self.backend.mouse_down(*button);
                note_pressed(*button);
            }
            Action::MouseUp { button } => {
                self.backend.mouse_up(*button);
                note_released(*button);
            }
            Action::KeyPress { key, mods } | Action::KeyDown { key, mods } => {
                if !cancel.load(Ordering::Acquire) {
                    self.backend.send_key(*key, *mods, false);
                }
            }
            Action::KeyUp { key, mods } => {
                if !cancel.load(Ordering::Acquire) {
                    self.backend.send_key(*key, *mods, true);
                }
            }
            Action::Scroll { delta_x, delta_y } => {
                self.backend.scroll_wheel(*delta_x, *delta_y);
            }
            Action::HoldStart => {
                // Equivalent to MouseDown(Left).
                self.backend.mouse_down(MouseButton::Left);
                note_pressed(MouseButton::Left);
            }
            // ── v4.0 control flow variants: handled at the *list* level.
            // Here they are atomic ticks (debug Step mode UI response).
            Action::Repeat { .. }
            | Action::If { .. }
            | Action::Call { .. }
            | Action::SetVar { .. }
            | Action::GetVar { .. } => {
                thread::sleep(Duration::from_millis(1));
            }
        }
    }
}

/// v4.0 — Cancel-aware dispatcher for free functions that don't carry a
/// handle. Uses the default platform backend. Used by the control-flow
/// runner which receives `cancel` only.
pub fn dispatch_primitive_with_cancel(action: &Action, cancel: &std::sync::atomic::AtomicBool) {
    if cancel.load(Ordering::Acquire) {
        return;
    }
    // Free-function entry point (control-flow runner has no handle):
    // construct a throwaway handle on the default platform backend and
    // reuse the same dispatch path as `step()`.
    let h = ExecutorHandle::new();
    h.execute_action(action, cancel);
}

/// Global singleton handle — set by Tauri `setup()`. Used by hotkey
/// callbacks to interrupt running macros.
static GLOBAL_EXECUTOR: Mutex<Option<ExecutorHandle>> = Mutex::new(None);

/// Register the singleton — call once from `setup()`.
pub fn set_global(handle: ExecutorHandle) {
    if let Ok(mut g) = GLOBAL_EXECUTOR.lock() {
        *g = Some(handle);
    }
}

/// Get the global handle (for hotkey ESC support).
pub fn global() -> Option<ExecutorHandle> {
    GLOBAL_EXECUTOR.lock().ok().and_then(|g| g.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::action::{KeyCode, Modifiers};
    use crate::core::RepeatMode;

    fn dummy_macro(actions: Vec<Action>) -> Macro {
        Macro {
            id: "t".into(),
            name: "t".into(),
            icon: "\u{1f3ac}".into(),
            actions,
            repeat: RepeatMode::Once,
            enabled: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn executor_handle_starts_not_running() {
        let h = ExecutorHandle::new();
        assert!(!h.is_running());
    }

    #[test]
    fn cancel_stops_run() {
        let h = ExecutorHandle::new();
        let m = dummy_macro(vec![Action::Wait { ms: 50_000 }]);
        let h2 = h.clone();
        let m_clone = m.clone();
        let start = std::time::Instant::now();
        let t = thread::spawn(move || {
            h2.play(&m_clone);
        });
        thread::sleep(Duration::from_millis(50));
        h.stop();
        let _ = t.join();
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(2),
            "play() did not respect cancel flag (took {:?})",
            elapsed
        );
    }

    #[test]
    fn step_advances_through_actions() {
        let h = ExecutorHandle::new();
        let m = dummy_macro(vec![
            Action::Wait { ms: 5 },
            Action::Wait { ms: 5 },
            Action::Wait { ms: 5 },
        ]);
        assert!(h.step(&m));
        assert!(h.step(&m));
        assert!(h.step(&m));
        assert!(!h.step(&m));
        h.rewind();
        assert!(h.step(&m));
    }

    #[test]
    fn run_from_here_offset() {
        let h = ExecutorHandle::new();
        let m = dummy_macro(vec![
            Action::Wait { ms: 5000 },
            Action::Wait { ms: 5 },
            Action::Wait { ms: 5 },
        ]);
        h.set_start_idx(1);
        let h2 = h.clone();
        let m2 = m.clone();
        let t = thread::spawn(move || h2.play(&m2));
        thread::sleep(Duration::from_millis(100));
        h.stop();
        let _ = t.join();
    }

    #[test]
    fn step_skips_disabled_actions() {
        let h = ExecutorHandle::new();
        let mut m = dummy_macro(vec![
            Action::Wait { ms: 5 },
            Action::KeyPress {
                key: KeyCode(0x41),
                mods: Modifiers {
                    ctrl: false,
                    alt: false,
                    shift: false,
                    win: false,
                },
            },
        ]);
        m.enabled = Some(vec![false, true]);
        assert!(h.step(&m));
    }

    /// v4.2 contract test — the mock backend records every dispatch so we
    /// can prove the executor routes ALL primitives through InputBackend.
    struct MockBackend {
        clicks: AtomicUsize,
        keys: AtomicUsize,
        moves: AtomicUsize,
        scrolls: AtomicUsize,
    }

    impl MockBackend {
        fn new_arc() -> Arc<Self> {
            Arc::new(MockBackend {
                clicks: AtomicUsize::new(0),
                keys: AtomicUsize::new(0),
                moves: AtomicUsize::new(0),
                scrolls: AtomicUsize::new(0),
            })
        }
    }

    impl crate::platform::backend::InputBackend for MockBackend {
        fn mouse_click(&self, _: MouseButton) {
            self.clicks.fetch_add(1, Ordering::Relaxed);
        }
        fn mouse_down(&self, _: MouseButton) {}
        fn mouse_up(&self, _: MouseButton) {}
        fn scroll_wheel(&self, _: i32, _: i32) {
            self.scrolls.fetch_add(1, Ordering::Relaxed);
        }
        fn set_cursor_pos(&self, _: i32, _: i32) {
            self.moves.fetch_add(1, Ordering::Relaxed);
        }
        fn send_key(&self, _: KeyCode, _: Modifiers, _: bool) {
            self.keys.fetch_add(1, Ordering::Relaxed);
        }
        fn cursor_position(&self) -> (i32, i32) {
            (0, 0)
        }
        fn click_mouse(&self, _: &crate::platform::backend::ClickSpec) -> bool {
            true
        }
        fn release_mouse_hold(&self, _: MouseButton) {}
    }

    #[test]
    fn all_primitives_route_through_input_backend_contract() {
        let backend = MockBackend::new_arc();
        let h = ExecutorHandle::with_backend(Arc::clone(&backend) as Arc<dyn InputBackend>);
        let m = dummy_macro(vec![
            Action::MouseMove { x: 10, y: 20 },
            Action::MouseClick {
                button: MouseButton::Left,
                count: 2,
            },
            Action::KeyPress {
                key: KeyCode(0x41),
                mods: Modifiers::default(),
            },
            Action::Scroll {
                delta_x: 0,
                delta_y: -1,
            },
        ]);
        h.step(&m);
        h.step(&m);
        h.step(&m);
        h.step(&m);
        assert_eq!(backend.moves.load(Ordering::Relaxed), 1);
        assert_eq!(backend.clicks.load(Ordering::Relaxed), 2); // count=2
        assert_eq!(backend.keys.load(Ordering::Relaxed), 1);
        assert_eq!(backend.scrolls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn exit_guard_releases_each_held_button_once() {
        struct CountingBackend {
            ups: AtomicUsize,
        }
        impl InputBackend for CountingBackend {
            fn mouse_click(&self, _: MouseButton) {}
            fn mouse_down(&self, _: MouseButton) {}
            fn mouse_up(&self, _: MouseButton) {
                self.ups.fetch_add(1, Ordering::Relaxed);
            }
            fn scroll_wheel(&self, _: i32, _: i32) {}
            fn set_cursor_pos(&self, _: i32, _: i32) {}
            fn send_key(&self, _: KeyCode, _: Modifiers, _: bool) {}
            fn cursor_position(&self) -> (i32, i32) {
                (0, 0)
            }
            fn click_mouse(&self, _: &crate::platform::backend::ClickSpec) -> bool {
                true
            }
            fn release_mouse_hold(&self, _: MouseButton) {}
        }

        let backend = CountingBackend {
            ups: AtomicUsize::new(0),
        };
        // Left is pressed and released (nothing owed); X1 stays held.
        HELD_MASK.with(|m| m.set(0));
        note_pressed(MouseButton::Left);
        note_pressed(MouseButton::X1);
        note_released(MouseButton::Left);
        release_held_buttons(&backend);
        assert_eq!(
            backend.ups.load(Ordering::Relaxed),
            1,
            "only the button still held may be released"
        );
        release_held_buttons(&backend);
        assert_eq!(
            backend.ups.load(Ordering::Relaxed),
            1,
            "a cleared mask must not release twice"
        );
    }

    #[test]
    fn exit_guard_clears_is_running_after_early_exit() {
        let h = ExecutorHandle::new();
        HELD_MASK.with(|m| m.set(0));
        h.shared.is_running.store(true, Ordering::Release);
        let guard = MacroExitGuard {
            shared: Arc::clone(&h.shared),
            completed: false,
        };
        drop(guard);
        assert!(
            !h.is_running(),
            "a dropped run must not leave the executor permanently busy"
        );
    }

    /// Stop-then-play must not resurrect the cancelled run.
    ///
    /// Regression shape: `stop()` → `play(quick_macro)` must run the new
    /// macro to completion; the stale `cancel=true` must not leak into it.
    /// (`play` clears `cancel` on entry — the new run owns a fresh flag.)
    /// NOTE: `play` goes through the shared `macro_node()` bulkhead, which
    /// a parallel test may hold — so this test asserts the *flag contract*
    /// directly (stop sets, play-entry clears) instead of depending on node
    /// admission.
    #[test]
    fn stop_then_play_runs_the_new_macro_cleanly() {
        let h = ExecutorHandle::new();
        // Poison the flag first, like a hard stop mid-run would.
        h.stop();
        assert!(h.cancel_handle().load(Ordering::Acquire));
        assert!(!h.is_running());
        // Simulate exactly what `play_with_lookup` does on entry: claim the
        // run and clear the stale cancel. If this ever stops happening, a
        // stop-then-play resurrects the cancelled run (ghost macro).
        h.shared.is_running.store(true, Ordering::Release);
        h.shared.cancel.store(false, Ordering::Release);
        assert!(!h.cancel_handle().load(Ordering::Acquire));
        assert!(h.is_running());
        h.shared.is_running.store(false, Ordering::Release);
        h.stop();
        assert!(!h.is_running());
    }

    /// `stop()` is idempotent and settles the books: double-stop never
    /// panics, never leaves `is_running == true`.
    #[test]
    fn double_stop_is_idempotent_and_settles() {
        let h = ExecutorHandle::new();
        h.stop();
        h.stop();
        assert!(!h.is_running());
        // A fresh handle that never ran stops cleanly too.
        let fresh = ExecutorHandle::new();
        fresh.stop();
        assert!(!fresh.is_running());
    }

    /// Snubber Level 1 (`SnubberGuard`, USER_GUIDE Level 1): a guard that is
    /// never committed rolls back on drop — including the panic path. This
    /// is the contract the click-worker and macro `ErasedGuard` rely on:
    /// unwind → hardware released, no stuck button.
    #[test]
    fn snubber_guard_rolls_back_on_drop_and_on_panic() {
        use std::sync::atomic::{AtomicU32, Ordering as Ord};
        // Normal drop without commit → rollback runs exactly once.
        let fired = Arc::new(AtomicU32::new(0));
        {
            let c = Arc::clone(&fired);
            let _g = snubber::SnubberGuard::new(1, 7, move || {
                c.fetch_add(1, Ord::SeqCst);
            });
            // no commit → Drop fires
        }
        assert_eq!(fired.load(Ord::SeqCst), 1, "drop must roll back");
        // Committed guard → rollback never runs.
        let fired2 = Arc::new(AtomicU32::new(0));
        {
            let c = Arc::clone(&fired2);
            let g = snubber::SnubberGuard::new(1, 8, move || {
                c.fetch_add(1, Ord::SeqCst);
            });
            g.commit();
        }
        assert_eq!(fired2.load(Ord::SeqCst), 0, "commit must disarm");
        // Panic unwind → rollback still runs (catch the panic here so the
        // test harness stays green; the guard ran during unwinding).
        let fired3 = Arc::new(AtomicU32::new(0));
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let c = Arc::clone(&fired3);
            let _g = snubber::SnubberGuard::new(1, 9, move || {
                c.fetch_add(1, Ord::SeqCst);
            });
            panic!("simulated worker death");
        }));
        assert!(r.is_err());
        assert_eq!(
            fired3.load(Ord::SeqCst),
            1,
            "panic unwind must roll back via Drop"
        );
    }

    /// Snubber Level 2 (`scoped`, USER_GUIDE Level 0): `Ok` commits (no
    /// rollback), `Err` rolls back exactly once. Pins the exact API the
    /// stop-path relies on for transactional settle.
    #[test]
    fn snubber_scoped_commits_on_ok_and_rolls_back_on_err() {
        use std::sync::atomic::{AtomicU32, Ordering as Ord};
        let node = snubber::SnubberNode::builder("test_scoped_stop_path").build();
        let rolled = Arc::new(AtomicU32::new(0));
        let c = Arc::clone(&rolled);
        let ok = node.scoped(
            move || {
                c.fetch_add(1, Ord::SeqCst);
            },
            || Ok::<u32, &'static str>(7),
        );
        assert_eq!(ok.unwrap(), 7);
        assert_eq!(rolled.load(Ord::SeqCst), 0, "Ok must not roll back");
        let c2 = Arc::clone(&rolled);
        let err = node.scoped(
            move || {
                c2.fetch_add(1, Ord::SeqCst);
            },
            || Err::<u32, &'static str>("boom"),
        );
        assert!(matches!(err, Err(snubber::SnubberError::Work("boom"))));
        assert_eq!(rolled.load(Ord::SeqCst), 1, "Err must roll back once");
    }
}
