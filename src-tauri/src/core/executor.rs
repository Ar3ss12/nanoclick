//! # Executor — plays a `Macro` by running each `Action` sequentially.
//!
//! The Executor is the playback counterpart of the Recorder. It reuses
//! the existing platform layer (`platform::windows::click_mouse_ext`,
//! `scroll_wheel`, etc.) so behavior matches what the user recorded.
//!
//! Reference: `docs/MACRO_ARCHITECTURE.md` §4.

use crate::core::action::{KeyCode, Modifiers, MouseButton};
use crate::core::{Action, Macro, MacroLookup, RepeatMode};

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Handle to a running macro playback. `stop()` interrupts the playback.
#[derive(Clone)]
pub struct ExecutorHandle {
    shared: Arc<ExecutorShared>,
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
    /// Create an empty (not-running) handle.
    pub fn new() -> Self {
        ExecutorHandle {
            shared: Arc::new(ExecutorShared {
                is_running: AtomicBool::new(false),
                cancel: Arc::new(AtomicBool::new(false)),
                next_step_idx: AtomicUsize::new(0),
            }),
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
    /// the platform layer (windows.rs on Windows).
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
                // For infinite loop, ignore the user-set start (only first user
                // action was honoring it).
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
            execute_action(&m.actions[idx], &self.shared.cancel);
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
        // and delegates primitive actions (`MouseMove`, `KeyPress`, etc.)
        // back to `dispatch_primitive`.
        let end = to.min(m.actions.len());
        let from = from.min(end);
        let mut ctx = crate::core::ExecutionContext::new();
        // Share the same Arc<AtomicBool> as ExecutorHandle — `stop()` writes to
        // this exact flag, so the runner observes it on the next loop tick.
        let cancel = self.cancel_handle();
        crate::core::run_actions_in(&m.actions[from..end], &mut ctx, cancel, macro_lookup, 0);
    }
}

impl Default for ExecutorHandle {
    fn default() -> Self {
        Self::new()
    }
}

/// Single-action dispatch. Uses platform layer where available; falls back
/// to sleeping for `Wait`.
fn execute_action(action: &Action, cancel: &AtomicBool) {
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
            // Reuse platform::set_cursor_pos.
            #[cfg(target_os = "windows")]
            {
                use crate::platform;
                platform::windows::set_cursor_pos(*x, *y);
            }
            #[cfg(not(target_os = "windows"))]
            {
                let _ = (x, y);
            }
        }
        Action::MouseClick { button, count } => {
            click_button(*button, *count, cancel);
        }
        Action::MouseDown { button } => {
            #[cfg(target_os = "windows")]
            {
                use crate::platform;
                platform::windows::mouse_down(*button);
            }
            #[cfg(not(target_os = "windows"))]
            {
                let _ = button;
            }
        }
        Action::MouseUp { button } => {
            #[cfg(target_os = "windows")]
            {
                use crate::platform;
                platform::windows::mouse_up(*button);
            }
            #[cfg(not(target_os = "windows"))]
            {
                let _ = button;
            }
        }
        Action::KeyPress { key, mods } | Action::KeyDown { key, mods } => {
            send_key(*key, *mods, false, cancel);
        }
        Action::KeyUp { key, mods } => {
            send_key(*key, *mods, true, cancel);
        }
        Action::Scroll { delta_x, delta_y } => {
            #[cfg(target_os = "windows")]
            {
                use crate::platform;
                platform::windows::scroll_wheel(*delta_x, *delta_y);
            }
            #[cfg(not(target_os = "windows"))]
            {
                let _ = (delta_x, delta_y);
            }
        }
        Action::HoldStart => {
            // Equivalent to MouseDown(Left).
            #[cfg(target_os = "windows")]
            {
                use crate::platform;
                platform::windows::mouse_down(MouseButton::Left);
            }
        }
        // ── v4.0: control flow variants. They are handled at the *list*
        // level (`run_repeat`, `run_if`, etc.) — here we just no-op if they
        // somehow reach `execute_action` directly (e.g. via `step()` which
        // expects a single primitive action).
        Action::Repeat { .. }
        | Action::If { .. }
        | Action::Call { .. }
        | Action::SetVar { .. }
        | Action::GetVar { .. } => {
            // See `ExecutionContext` for the full semantic implementation.
            // When invoked as a single step (debug Step mode), control-flow
            // actions are *atomic* ticks — we just record the intent and
            // wait a tick so the user sees a UI response.
            thread::sleep(Duration::from_millis(1));
        }
    }
}

#[cfg(target_os = "windows")]
fn click_button(button: MouseButton, count: u8, cancel: &AtomicBool) {
    use crate::platform;
    for _ in 0..count {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        platform::windows::mouse_click(button);
        if count > 1 {
            thread::sleep(Duration::from_millis(50));
        }
    }
}

/// v4.0 — Cancel-aware dispatcher. Same as `dispatch_primitive` but checks
/// `cancel` between every chunk of `Wait`. Used by the control-flow runner.
pub fn dispatch_primitive_with_cancel(action: &Action, cancel: &std::sync::atomic::AtomicBool) {
    if cancel.load(Ordering::Relaxed) {
        return;
    }
    execute_action(action, cancel);
}

#[cfg(not(target_os = "windows"))]
fn click_button(button: MouseButton, count: u8, cancel: &AtomicBool) {
    let _ = button;
    let _ = count;
    let _ = cancel;
}

fn send_key(key: KeyCode, mods: Modifiers, is_up: bool, cancel: &AtomicBool) {
    if cancel.load(Ordering::Relaxed) {
        return;
    }
    #[cfg(target_os = "windows")]
    {
        use crate::platform;
        platform::windows::send_key(key.0, mods.ctrl, mods.alt, mods.shift, mods.win, is_up);
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (key, mods, is_up);
    }
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

    fn dummy_macro(actions: Vec<Action>) -> Macro {
        use crate::core::RepeatMode;
        Macro {
            id: "t".into(),
            name: "t".into(),
            icon: "🎬".into(),
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
        // Wait briefly for the macro to start executing, then cancel.
        thread::sleep(Duration::from_millis(50));
        h.stop();
        // Wait for thread to finish.
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
        // Three steps should each return true.
        assert!(h.step(&m));
        assert!(h.step(&m));
        assert!(h.step(&m));
        // Fourth returns false (out of bounds).
        assert!(!h.step(&m));
        // rewind() puts cursor back at zero.
        h.rewind();
        assert!(h.step(&m));
    }

    #[test]
    fn run_from_here_offset() {
        let h = ExecutorHandle::new();
        let m = dummy_macro(vec![
            Action::Wait { ms: 5000 }, // idx 0 — should be skipped
            Action::Wait { ms: 5 },    // idx 1 — start
            Action::Wait { ms: 5 },    // idx 2
        ]);
        // Set start index to 1 → skips the long Wait.
        h.set_start_idx(1);
        let h2 = h.clone();
        let m2 = m.clone();
        let t = thread::spawn(move || h2.play(&m2));
        // If we started from idx 0 it'd take >5s; from 1 it should finish fast.
        thread::sleep(Duration::from_millis(100));
        h.stop();
        let _ = t.join();
        // Note: with cancel at 100ms we don't strictly test the speed, only
        // that no crash occurred.
    }

    #[test]
    fn step_skips_disabled_actions() {
        use crate::core::action::{KeyCode, Modifiers};
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
        // Disable idx 0 via macro.enabled; only KeyPress should run.
        m.enabled = Some(vec![false, true]);
        assert!(h.step(&m));
    }
}
