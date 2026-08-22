//! # v4.0 — Execution context for advanced automation.
//!
//! Holds the live state of a macro execution:
//! - **variables**: a Map<String, i64> for SetVar/GetVar
//! - **last_value register**: SetVar/GetVar write here
//!
//! `run_actions_in` recursively drives `Repeat`/`If`/`Call`/`SetVar`/`GetVar`
//! while delegating primitive actions (`MouseMove`, `KeyPress`, etc.) to
//! `executor::dispatch_primitive`.

use crate::core::{Action, Condition, Macro};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Mutable runtime state for a single macro execution.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutionContext {
    /// Named variables — `SetVar` writes here, `GetVar` reads from here.
    /// Missing variable reads as `0`.
    pub variables: HashMap<String, i64>,
    /// Last numeric value seen (for debugging or chained computation).
    pub last_value: i64,
}

#[allow(dead_code)]
impl ExecutionContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// Snapshot of all non-zero variables — useful for the UI inspector.
    pub fn variables_snapshot(&self) -> Vec<(String, i64)> {
        let mut v: Vec<(String, i64)> = self
            .variables
            .iter()
            .filter(|(_, n)| **n != 0)
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }

    /// Read a variable, returning 0 if absent. Updates `last_value`.
    pub fn read_var(&mut self, name: &str) -> i64 {
        let v = self.variables.get(name).copied().unwrap_or(0);
        self.last_value = v;
        v
    }

    /// Write a variable.
    pub fn write_var(&mut self, name: &str, value: i64) {
        self.variables.insert(name.to_string(), value);
        self.last_value = value;
    }
}

/// Evaluates a `Condition` in the given context.
pub fn evaluate_condition(cond: &Condition, ctx: &mut ExecutionContext) -> bool {
    match cond {
        Condition::True => true,
        Condition::VarEq { name, value } => ctx.read_var(name) == *value,
        Condition::VarLt { name, value } => ctx.read_var(name) < *value,
        Condition::VarGt { name, value } => ctx.read_var(name) > *value,
        Condition::PixelEquals { .. } => {
            // Not implemented in v4.0 — signal "always-false" so users must
            // upgrade to a custom condition plugin later.
            false
        }
    }
}

/// Maximum recursion depth for `Call { macro_id }` to prevent infinite loops.
pub const MAX_CALL_DEPTH: usize = 8;

/// Macro lookup function used by `Action::Call`. Wrapped in `Arc` so the
/// runner can clone the same closure for recursive `Call` actions.
pub type MacroLookup = Arc<dyn Fn(&str) -> Option<Macro> + Send + Sync>;

/// Run a sequence of actions with control-flow handling. Stops early when
/// `cancel` becomes true.
///
/// `macro_lookup` is invoked when a `Call` action needs to resolve a target
/// macro. The lookup should return `None` if the macro id is unknown —
/// `Call` then becomes a no-op (so playback continues).
///
/// `depth` is the call stack depth — incremented on every `Call`.
pub fn run_actions_in(
    actions: &[Action],
    ctx: &mut ExecutionContext,
    cancel: Arc<AtomicBool>,
    macro_lookup: MacroLookup,
    depth: usize,
) {
    for a in actions {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        // Delegate to executor for primitive actions so the platform layer
        // does the real work (SendInput, SendKey, etc.).
        if let Some(primitive) = to_primitive(a) {
            super::executor::dispatch_primitive_with_cancel(&primitive, &cancel);
            continue;
        }
        match a {
            Action::Repeat { count, inner } => {
                let n = *count;
                for _ in 0..n {
                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let lookup = macro_lookup_clone(&macro_lookup);
                    run_actions_in(inner, ctx, cancel.clone(), lookup, depth);
                }
            }
            Action::If {
                condition,
                then_branch,
                else_branch,
            } => {
                if evaluate_condition(condition, ctx) {
                    let lookup = macro_lookup_clone(&macro_lookup);
                    run_actions_in(then_branch, ctx, cancel.clone(), lookup, depth);
                } else {
                    let lookup = macro_lookup_clone(&macro_lookup);
                    run_actions_in(else_branch, ctx, cancel.clone(), lookup, depth);
                }
            }
            Action::Call { macro_id } => {
                if depth >= MAX_CALL_DEPTH {
                    eprintln!(
                        "Call depth exceeded ({}/{}) — skipping {}",
                        depth, MAX_CALL_DEPTH, macro_id
                    );
                    continue;
                }
                if let Some(target) = macro_lookup(macro_id) {
                    let lookup = macro_lookup_clone(&macro_lookup);
                    run_actions_in(&target.actions, ctx, cancel.clone(), lookup, depth + 1);
                } else {
                    eprintln!("Call: macro '{}' not found — skipping", macro_id);
                }
            }
            Action::SetVar { name, value } => {
                ctx.write_var(name, *value);
            }
            Action::GetVar { name } => {
                let _ = ctx.read_var(name);
            }
            _ => {}
        }
    }
}

/// Helper: hand out a cloneable reference to the same lookup closure.
fn macro_lookup_clone(lookup: &MacroLookup) -> MacroLookup {
    Arc::clone(lookup)
}

/// Returns Some(primitive clone) for primitive actions, None for control-flow.
fn to_primitive(a: &Action) -> Option<Action> {
    match a {
        Action::MouseMove { .. }
        | Action::MouseClick { .. }
        | Action::MouseDown { .. }
        | Action::MouseUp { .. }
        | Action::KeyPress { .. }
        | Action::KeyDown { .. }
        | Action::KeyUp { .. }
        | Action::Scroll { .. }
        | Action::Wait { .. }
        | Action::HoldStart => Some(a.clone()),
        Action::Repeat { .. }
        | Action::If { .. }
        | Action::Call { .. }
        | Action::SetVar { .. }
        | Action::GetVar { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::action::MouseButton;

    fn cancel_false() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }
    fn no_macros(_: &str) -> Option<Macro> {
        None
    }

    #[test]
    fn write_then_read_var() {
        let mut ctx = ExecutionContext::new();
        ctx.write_var("attempts", 7);
        assert_eq!(ctx.read_var("attempts"), 7);
    }

    #[test]
    fn read_missing_var_is_zero() {
        let mut ctx = ExecutionContext::new();
        assert_eq!(ctx.read_var("missing"), 0);
    }

    #[test]
    fn condition_var_eq() {
        let mut ctx = ExecutionContext::new();
        ctx.write_var("count", 5);
        let c = Condition::VarEq {
            name: "count".into(),
            value: 5,
        };
        assert!(evaluate_condition(&c, &mut ctx));
    }

    #[test]
    fn condition_var_lt() {
        let mut ctx = ExecutionContext::new();
        ctx.write_var("count", 4);
        let c = Condition::VarLt {
            name: "count".into(),
            value: 5,
        };
        assert!(evaluate_condition(&c, &mut ctx));
    }

    #[test]
    fn condition_var_gt() {
        let mut ctx = ExecutionContext::new();
        ctx.write_var("count", 6);
        let c = Condition::VarGt {
            name: "count".into(),
            value: 5,
        };
        assert!(evaluate_condition(&c, &mut ctx));
    }

    #[test]
    fn condition_true_always_passes() {
        let mut ctx = ExecutionContext::new();
        assert!(evaluate_condition(&Condition::True, &mut ctx));
    }

    #[test]
    fn condition_pixel_equals_unsupported_yields_false() {
        let mut ctx = ExecutionContext::new();
        let c = Condition::PixelEquals {
            x: 0,
            y: 0,
            rgb: 0x000000,
        };
        assert!(!evaluate_condition(&c, &mut ctx));
    }

    #[test]
    fn repeat_runs_inner_loop_n_times() {
        let mut ctx = ExecutionContext::new();
        let actions = vec![Action::Repeat {
            count: 3,
            inner: vec![Action::SetVar {
                name: "iter".into(),
                value: 1,
            }],
        }];
        run_actions_in(
            &actions,
            &mut ctx,
            cancel_false(),
            Arc::new(no_macros) as MacroLookup,
            0,
        );
        assert_eq!(ctx.read_var("iter"), 1);
    }

    #[test]
    fn if_then_branch_runs_when_true() {
        let mut ctx = ExecutionContext::new();
        ctx.write_var("flag", 1);
        let actions = vec![Action::If {
            condition: Condition::VarEq {
                name: "flag".into(),
                value: 1,
            },
            then_branch: vec![Action::SetVar {
                name: "took_then".into(),
                value: 1,
            }],
            else_branch: vec![Action::SetVar {
                name: "took_else".into(),
                value: 1,
            }],
        }];
        run_actions_in(
            &actions,
            &mut ctx,
            cancel_false(),
            Arc::new(no_macros) as MacroLookup,
            0,
        );
        assert_eq!(ctx.read_var("took_then"), 1);
        assert_eq!(ctx.read_var("took_else"), 0);
    }

    #[test]
    fn if_else_branch_runs_when_false() {
        let mut ctx = ExecutionContext::new();
        ctx.write_var("flag", 0);
        let actions = vec![Action::If {
            condition: Condition::VarEq {
                name: "flag".into(),
                value: 1,
            },
            then_branch: vec![Action::SetVar {
                name: "took_then".into(),
                value: 1,
            }],
            else_branch: vec![Action::SetVar {
                name: "took_else".into(),
                value: 1,
            }],
        }];
        run_actions_in(
            &actions,
            &mut ctx,
            cancel_false(),
            Arc::new(no_macros) as MacroLookup,
            0,
        );
        assert_eq!(ctx.read_var("took_then"), 0);
        assert_eq!(ctx.read_var("took_else"), 1);
    }

    #[test]
    fn call_resolves_through_macro_lookup() {
        use crate::core::RepeatMode;
        let mut ctx = ExecutionContext::new();
        // Move `inner` into the closure so the closure owns its data.
        let lookup = |id: &str| -> Option<Macro> {
            if id == "inner" {
                Some(Macro {
                    id: "inner".into(),
                    name: "inner macro".into(),
                    icon: "✨".into(),
                    actions: vec![Action::SetVar {
                        name: "callee_ran".into(),
                        value: 1,
                    }],
                    repeat: RepeatMode::Once,
                    enabled: None,
                    created_at: 0,
                    updated_at: 0,
                })
            } else {
                None
            }
        };
        let actions = vec![Action::Call {
            macro_id: "inner".into(),
        }];
        run_actions_in(
            &actions,
            &mut ctx,
            cancel_false(),
            Arc::new(lookup) as MacroLookup,
            0,
        );
        assert_eq!(ctx.read_var("callee_ran"), 1);
    }

    #[test]
    fn call_unknown_macro_is_no_op() {
        let mut ctx = ExecutionContext::new();
        let actions = vec![Action::Call {
            macro_id: "nonexistent".into(),
        }];
        run_actions_in(
            &actions,
            &mut ctx,
            cancel_false(),
            Arc::new(no_macros) as MacroLookup,
            0,
        );
        assert_eq!(ctx.read_var("callee_ran"), 0);
    }

    #[test]
    fn mouse_click_action_in_run_actions_in() {
        let mut ctx = ExecutionContext::new();
        let actions = vec![Action::MouseClick {
            button: MouseButton::Left,
            count: 1,
        }];
        run_actions_in(
            &actions,
            &mut ctx,
            cancel_false(),
            Arc::new(no_macros) as MacroLookup,
            0,
        );
    }

    #[test]
    fn cancel_short_circuits_loop() {
        let mut ctx = ExecutionContext::new();
        let cancel = Arc::new(AtomicBool::new(false));
        let actions = vec![Action::Repeat {
            count: 100,
            inner: vec![Action::Wait { ms: 100 }],
        }];
        let c2 = cancel.clone();
        let t = std::thread::spawn(move || {
            run_actions_in(
                &actions,
                &mut ctx,
                c2,
                Arc::new(no_macros) as MacroLookup,
                0,
            );
        });
        std::thread::sleep(std::time::Duration::from_millis(50));
        cancel.store(true, Ordering::Relaxed);
        t.join().expect("cancel thread should complete fast");
    }
}
