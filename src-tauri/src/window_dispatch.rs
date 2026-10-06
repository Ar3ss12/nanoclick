//! In-window hotkey dispatcher — the decision half of the page's keydown listener.
//!
//! When the NanoClick window has focus, the global `WH_KEYBOARD_LL` hook can be
//! affected by Chromium message-loop isolation, so the page forwards key
//! presses here for a zero-latency decision. The page keeps ONLY what must live
//! in the DOM: `preventDefault()` on browser accelerators, the `isInput` /
//! `activeRecordingBtn` guards, and painting the outcome. Matching itself —
//! trigger + required-modifier logic over the live bindings — lives here, next
//! to the `HotkeyCombo` parser it mirrors, and is unit-tested without WebView.
//!
//! Label-based on purpose: the page speaks physical key labels (`R`, `Ctrl`,
//! `Mouse4`), the same vocabulary as `codeToPhysicalKey` in `main.js` and the
//! persisted binding strings. No VK mapping, no new snapshot type — the live
//! bindings come from `ClickScheduler::hotkey_snapshot_data()`.

/// Outcome of one in-window key press.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowAction {
    /// No binding matched — the page does nothing.
    NoMatch,
    /// Emergency stop, but only meaningful while running (page gates on it).
    EmergencyStop,
    /// Mode switch (`mode_switch` binding matched).
    ModeSwitch,
    /// Toggle, with the Work-Mode veto resolved here (not in the page).
    /// `vetoed == true` means "Work Mode blocks the start" — the page toasts
    /// `tray_blocked_work_mode` instead of toggling.
    Toggle { vetoed: bool },
    /// Preset slot `0..9` matched — the page applies `presetLibrary()[slot]`.
    PresetSlot(usize),
}

/// Normalize one physical key label for comparison (mirrors the page's
/// `codeToPhysicalKey` output vocabulary, uppercased).
fn norm(label: &str) -> String {
    label.trim().to_uppercase()
}

/// `required` token matches one held key, with the Ctrl/Alt/Shift family
/// aliases the page's `_matchesInWindowHotkey` understands.
fn required_matches(req: &str, held_upper: &[String]) -> bool {
    let req = req.to_uppercase();
    for h in held_upper {
        if h == &req {
            return true;
        }
        if req == "CTRL" && (h == "CONTROLLEFT" || h == "CONTROLRIGHT" || h == "CTRL") {
            return true;
        }
        if req == "ALT" && (h == "ALTLEFT" || h == "ALTRIGHT" || h == "ALT") {
            return true;
        }
        if req == "SHIFT" && (h == "SHIFTLEFT" || h == "SHIFTRIGHT" || h == "SHIFT") {
            return true;
        }
    }
    false
}

/// Mirrors `_matchesInWindowHotkey(binding, current, held)`: `/`- or
/// `|`-separated groups, `+`-separated `required...+trigger`.
pub fn binding_matches(binding: &str, current_key: &str, held_keys: &[String]) -> bool {
    if binding.trim().is_empty() {
        return false;
    }
    let current = norm(current_key);
    let held_upper: Vec<String> = held_keys.iter().map(|k| norm(k)).collect();
    for group in binding.split(|c| c == '/' || c == '|') {
        let parts: Vec<&str> = group.split('+').map(str::trim).filter(|s| !s.is_empty()).collect();
        if parts.is_empty() {
            continue;
        }
        let trigger = norm(parts[parts.len() - 1]);
        if trigger != current {
            continue;
        }
        let required = &parts[..parts.len() - 1];
        if required.iter().all(|r| required_matches(r, &held_upper)) {
            return true;
        }
    }
    false
}

/// Decide one in-window key press against live bindings.
/// Priority mirrors the page: emergency → mode → toggle → preset slots.
/// `mode_is_work` + `running` resolve the Work-Mode veto here so the page
/// never branches on clicker state.
pub fn decide_window_key(
    toggle: &str,
    mode_switch: &str,
    emergency_stop: &str,
    preset_slots: &[String],
    current_key: &str,
    held_keys: &[String],
    mode_is_work: bool,
    running: bool,
) -> WindowAction {
    if binding_matches(emergency_stop, current_key, held_keys) {
        return WindowAction::EmergencyStop;
    }
    if binding_matches(mode_switch, current_key, held_keys) {
        return WindowAction::ModeSwitch;
    }
    if binding_matches(toggle, current_key, held_keys) {
        // Work Mode veto: starting is blocked, stopping always goes through.
        let vetoed = mode_is_work && !running;
        return WindowAction::Toggle { vetoed };
    }
    for (slot, label) in preset_slots.iter().enumerate().take(9) {
        if !label.trim().is_empty() && binding_matches(label, current_key, held_keys) {
            return WindowAction::PresetSlot(slot);
        }
    }
    WindowAction::NoMatch
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(keys: &[&str]) -> Vec<String> {
        keys.iter().map(|k| k.to_string()).collect()
    }

    #[test]
    fn bare_letter_toggle_matches() {
        assert!(binding_matches("R", "R", &held(&[])));
        assert!(binding_matches("R", "r", &held(&[])));
        assert!(!binding_matches("R", "K", &held(&[])));
    }

    #[test]
    fn chord_needs_its_modifiers_held() {
        assert!(binding_matches("Ctrl+Alt+M", "M", &held(&["Ctrl", "Alt"])));
        assert!(!binding_matches("Ctrl+Alt+M", "M", &held(&["Ctrl"])));
        assert!(binding_matches("Ctrl+Alt+M", "M", &held(&["ControlLeft", "AltRight"])));
    }

    #[test]
    fn slash_groups_are_alternatives() {
        assert!(binding_matches("R/K", "K", &held(&[])));
        assert!(binding_matches("R|K", "R", &held(&[])));
        assert!(!binding_matches("R/K", "J", &held(&[])));
    }

    #[test]
    fn star_digit_chord_matches() {
        assert!(binding_matches("*+1", "1", &held(&["*"])));
        assert!(!binding_matches("*+1", "1", &held(&[])));
    }

    #[test]
    fn priority_is_emergency_then_mode_then_toggle() {
        // One press matching everything resolves to emergency stop.
        let a = decide_window_key("R", "R", "R", &[], "R", &held(&[]), false, true);
        assert_eq!(a, WindowAction::EmergencyStop);
        let b = decide_window_key("R", "R", "", &[], "R", &held(&[]), false, false);
        assert_eq!(b, WindowAction::ModeSwitch);
    }

    #[test]
    fn work_mode_vetoes_start_but_not_stop() {
        let start = decide_window_key("R", "", "", &[], "R", &held(&[]), true, false);
        assert_eq!(start, WindowAction::Toggle { vetoed: true });
        let stop = decide_window_key("R", "", "", &[], "R", &held(&[]), true, true);
        assert_eq!(stop, WindowAction::Toggle { vetoed: false });
    }

    #[test]
    fn preset_slot_reports_its_index() {
        let slots = vec!["".to_string(), "F2".to_string()];
        let a = decide_window_key("", "", "", &slots, "F2", &held(&[]), false, false);
        assert_eq!(a, WindowAction::PresetSlot(1));
        let miss = decide_window_key("", "", "", &slots, "F3", &held(&[]), false, false);
        assert_eq!(miss, WindowAction::NoMatch);
    }

    #[test]
    fn empty_bindings_never_match() {
        assert_eq!(
            decide_window_key("", "", "", &[], "R", &held(&[]), false, false),
            WindowAction::NoMatch
        );
    }
}
