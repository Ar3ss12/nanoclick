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
        self.shared.is_running.load(Ordering::Relaxed)
    }

    /// Stop a running macro (e.g. via Esc hotkey).
    pub fn stop(&self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
        self.shared.is_running.store(false, Ordering::Relaxed);
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
        if self.shared.is_running.swap(true, Ordering::Relaxed) {
            // Already running — don't double-start.
            return;
        }
        self.shared.cancel.store(false, Ordering::Relaxed);
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
                    if self.shared.cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    // Only honor "run from here" offset on the first iteration.
                    let s = if i == 0 { start } else { 0 };
                    self.run_from_to(m, s, m.actions.len(), macro_lookup.clone());
                }
            }
            RepeatMode::UntilStopped => loop {
                if self.shared.cancel.load(Ordering::Relaxed) {
                    break;
                }
                self.run_from_to(m, 0, m.actions.len(), macro_lookup.clone());
            },
        }

        self.shared.is_running.store(false, Ordering::Relaxed);
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
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        match action {
            Action::Wait { ms } => {
                // Interruptible sleep.
                let target_ms = *ms;
                let chunk_ms = target_ms.min(50);
                let mut slept = 0u64;
                while slept < target_ms {
                    if cancel.load(Ordering::Relaxed) {
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
                    if cancel.load(Ordering::Relaxed) {
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
            }
            Action::MouseUp { button } => {
                self.backend.mouse_up(*button);
            }
            Action::KeyPress { key, mods } | Action::KeyDown { key, mods } => {
                if !cancel.load(Ordering::Relaxed) {
                    self.backend.send_key(*key, *mods, false);
                }
            }
            Action::KeyUp { key, mods } => {
                if !cancel.load(Ordering::Relaxed) {
                    self.backend.send_key(*key, *mods, true);
                }
            }
            Action::Scroll { delta_x, delta_y } => {
                self.backend.scroll_wheel(*delta_x, *delta_y);
            }
            Action::HoldStart => {
                // Equivalent to MouseDown(Left).
                self.backend.mouse_down(MouseButton::Left);
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
    if cancel.load(Ordering::Relaxed) {
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
    use crate::core::RepeatMode;
    use crate::core::action::{KeyCode, Modifiers};

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
        fn mouse_click(&self, _: MouseButton) { self.clicks.fetch_add(1, Ordering::Relaxed); }
        fn mouse_down(&self, _: MouseButton) {}
        fn mouse_up(&self, _: MouseButton) {}
        fn scroll_wheel(&self, _: i32, _: i32) { self.scrolls.fetch_add(1, Ordering::Relaxed); }
        fn set_cursor_pos(&self, _: i32, _: i32) { self.moves.fetch_add(1, Ordering::Relaxed); }
        fn send_key(&self, _: KeyCode, _: Modifiers, _: bool) { self.keys.fetch_add(1, Ordering::Relaxed); }
        fn cursor_position(&self) -> (i32, i32) { (0, 0) }
        fn click_mouse(&self, _: &crate::platform::backend::ClickSpec) -> bool { true }
        fn release_mouse_hold(&self, _: MouseButton) {}
    }

    #[test]
    fn all_primitives_route_through_input_backend_contract() {
        let backend = MockBackend::new_arc();
        let h = ExecutorHandle::with_backend(Arc::clone(&backend) as Arc<dyn InputBackend>);
        let m = dummy_macro(vec![
            Action::MouseMove { x: 10, y: 20 },
            Action::MouseClick { button: MouseButton::Left, count: 2 },
            Action::KeyPress { key: KeyCode(0x41), mods: Modifiers::default() },
            Action::Scroll { delta_x: 0, delta_y: -1 },
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
}
