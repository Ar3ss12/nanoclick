//! Key classification policy — which keys count as "typing".
//!
//! # The bug this type exists to kill
//!
//! Bind the toggle hotkey to `J` and the clicker never starts. One keypress
//! produces two contradicting decisions, in this order:
//!
//! ```text
//! mod.rs  is_text_keypress_vk(0x4A /* J */)  ->  true   (J is not in the seed)
//! mod.rs  scheduler.set_active(false, …)                 <- clicker STOPPED
//! mod.rs  guard.try_arm_toggle(…)
//! typing  hotkeys_locked() == true            ->  veto  <- start also refused
//! ```
//!
//! The kill-switch runs **before** the toggle gate, so the same press both stops
//! the clicker and is refused for starting it. The net result is a hotkey that is
//! guaranteed never to start the clicker — on exactly the key the user pressed.
//!
//! # Why a hardcoded list cannot fix it
//!
//! Widening [`GAMEPLAY_LETTERS`] hides `J` and exposes `T`, `Y`, `U` or `O` on the
//! next build. The question is not "which keys are gameplay keys" — it is "**which
//! keys did this user bind to hotkeys**". That answer is dynamic, so the fix must be
//! dynamic too.
//!
//! # Two layers
//!
//! | Layer | Source | Owner | Removable? |
//! |---|---|---|---|
//! | A — binding shield | [`KeyPolicy::exempt_all`] | system, from `HotkeyBindings` | **never** |
//! | B — gameplay keys | [`KeyPolicy::rebuild`] | user, `ui.typing_ignore_keys` | yes |
//!
//! Layer A is not stored in the config and cannot be turned off — it is recomputed
//! from the parsed bindings whenever the hook re-parses. Layer B is the
//! user-facing list (see `docs/IGNORED_KEYS_FEATURE.md`).
//!
//! **The ordering is the contract:** `exempt_all` runs *after* `rebuild`, so no user
//! edit can un-exempt a bound hotkey. `docs/KEY_POLICY.md` §3 states it, and
//! `a_user_cannot_unexempt_a_bound_hotkey` pins it.
//!
//! # Zero-Jitter contract (`AGENTS.md` §2.17)
//!
//! This type is read from the `WH_KEYBOARD_LL` path, so:
//!
//! * **No lock** — not `Mutex`, not `RwLock`, not a `OnceLock` read on the hot path.
//! * **No allocation** — [`KeyPolicy::is_exempt`] is a relaxed load and a bit test.
//!   No `Vec`, no `String`, no iterator yielding owned values.
//! * **No file I/O, no logging, no `emit`** on the read side.
//! * **Single writer** — `rebuild` / `exempt_all` are called from the hook *loop*
//!   between event batches (and from `set_config`), never from inside a callback.
//!   The race is therefore structurally impossible; a lock would buy nothing.

use std::sync::atomic::{AtomicU64, Ordering};

/// Gameplay letter cluster — movement, inventory, drop, reload, use.
///
/// `R`/`Q`/`E`/`F` collide with common game actions and `R` is also the default
/// toggle hotkey, so they must never arm the lockout.
///
/// This is the **seed**, not the guarantee. It is a convenience for unbound keys;
/// the correctness guarantee for bound keys is layer A ([`KeyPolicy::exempt`]).
pub const GAMEPLAY_LETTERS: [u16; 8] = [
    0x41, // A
    0x44, // D
    0x45, // E
    0x46, // F
    0x51, // Q
    0x52, // R
    0x53, // S
    0x57, // W
];

/// 256 virtual-key codes = 4 × `u64`.
const WORDS: usize = 4;
const VK_MAX: u16 = 255;

/// Lock-free bitmap of virtual-key codes that never count as typing.
///
/// **TWO independent layers, two bitmaps** — see `docs/KEY_POLICY.md` §3:
///
/// * [`words_seed`] — the hardcoded defaults plus the user's list (layer B).
///   Rewritten wholesale by [`KeyPolicy::rebuild`].
/// * [`words_bound`] — the binding shield (layer A). Set-bit-only, never
///   cleared by [`KeyPolicy::rebuild`].
///
/// Keeping them separate is what makes "a config save must not un-exempt a
/// bound hotkey" a STRUCTURAL property rather than a hope about call order:
/// `rebuild` physically cannot touch layer A, so the window in which a bound
/// key would look like ordinary typing does not exist. (A single bitmap plus a
/// documented ordering leaves exactly the bug this module was written to kill.)
///
/// Relaxed ordering is deliberate and sufficient: the only invariant that matters
/// is that a word is either fully written or fully not, and every access goes
/// through the same `Relaxed` load/store pair, which guarantees that. A keypress
/// racing a config save may see the previous list — the consequence is one
/// armed-or-not keypress, never a crash and never a lost keystroke.
pub struct KeyPolicy {
    words_seed: [AtomicU64; WORDS],
    words_bound: [AtomicU64; WORDS],
}

impl Default for KeyPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyPolicy {
    /// Seed from the hardcoded defaults (layer B, no user list yet).
    pub fn new() -> Self {
        let policy = KeyPolicy {
            words_seed: [const { AtomicU64::new(0) }; WORDS],
            words_bound: [const { AtomicU64::new(0) }; WORDS],
        };
        policy.reset_seed();
        policy
    }

    /// Clear layer B and re-apply the hardcoded seed. Layer A is untouched.
    fn reset_seed(&self) {
        for word in &self.words_seed {
            word.store(0, Ordering::Relaxed);
        }
        for vk in 0..=VK_MAX {
            self.set_seed(vk, is_protected_key(vk));
        }
    }

    /// Clear layer B and re-apply the hardcoded seed, then overlay the user's
    /// list. **Layer B only.**
    ///
    /// The ONLY caller is `ClickScheduler::set_config`. It cannot clear the
    /// binding shield, because the shield lives in a separate bitmap this method
    /// never writes — so a config save can never un-exempt a bound hotkey,
    /// whatever the call order is (`a_user_cannot_unexempt_a_bound_hotkey`).
    ///
    /// Unknown labels are skipped rather than panicking — a hand-edited
    /// `config.json` must not be able to take the input layer down.
    pub fn rebuild(&self, user_labels: &[String]) {
        self.reset_seed();
        for label in user_labels {
            if let Some(vk) = vk_from_label(label) {
                self.set_seed(vk, true);
            }
        }
    }

    /// Mark one key as never-typing (layer A, automatic).
    ///
    /// Set-bit-only: there is deliberately no public way to remove a bound key.
    /// Values above 255 are ignored — the bitmap covers the standard virtual-key
    /// range, and an out-of-range code can never reach a real key-down.
    pub fn exempt(&self, vk: u16) {
        self.set_bound(vk, true);
    }

    /// Mark many keys at once (layer A, automatic).
    ///
    /// Takes an iterator so the caller can pass `bindings.policy_keys()` without
    /// building a `Vec` — this runs on the hook thread.
    pub fn exempt_all(&self, vks: impl Iterator<Item = u16>) {
        for vk in vks {
            self.exempt(vk);
        }
    }

    /// `true` when this key must never arm the Typing Guard (either layer).
    ///
    /// Hot path: two relaxed loads and two bit tests. No lock, no allocation,
    /// no syscall.
    pub fn is_exempt(&self, vk: u16) -> bool {
        if vk > VK_MAX {
            return false;
        }
        let index = usize::from(vk) / 64;
        let bit = 1u64 << (vk % 64);
        (self.words_seed[index].load(Ordering::Relaxed) & bit != 0)
            || (self.words_bound[index].load(Ordering::Relaxed) & bit != 0)
    }

    /// `true` when this key-down counts as real text input.
    ///
    /// The dynamic decision: `vk != 0` and not exempt (seed ∪ user list ∪ bound
    /// hotkeys). Replaces the static `is_text_keypress_vk` in the hook.
    pub fn is_text_keypress(&self, vk: u16) -> bool {
        vk != 0 && vk <= VK_MAX && !self.is_exempt(vk)
    }

    /// `true` when this key could plausibly be produced by typing — i.e. a bare
    /// hotkey on it is ambiguous and needs the toggle-confirmation window.
    ///
    /// Dynamic for one reason only: **an exempt key can never be a letter inside a
    /// word**, so a hotkey bound to an ignored key must fire instantly rather than
    /// wait for confirmation. The static part is [`is_typable_vk`].
    pub fn is_typable(&self, vk: u16) -> bool {
        if self.is_exempt(vk) {
            return false;
        }
        is_typable_vk(vk)
    }

    fn set_seed(&self, vk: u16, on: bool) {
        self.write(&self.words_seed, vk, on);
    }

    fn set_bound(&self, vk: u16, on: bool) {
        self.write(&self.words_bound, vk, on);
    }

    fn write(&self, words: &[AtomicU64; WORDS], vk: u16, on: bool) {
        if vk > VK_MAX {
            return;
        }
        let index = usize::from(vk) / 64;
        let bit = 1u64 << (vk % 64);
        if on {
            words[index].fetch_or(bit, Ordering::Relaxed);
        } else {
            words[index].fetch_and(!bit, Ordering::Relaxed);
        }
    }

    /// Diagnostics for Settings → Input diagnostics. Not on any hot path.
    pub fn report(&self) -> KeyPolicyReport {
        let mut exempt_total = 0u32;
        let mut gameplay_total = 0u32;
        let mut non_seed_keys = Vec::new();
        for vk in 0..=VK_MAX {
            if is_protected_key(vk) {
                gameplay_total += 1;
            }
            if !self.is_exempt(vk) {
                continue;
            }
            exempt_total += 1;
            if !is_protected_key(vk) {
                non_seed_keys.push(key_label(vk));
            }
        }
        KeyPolicyReport {
            exempt_total,
            gameplay_total,
            non_seed_keys,
        }
    }
}

/// Diagnostics payload for Settings → Input diagnostics.
#[derive(Debug, Clone, serde::Serialize)]
pub struct KeyPolicyReport {
    /// Total keys in force (seed ∪ user list ∪ bound hotkeys).
    pub exempt_total: u32,
    /// Size of the hardcoded seed, for the "you can edit the rest" wording.
    pub gameplay_total: u32,
    /// Keys in force that are NOT part of the seed — the user's list plus the
    /// automatic binding shield. Locked ones render without a delete button.
    pub non_seed_keys: Vec<String>,
}

/// Result of a pre-add check in the Settings UI.
#[derive(Debug, Clone, serde::Serialize)]
pub struct IgnoreKeyReport {
    pub label: String,
    pub vk: u16,
    /// May this label be added to the user's list?
    pub ok: bool,
    /// `None` when `ok`, otherwise `"unknown_label"` / `"duplicate"`.
    pub reason: Option<&'static str>,
    /// Part of the hardcoded seed — already ignored, adding is pointless.
    pub protected: bool,
    /// Bound to a hotkey — already ignored automatically, always, no matter what
    /// the user does.
    pub already_bound: bool,
}
/// `true` for keys the hardcoded seed already ignores.
///
/// Pure static predicate — kept here so [`KeyPolicy`] and the UI agree on what
/// "protected" means without duplicating the list. Moved verbatim from
/// `typing::is_ignored_key_vk`.
pub fn is_protected_key(vk: u16) -> bool {
    if GAMEPLAY_LETTERS.contains(&vk) {
        return true;
    }
    // Hotbar digits (top row) and the numpad digit block.
    if (0x30..=0x39).contains(&vk) || (0x60..=0x69).contains(&vk) {
        return true;
    }
    // Function keys F1..F24.
    if (0x70..=0x87).contains(&vk) {
        return true;
    }
    matches!(
        vk,
        0x01..=0x06
            | 0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0..=0xA5
            | 0x09
            | 0x13
            | 0x14
            | 0x1B
            | 0x20
            | 0x21
            | 0x22
            | 0x23
            | 0x24
            | 0x25
            | 0x26
            | 0x27
            | 0x28
            | 0x2C
            | 0x2D
            | 0x5D
            | 0x90
            | 0x91
            | 0x6A | 0x6B | 0x6D | 0x6E | 0x6F
    )
}

/// `true` when this key could plausibly be produced **by typing text**.
///
/// Pure, static, and unchanged from the original classifier — it is a property of
/// the key, not of the user's bindings, so it stays in `typing.rs` as the single
/// definition. [`KeyPolicy::is_typable`] adds the dynamic part on top of it.
use super::typing::is_typable_vk;

/// The hardcoded seed as user-facing labels, for a "Reset to defaults" action.
pub fn default_ignored_labels() -> Vec<String> {
    (0..=VK_MAX)
        .filter(|&vk| is_protected_key(vk))
        .map(key_label)
        .collect()
}

/// Canonical label for a virtual-key code ("A", "F5", "Ctrl", "Space").
pub fn key_label(vk: u16) -> String {
    match vk {
        0x20 => "Space".into(),
        0x09 => "Tab".into(),
        0x0D => "Enter".into(),
        0x1B => "Esc".into(),
        0x08 => "Backspace".into(),
        0x10 => "Shift".into(),
        0xA0 => "LShift".into(),
        0xA1 => "RShift".into(),
        0x11 => "Ctrl".into(),
        0xA2 => "LCtrl".into(),
        0xA3 => "RCtrl".into(),
        0x12 => "Alt".into(),
        0xA4 => "LAlt".into(),
        0xA5 => "RAlt".into(),
        0x5B => "WinLeft".into(),
        0x5C => "WinRight".into(),
        0x14 => "CapsLock".into(),
        0x90 => "NumLock".into(),
        0x91 => "ScrollLock".into(),
        0x2D => "Insert".into(),
        0x2E => "Delete".into(),
        0x2C => "PrintScreen".into(),
        0x25 => "Left".into(),
        0x26 => "Up".into(),
        0x27 => "Right".into(),
        0x28 => "Down".into(),
        0x21 => "PageUp".into(),
        0x22 => "PageDown".into(),
        0x23 => "End".into(),
        0x24 => "Home".into(),
        0x5D => "Apps".into(),
        0x13 => "Pause".into(),
        // 0x01..=0x06 are LBUTTON..XBUTTON2, printed as Mouse1..Mouse6 so the label
        // round-trips through `vk_from_label` (which accepts exactly 1..=6).
        _ if (0x01..=0x06).contains(&vk) => format!("Mouse{vk}"),
        _ if (0x70..=0x87).contains(&vk) => format!("F{}", vk - 0x6F),
        _ if (0x60..=0x69).contains(&vk) => format!("Num{}", vk - 0x60),
        _ if (0x41..=0x5A).contains(&vk) || (0x30..=0x39).contains(&vk) => {
            char::from_u32(u32::from(vk)).unwrap_or('?').to_string()
        }
        _ => format!("VK{vk:02X}"),
    }
}

/// Canonical label → virtual-key code. The inverse of [`key_label`].
///
/// Deliberately accepts a single key name (not a `+` chord): an ignored key is one
/// physical key, and accepting chords here would let the UI store something the
/// policy can never act on. Returns `None` for anything unrecognised so a
/// hand-edited config degrades to "ignored" rather than panicking.
pub fn vk_from_label(label: &str) -> Option<u16> {
    let label = label.trim();
    if label.is_empty() {
        return None;
    }
    let wanted = label.to_ascii_lowercase();
    (0..=VK_MAX).find(|&vk| key_label(vk).to_ascii_lowercase() == wanted)
}

/// Check a candidate label before the UI adds it to the user's list.
///
/// Explains *why* a key cannot be added, so the interface can teach the two-layer
/// model instead of silently refusing. `already_in_list` is the caller's knowledge;
/// everything else here is static.
pub fn check_ignore_label(label: &str, already_in_list: bool) -> IgnoreKeyReport {
    let trimmed = label.trim().to_string();
    let Some(vk) = vk_from_label(&trimmed) else {
        return IgnoreKeyReport {
            label: trimmed,
            vk: 0,
            ok: false,
            reason: Some("unknown_label"),
            protected: false,
            already_bound: false,
        };
    };
    let protected = is_protected_key(vk);
    let reason = if protected {
        Some("already_ignored")
    } else if already_in_list {
        Some("duplicate")
    } else {
        None
    };
    IgnoreKeyReport {
        label: trimmed,
        vk,
        ok: reason.is_none(),
        reason,
        protected,
        already_bound: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VK_J: u16 = 0x4A; // 'J' — the reported binding
    const VK_CTRL: u16 = 0x11;
    const VK_F6: u16 = 0x75;

    fn labels(vks: &[&str]) -> Vec<String> {
        vks.iter().map(|s| (*s).to_string()).collect()
    }

    /// THE primary test. A key the user bound to a hotkey can never arm the
    /// lockout that gates that very hotkey. This is the `J` bug, encoded.
    #[test]
    fn a_bound_key_can_never_arm_the_lockout_that_gates_it() {
        let policy = KeyPolicy::new();
        // Before the hook reports the binding, `J` would be classified as typing.
        assert!(
            policy.is_text_keypress(VK_J),
            "precondition: an unbound J is ordinary text"
        );

        policy.exempt(VK_J);

        assert!(
            !policy.is_text_keypress(VK_J),
            "a bound hotkey key must never arm the Typing Guard"
        );
    }

    /// The seed still works: WASD and the navigation keys keep gaming alive.
    #[test]
    fn the_seed_is_in_force_after_construction() {
        let policy = KeyPolicy::new();
        for vk in GAMEPLAY_LETTERS {
            assert!(!policy.is_text_keypress(vk), "seed key 0x{vk:02X}");
        }
        assert!(!policy.is_text_keypress(0x20), "Space must stay a gameplay key");
        assert!(!policy.is_text_keypress(0x26), "ArrowUp must stay a gameplay key");
        assert!(policy.is_text_keypress(VK_J), "J is not in the seed");
        assert!(policy.is_text_keypress(0x54), "T is not in the seed");
    }

    /// The user list is applied and normalised from labels.
    #[test]
    fn rebuild_applies_the_user_list() {
        let policy = KeyPolicy::new();
        policy.rebuild(&labels(&["c", " x ", "Z", "v"]));
        for vk in [0x43u16, 0x58, 0x5A, 0x56] {
            assert!(!policy.is_text_keypress(vk), "user key 0x{vk:02X}");
        }
        // A previous list is fully replaced, never merged.
        assert!(policy.is_text_keypress(VK_J), "J was not in the new list");
    }

    /// An unknown or malformed label must be skipped, never panic — a
    /// hand-edited config cannot take the input layer down.
    #[test]
    fn an_unknown_label_is_skipped_not_fatal() {
        let policy = KeyPolicy::new();
        policy.rebuild(&labels(&["", "NotAKey", "Mouse99", "F99", "Zz"]));
        assert!(policy.is_text_keypress(VK_J), "J still counts as typing");
    }

    /// THE ordering invariant. A config save must never drop the binding shield,
    /// because the hook re-applies it only on the NEXT re-parse.
    #[test]
    fn a_user_cannot_unexempt_a_bound_hotkey() {
        let policy = KeyPolicy::new();
        policy.exempt(VK_J);
        assert!(!policy.is_text_keypress(VK_J));

        // The user saves an unrelated config change; the hook has not
        // re-parsed yet, so `rebuild` is the only thing that ran.
        policy.rebuild(&labels(&["C", "X"]));

        assert!(
            !policy.is_text_keypress(VK_J),
            "a config save must not un-exempt a bound hotkey"
        );
    }

    /// Same ordering, for a key the user never sees listed.
    #[test]
    fn a_user_cannot_unexempt_a_protected_key() {
        let policy = KeyPolicy::new();
        policy.exempt(VK_CTRL);
        policy.rebuild(&[]);
        assert!(!policy.is_text_keypress(VK_CTRL), "Ctrl stays exempt");
    }

    /// A bound key also skips the toggle-confirmation window: it cannot be a
    /// letter inside a word, so it must fire instantly.
    #[test]
    fn an_exempt_key_is_never_typable() {
        let policy = KeyPolicy::new();
        assert!(policy.is_typable(VK_J), "precondition: J is typable");
        policy.exempt(VK_J);
        assert!(
            !policy.is_typable(VK_J),
            "an ignored key cannot be a letter inside a word"
        );
    }

    /// `F6` is not typable even when unbound — the static part still holds.
    #[test]
    fn the_static_typable_predicate_is_unchanged() {
        assert!(!is_typable_vk(VK_F6));
        assert!(!is_typable_vk(0x01), "a mouse button is never typable");
        let policy = KeyPolicy::new();
        assert!(!policy.is_typable(VK_F6));
    }

    /// Out-of-range codes are handled, not indexed out of bounds.
    #[test]
    fn codes_above_the_bitmap_are_safe() {
        let policy = KeyPolicy::new();
        assert!(!policy.is_exempt(0x1FF));
        assert!(!policy.is_text_keypress(0x1FF));
        policy.exempt(0x1FF);
        assert!(!policy.is_exempt(0x1FF));
        assert!(!policy.is_text_keypress(0), "vk 0 is never a keypress");
    }

    /// The shield must survive repeated re-application.
    #[test]
    fn exempting_twice_is_idempotent() {
        let policy = KeyPolicy::new();
        policy.exempt_all([VK_J, VK_CTRL].into_iter());
        policy.exempt_all([VK_J, VK_CTRL].into_iter());
        assert!(!policy.is_text_keypress(VK_J));
        assert!(!policy.is_text_keypress(VK_CTRL));
    }

    /// Label round-trip: what the UI shows can be parsed back.
    #[test]
    fn labels_round_trip() {
        for vk in [0x41u16, 0x5A, 0x30, 0x39, 0x60, 0x75, 0x20, 0x11, 0x01, 0x04] {
            let label = key_label(vk);
            assert_eq!(
                vk_from_label(&label),
                Some(vk),
                "label {label:?} must parse back to 0x{vk:02X}"
            );
        }
    }

    /// The default list is non-empty, parseable, and free of duplicates.
    #[test]
    fn the_default_list_is_parseable_and_unique() {
        let defaults = default_ignored_labels();
        assert!(!defaults.is_empty());
        let mut seen = std::collections::HashSet::new();
        for label in &defaults {
            let vk = vk_from_label(label).unwrap_or_else(|| panic!("{label:?}"));
            assert!(seen.insert(vk), "duplicate label {label:?}");
            assert!(is_protected_key(vk), "{label:?} must be a seed key");
        }
    }

    /// The report separates the seed from what the user added.
    #[test]
    fn the_report_lists_non_seed_keys() {
        let policy = KeyPolicy::new();
        let before = policy.report();
        policy.exempt(VK_J);
        let after = policy.report();

        assert_eq!(after.exempt_total, before.exempt_total + 1);
        assert!(after.non_seed_keys.contains(&"J".to_string()));
        assert!(!before.non_seed_keys.contains(&"J".to_string()));
    }

    /// The pre-add check explains refusals instead of silently failing.
    #[test]
    fn the_pre_add_check_explains_its_refusal() {
        let unknown = check_ignore_label("NotAKey", false);
        assert!(!unknown.ok);
        assert_eq!(unknown.reason, Some("unknown_label"));

        let protected = check_ignore_label("Ctrl", false);
        assert!(!protected.ok);
        assert!(protected.protected);

        let dup = check_ignore_label("X", true);
        assert!(!dup.ok);
        assert_eq!(dup.reason, Some("duplicate"));

        let good = check_ignore_label(" x ", false);
        assert!(good.ok);
        assert_eq!(good.label, "x");
    }

    /// Wiring: the static classifier production code used to call still exists
    /// as a pure function, but the hook must go through the policy (enforced in
    /// `tests/test_assets.rs`). This test pins the semantic difference.
    #[test]
    fn the_static_predicate_cannot_see_bindings() {
        // The old function still classifies J as text — which is exactly why the
        // hook must no longer call it. If this ever changes, the bug is back.
        assert!(super::super::typing::is_text_keypress_vk(VK_J));
    }
}
