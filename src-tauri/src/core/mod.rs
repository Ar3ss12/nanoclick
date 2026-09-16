//! # core — Pure-Rust business logic for NanoClick.
//!
//! No Tauri, no platform-specific code here. Everything is unit-testable
//! in isolation.
//!
//! Layout (proposed in `docs/MACRO_ARCHITECTURE.md` §11):
//! - `action.rs` — `Action` enum (single atomic step).
//! - `sequence.rs` — `Macro` struct + `RepeatMode` (named, ordered list).

pub mod action;
pub mod execution;
pub mod executor;
pub mod sequence;

pub use action::{Action, Condition};
pub use execution::{run_actions_in, ExecutionContext, MacroLookup};
pub use executor::{global, set_global, ExecutorHandle};
pub use sequence::{Macro, RepeatMode};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_full_action_macro_roundtrip() {
        use action::{KeyCode, Modifiers, MouseButton};

        let macro_seq = Macro {
            id: "smoke".into(),
            name: "Smoke".into(),
            icon: "🎬".into(),
            actions: vec![
                Action::MouseMove { x: 500, y: 300 },
                Action::MouseClick {
                    button: MouseButton::Left,
                    count: 1,
                },
                Action::Wait { ms: 1000 },
                Action::KeyPress {
                    key: KeyCode::E,
                    mods: Modifiers::NONE,
                },
            ],
            repeat: RepeatMode::Times { count: 3 },
            enabled: None,
            created_at: 1700000000,
            updated_at: 1700000000,
        };

        let json = serde_json::to_string(&macro_seq).unwrap();
        let back: Macro = serde_json::from_str(&json).unwrap();
        assert_eq!(macro_seq, back);
        assert_eq!(back.approx_duration_ms(), 1000);
    }
}
