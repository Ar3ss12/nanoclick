//! # Action Engine — single atomic step in a Macro.
//!
//! All actions are JSON-serializable via serde (tagged enum). Storage,
//! playback, and the Visual Editor all work on this single type.
//!
//! Reference: `docs/MACRO_ARCHITECTURE.md` §2.

use serde::{Deserialize, Serialize};

/// Mouse button selector. Matches the 5 buttons supported by Win32 SendInput.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

#[allow(dead_code)]
impl MouseButton {
    /// Short emoji label for the Visual Editor.
    pub fn label(&self) -> &'static str {
        match self {
            MouseButton::Left => "Left",
            MouseButton::Right => "Right",
            MouseButton::Middle => "Middle",
            MouseButton::X1 => "X1",
            MouseButton::X2 => "X2",
        }
    }
}

impl Default for MouseButton {
    fn default() -> Self {
        MouseButton::Left
    }
}

/// Keyboard modifier bitflags. Captured at recording time so the replay
/// is exact even if the user has remapped keys in OS settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

#[allow(dead_code)]
impl Modifiers {
    pub const NONE: Modifiers = Modifiers {
        ctrl: false,
        alt: false,
        shift: false,
        win: false,
    };

    pub fn any(&self) -> bool {
        self.ctrl || self.alt || self.shift || self.win
    }

    /// Human-readable combo, e.g. `Ctrl + Shift + E`.
    pub fn display(&self) -> String {
        let mut parts: Vec<&'static str> = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.win {
            parts.push("Win");
        }
        parts.join(" + ")
    }
}

/// A virtual key identifier. We store the Windows VK code as a u16 to
/// make serialization stable across keyboard layouts. Display-friendly
/// names are produced by `key_name()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyCode(pub u16);

#[allow(dead_code)]
impl KeyCode {
    /// Common helper: the `E` key (used in many of the examples).
    pub const E: KeyCode = KeyCode(0x45);
    /// The `Escape` key.
    pub const ESCAPE: KeyCode = KeyCode(0x1B);

    /// Best-effort human-readable name. Returns "VK_xx" if unknown.
    pub fn name(&self) -> String {
        match self.0 {
            0x08 => "Backspace".into(),
            0x09 => "Tab".into(),
            0x0D => "Enter".into(),
            0x10 => "Shift".into(),
            0x11 => "Ctrl".into(),
            0x12 => "Alt".into(),
            0x1B => "Escape".into(),
            0x20 => "Space".into(),
            0x25 => "←".into(),
            0x26 => "↑".into(),
            0x27 => "→".into(),
            0x28 => "↓".into(),
            0x30..=0x39 => {
                let c = (self.0 - 0x30) as u8 + b'0';
                (c as char).to_string()
            }
            0x41..=0x5A => {
                let c = (self.0 - 0x41) as u8 + b'A';
                (c as char).to_string()
            }
            0x60..=0x69 => format!("Num{}", self.0 - 0x60),
            0x70..=0x7B => format!("F{}", self.0 - 0x6F),
            other => format!("VK_{:02X}", other),
        }
    }
}

/// One atomic step. Tagged enum so JSON has a `"type"` discriminator.
///
/// v3.2 variants: input primitives + Wait.
/// v4.0 variants: control flow (`Repeat`, `If`, `Call`, `SetVar`, `GetVar`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Move the cursor to absolute screen coordinates.
    MouseMove { x: i32, y: i32 },

    /// Press and release a mouse button at the current cursor position.
    /// `count = 2` produces a double-click.
    MouseClick { button: MouseButton, count: u8 },

    /// Press and hold a mouse button (no release). Paired with a later `MouseUp`.
    MouseDown { button: MouseButton },

    /// Release a previously-held mouse button.
    MouseUp { button: MouseButton },

    /// Press and release a keyboard key.
    KeyPress { key: KeyCode, mods: Modifiers },

    /// Press and hold a key. Paired with `KeyUp`.
    KeyDown { key: KeyCode, mods: Modifiers },

    /// Release a previously-held key.
    KeyUp { key: KeyCode, mods: Modifiers },

    /// Scroll wheel delta. Positive `delta_y` = down (toward user).
    /// Recorded in WHEEL_DELTA units (120 = one notch).
    Scroll { delta_x: i32, delta_y: i32 },

    /// Pause for N milliseconds. Auto-generated between recorded events.
    Wait { ms: u64 },

    /// Begin holding the left mouse button indefinitely until release
    /// or end-of-macro. Used to model long "hold" actions in sequences.
    HoldStart,

    // ── v4.0: Advanced Automation ─────────────────────────────────────────
    /// Repeat a block of actions N times. Inline block — no recursion needed.
    Repeat { count: i32, inner: Vec<Action> },

    /// Conditional branch. If `condition` holds, execute the `then` branch;
    /// otherwise (optionally) execute `else_branch`.
    If {
        condition: Condition,
        then_branch: Vec<Action>,
        else_branch: Vec<Action>,
    },

    /// Call another macro by id. Supports composition and re-use.
    Call { macro_id: String },

    /// Set a named variable in the execution context. Used for state that
    /// survives across iterations (e.g. attempt counts, last seen values).
    SetVar { name: String, value: i64 },

    /// Read a named variable, store its value into the counter register
    /// (`last_value`). No side-effect output; useful as a probing step.
    GetVar { name: String },
}

#[allow(dead_code)]
impl Action {
    /// Short human-readable kind for table-style displays.
    pub fn kind_label(&self) -> &'static str {
        match self {
            Action::MouseMove { .. } => "MOVE",
            Action::MouseClick { count, .. } if *count == 2 => "DOUBLE CLICK",
            Action::MouseClick { count, .. } if *count == 1 => "CLICK",
            Action::MouseClick { .. } => "CLICK",
            Action::MouseDown { .. } => "MOUSE DOWN",
            Action::MouseUp { .. } => "MOUSE UP",
            Action::KeyPress { .. } => "KEY PRESS",
            Action::KeyDown { .. } => "KEY DOWN",
            Action::KeyUp { .. } => "KEY UP",
            Action::Scroll { .. } => "SCROLL",
            Action::Wait { .. } => "WAIT",
            Action::HoldStart => "HOLD START",
            Action::Repeat { .. } => "REPEAT",
            Action::If { .. } => "IF",
            Action::Call { .. } => "CALL",
            Action::SetVar { .. } => "SET",
            Action::GetVar { .. } => "GET",
        }
    }

    /// Emoji glyph for the Visual Editor block.
    pub fn icon(&self) -> &'static str {
        match self {
            Action::MouseMove { .. } => "🖱",
            Action::MouseClick { .. } => "🖱",
            Action::MouseDown { .. } => "🖱",
            Action::MouseUp { .. } => "🖱",
            Action::KeyPress { .. } | Action::KeyDown { .. } | Action::KeyUp { .. } => "⌨️",
            Action::Scroll { .. } => "🖱",
            Action::Wait { .. } => "⏱️",
            Action::HoldStart => "🖱",
            Action::Repeat { .. } => "🔁",
            Action::If { .. } => "🔀",
            Action::Call { .. } => "📞",
            Action::SetVar { .. } => "📝",
            Action::GetVar { .. } => "📖",
        }
    }

    /// Short detail summary for table views.
    pub fn detail(&self) -> String {
        match self {
            Action::MouseMove { x, y } => format!("({}, {})", x, y),
            Action::MouseClick { button, count } => {
                if *count > 1 {
                    format!("{} · x{}", button.label(), count)
                } else {
                    button.label().to_string()
                }
            }
            Action::MouseDown { button } => button.label().to_string(),
            Action::MouseUp { button } => button.label().to_string(),
            Action::KeyPress { key, mods } | Action::KeyDown { key, mods } => {
                let base = key.name();
                if mods.any() {
                    format!("{} + {}", mods.display(), base)
                } else {
                    base
                }
            }
            Action::KeyUp { key, .. } => key.name(),
            Action::Scroll { delta_y, .. } => {
                let dir = if *delta_y > 0 { "↓" } else { "↑" };
                let notches = delta_y.abs() / 120;
                format!(
                    "{} {} step{}",
                    dir,
                    notches.max(1),
                    if notches != 1 { "s" } else { "" }
                )
            }
            Action::Wait { ms } => format!("{} ms", ms),
            Action::HoldStart => "Left · indefinite".to_string(),
            Action::Repeat { count, inner } => format!("×{} ({} actions)", count, inner.len()),
            Action::If { then_branch, .. } => format!("if … then ({} actions)", then_branch.len()),
            Action::Call { macro_id } => format!("→ {}", macro_id),
            Action::SetVar { name, value } => format!("{} = {}", name, value),
            Action::GetVar { name } => format!("read {}", name),
        }
    }
}

/// Boolean condition for an `Action::If` branch.
///
/// Discriminated by `kind` so JSON looks like:
/// ```json
/// { "kind": "var_eq", "name": "attempts", "value": 5 }
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Condition {
    /// Variable `name` == `value`.
    VarEq { name: String, value: i64 },
    /// Variable `name` < `value`.
    VarLt { name: String, value: i64 },
    /// Variable `name` > `value`.
    VarGt { name: String, value: i64 },
    /// Pixel at (x, y) equals the expected RGB color.
    PixelEquals { x: i32, y: i32, rgb: u32 },
    /// Always-true literal. Useful for code-generated defaults.
    True,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_mouse_click_serializes_with_snake_case_tag() {
        let a = Action::MouseClick {
            button: MouseButton::Left,
            count: 1,
        };
        let json = serde_json::to_string(&a).unwrap();
        assert!(json.contains("\"type\":\"mouse_click\""));
        assert!(json.contains("\"button\":\"left\""));
        assert!(json.contains("\"count\":1"));
    }

    #[test]
    fn action_wait_roundtrip() {
        let a = Action::Wait { ms: 742 };
        let json = serde_json::to_string(&a).unwrap();
        let back: Action = serde_json::from_str(&json).unwrap();
        assert_eq!(a, back);
    }

    #[test]
    fn modifiers_display() {
        let m = Modifiers {
            ctrl: true,
            shift: true,
            alt: false,
            win: false,
        };
        assert_eq!(m.display(), "Ctrl + Shift");
    }

    #[test]
    fn key_name_for_letters_and_function_keys() {
        assert_eq!(KeyCode(0x41).name(), "A");
        assert_eq!(KeyCode(0x35).name(), "5");
        assert_eq!(KeyCode(0x74).name(), "F5");
    }
}
