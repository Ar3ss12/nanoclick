//! # Raw Event Stream — what the platform layer captures.
//!
//! These are the lowest-level events from Windows hooks (KEYBOARD_LL,
//! MOUSE_LL) and cursor polling. They are *not* what gets saved in a
//! Macro — the Normalizer transforms them into clean `Action`s.

use crate::core::action::{KeyCode, Modifiers, MouseButton};
use crate::core::Action;

/// One raw captured event. `t_ms` is milliseconds since recording start.
#[derive(Debug, Clone, PartialEq)]
pub enum RawEvent {
    MouseMove {
        x: i32,
        y: i32,
        t_ms: u64,
    },
    MouseDown {
        button: MouseButton,
        t_ms: u64,
    },
    MouseUp {
        button: MouseButton,
        t_ms: u64,
    },
    KeyDown {
        key: KeyCode,
        mods: Modifiers,
        t_ms: u64,
    },
    KeyUp {
        key: KeyCode,
        mods: Modifiers,
        t_ms: u64,
    },
    Scroll {
        delta_x: i32,
        delta_y: i32,
        t_ms: u64,
    },
    /// Synthetic gap marker — emitted by Phase 2 (key coalescing) and Phase
    /// 4 (mouse hold detection) to preserve the timing between events.
    /// Phase 5 turns this into an `Action::Wait`.
    WaitGap {
        ms: u64,
        t_ms: u64,
    },
}

impl RawEvent {
    /// Timestamp of this event in ms since recording start.
    pub fn t(&self) -> u64 {
        match self {
            RawEvent::MouseMove { t_ms, .. }
            | RawEvent::MouseDown { t_ms, .. }
            | RawEvent::MouseUp { t_ms, .. }
            | RawEvent::KeyDown { t_ms, .. }
            | RawEvent::KeyUp { t_ms, .. }
            | RawEvent::Scroll { t_ms, .. }
            | RawEvent::WaitGap { t_ms, .. } => *t_ms,
        }
    }

    /// Convert raw event → Action (used only by Precise mode).
    pub fn to_action(&self) -> Action {
        match self {
            RawEvent::MouseMove { x, y, .. } => Action::MouseMove { x: *x, y: *y },
            RawEvent::MouseDown { button, .. } => Action::MouseDown { button: *button },
            RawEvent::MouseUp { button, .. } => Action::MouseUp { button: *button },
            RawEvent::KeyDown { key, mods, .. } => Action::KeyDown {
                key: *key,
                mods: *mods,
            },
            RawEvent::KeyUp { key, mods, .. } => Action::KeyUp {
                key: *key,
                mods: *mods,
            },
            RawEvent::Scroll {
                delta_x, delta_y, ..
            } => Action::Scroll {
                delta_x: *delta_x,
                delta_y: *delta_y,
            },
            RawEvent::WaitGap { ms, .. } => Action::Wait { ms: *ms },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_event_to_action_each_variant() {
        let t = 100;
        let events = vec![
            RawEvent::MouseMove {
                x: 1,
                y: 2,
                t_ms: t,
            },
            RawEvent::MouseDown {
                button: MouseButton::Left,
                t_ms: t,
            },
            RawEvent::MouseUp {
                button: MouseButton::Left,
                t_ms: t,
            },
            RawEvent::KeyDown {
                key: KeyCode::E,
                mods: Modifiers::NONE,
                t_ms: t,
            },
            RawEvent::KeyUp {
                key: KeyCode::E,
                mods: Modifiers::NONE,
                t_ms: t,
            },
            RawEvent::Scroll {
                delta_x: 0,
                delta_y: -120,
                t_ms: t,
            },
        ];
        for e in &events {
            let _a = e.to_action(); // just verify it compiles + doesn't panic
        }
    }
}
