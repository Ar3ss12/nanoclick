//! # Macro Sequence — a named, ordered list of [`Action`]s.
//!
//! Reference: `docs/MACRO_ARCHITECTURE.md` §2 (struct definition).

use super::action::Action;
use serde::{Deserialize, Serialize};

/// How a macro should repeat when played.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum RepeatMode {
    /// Run the actions exactly once.
    Once,
    /// Run the actions `count` times total.
    Times { count: u32 },
    /// Run forever until cancelled by the user (e.g. via Esc).
    UntilStopped,
}

impl Default for RepeatMode {
    fn default() -> Self {
        RepeatMode::Once
    }
}

#[allow(dead_code)]
impl RepeatMode {
    /// Human-readable summary, e.g. `12 actions · 4.2 sec`.
    pub fn summary(&self) -> String {
        match self {
            RepeatMode::Once => "once".to_string(),
            RepeatMode::Times { count } => format!("× {}", count),
            RepeatMode::UntilStopped => "until stopped".to_string(),
        }
    }
}

/// A complete macro: metadata + ordered action list + repeat policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Macro {
    pub id: String,
    pub name: String,
    /// Short emoji used as a card icon.
    #[serde(default)]
    pub icon: String,
    /// Ordered list of actions executed by the player.
    pub actions: Vec<Action>,
    #[serde(default)]
    pub repeat: RepeatMode,
    /// Per-action enabled/disabled state (parallel to `actions`).
    /// When `None`, every action is considered enabled.
    /// When `Some(vec)`, use the parallel bool — `false` means "skip during play".
    #[serde(default)]
    pub enabled: Option<Vec<bool>>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[allow(dead_code)]
impl Macro {
    /// Total number of actions in the macro (including disabled ones).
    pub fn len(&self) -> usize {
        self.actions.len()
    }

    /// Whether the macro has no actions yet.
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    /// Whether the action at `idx` should be executed.
    pub fn is_enabled(&self, idx: usize) -> bool {
        match &self.enabled {
            Some(vec) => vec.get(idx).copied().unwrap_or(true),
            None => true,
        }
    }

    /// Approximate total runtime in milliseconds, summed across Wait actions.
    /// Doesn't include mouse/scroll execution time (negligible relative to waits).
    pub fn approx_duration_ms(&self) -> u64 {
        let mut total = 0u64;
        for a in &self.actions {
            if let Action::Wait { ms } = a {
                total = total.saturating_add(*ms);
            }
        }
        total
    }

    /// Set the parallel `enabled` vector to all-true (used by Optimize).
    pub fn ensure_enabled_all(&mut self) {
        if self.enabled.is_none() {
            self.enabled = Some(vec![true; self.actions.len()]);
        }
    }

    /// Replace `enabled` with a vector of the given length, filling with `true`.
    pub fn sync_enabled_length(&mut self) {
        if let Some(ref mut v) = self.enabled {
            if v.len() != self.actions.len() {
                let n = self.actions.len();
                v.resize(n, true);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::action::{KeyCode, Modifiers};
    use super::*;

    fn sample_macro() -> Macro {
        Macro {
            id: "m1".into(),
            name: "Test".into(),
            icon: "🎬".into(),
            actions: vec![
                Action::MouseMove { x: 100, y: 200 },
                Action::Wait { ms: 300 },
                Action::KeyPress {
                    key: KeyCode::E,
                    mods: Modifiers::NONE,
                },
            ],
            repeat: RepeatMode::Once,
            enabled: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn len_and_is_empty() {
        let m = sample_macro();
        assert_eq!(m.len(), 3);
        assert!(!m.is_empty());
    }

    #[test]
    fn approx_duration_sums_only_waits() {
        let m = sample_macro();
        assert_eq!(m.approx_duration_ms(), 300);
    }

    #[test]
    fn is_enabled_defaults_true_when_none() {
        let m = sample_macro();
        assert!(m.is_enabled(0));
        assert!(m.is_enabled(2));
    }

    #[test]
    fn macro_roundtrip_json() {
        let m = sample_macro();
        let json = serde_json::to_string(&m).unwrap();
        let back: Macro = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }
}
