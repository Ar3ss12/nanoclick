//! Typing Guard — do not let a hotkey fire right after the user typed text.
//!
//! # The real problem this solves
//! While you type, the autoclicker is already idle. The pain is not "it keeps
//! clicking" — it is that a single-key hotkey sits **inside a word**:
//!
//! ```text
//! user types:  g o _ r u s h _ B
//!                       ^
//!                       R is the toggle hotkey -> the clicker switches ON
//!                       mid-sentence and clicks into the chat box
//! ```
//!
//! So instead of freezing the click loop (which costs performance and does
//! nothing useful), the *hotkey itself* is gated: if a real text key was
//! pressed less than the guard window ago, the toggle is ignored — the fingers
//! were still typing, that was a letter, not a deliberate switch.
//!
//! Typing stops → the window elapses → the hotkey works instantly again.
//!
//! # How detection works — a blocklist, not a whitelist
//! Every key counts as typing **except** the ones gamers and everyday users
//! hammer without typing anything: movement, actions, modifiers, navigation
//! and hotbar digits. A whitelist would silently miss keys we forgot to list
//! (European OEM keys, media keys, exotic layouts); a blocklist fails safe —
//! the worst case is one extra armed keypress.
//!
//! ## Never arms the lockout
//! Movement / actions: `W A S D`, `Q E R F`
//! Modifiers:          `Shift` `Ctrl` `Alt` `Win` (and their L/R variants)
//! Navigation:         `Space` `Tab` `Esc` `Caps` `NumLock` `ScrollLock`
//!                     `Pause` `PrintScreen` `Apps`
//!                     `Up` `Down` `Left` `Right` `Home` `End` `PgUp` `PgDn`
//!                     `Insert`
//! Hotbar digits:      `0..9`, numpad `Num0..Num9` and numpad `* + - . /`
//! Function keys:      `F1..F24`
//!
//! ## Arms the lockout — everything else
//! Letters outside the gameplay set (`T`, `Y`, `U`, `H`, `G`, ...)
//! `Enter` (open/send chat), `Backspace` / `Delete` (edit it)
//! OEM punctuation keys (`;` `=` `,` `-` `.` `/` `~` `[` `\` `]` `'`)
//! Plus any other key the system reports.
//!
//! Note how `R` itself is *ignored* as an arming key — it is a gameplay key.
//! That is fine: by the time the user reaches the `r` of `rush`, the `g` and
//! `o` they typed a moment earlier already armed the lockout.
//!
//! Disabled guard (`pause_ms == 0`) is zero-cost: [`TypingGuard::note`]
//! returns before touching memory.


use super::now_ms;
use std::sync::atomic::{AtomicU64, Ordering};

/// Freeze window the UI checkbox enables (milliseconds).
///
/// This is the "fingers were still typing" window: a toggle pressed sooner
/// than this after a text key is treated as a letter inside a word.
/// 800ms covers average typing pause between words (~600ms) plus margin.
pub const TYPING_FREEZE_MS: u32 = 800;

/// Gameplay letter cluster — movement, inventory, drop, reload, use.
/// `R`/`Q`/`E`/`F` collide with common game actions and `R` is also the
/// default toggle hotkey, so they must never arm the lockout.
const GAMEPLAY_LETTERS: [u16; 8] = [
    0x41, // A
    0x44, // D
    0x45, // E
    0x46, // F
    0x51, // Q
    0x52, // R
    0x53, // S
    0x57, // W
];

/// `true` when this virtual-key counts as real text input, i.e. the fingers
/// are typing rather than driving a game.
///
/// Blocklist semantics: anything the OS reports counts as typing unless it is
/// a known gameplay / navigation key (see [`is_ignored_key_vk`]). Pure integer
/// classification — safe to call from the low-level keyboard hook for every
/// key-down (no allocation, no syscalls, no logging).
pub fn is_text_keypress_vk(vk: u16) -> bool {
    vk != 0 && !is_ignored_key_vk(vk)
}

/// Keys gamers and everyday users press constantly **without** typing text.
///
/// Keeping this list narrow is intentional: the classifier fails safe, so a
/// missed key only means one extra armed keypress, never a lost keystroke.
fn is_ignored_key_vk(vk: u16) -> bool {
    // Movement / action letters.
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
        // Modifiers: generic and left/right variants.
        0x10 | 0x11 | 0x12 | 0x5B | 0x5C | 0xA0..=0xA5
        // Navigation, locks and system keys.
        | 0x09  // Tab (scoreboard)
        | 0x13  // Pause
        | 0x14  // CapsLock
        | 0x1B  // Esc (menus)
        | 0x20  // Space (gameplay jump key - never interrupts gaming/clicking)
        | 0x21  // PageUp
        | 0x22  // PageDown
        | 0x23  // End
        | 0x24  // Home
        | 0x25  // ArrowLeft
        | 0x26  // ArrowUp
        | 0x27  // ArrowRight
        | 0x28  // ArrowDown
        | 0x2C  // PrintScreen
        | 0x2D  // Insert
        | 0x5D  // Apps / context menu
        | 0x90  // NumLock
        | 0x91  // ScrollLock
        // Numpad operators.
        | 0x6A | 0x6B | 0x6D | 0x6E | 0x6F
    )
}

/// `true` when this key could plausibly be produced **by typing text**.
///
/// A bare hotkey on such a key (`R`, `K`, `1`, `.`) is ambiguous: it may be a
/// letter inside a word. Those presses are therefore held back briefly to see
/// whether the user is still typing. Hotkeys on keys that typing never
/// produces (`F6`, `Ctrl+Alt+M`, side mouse buttons) skip that delay.
pub fn is_typable_vk(vk: u16) -> bool {
    matches!(
        vk,
        0x08                        // Backspace
        | 0x0D                      // Enter
        | 0x20                      // Space
    ) || (0x30..=0x39).contains(&vk)  // digit row
        || (0x41..=0x5A).contains(&vk) // A..Z
        || (0x60..=0x69).contains(&vk) // numpad digits
        || (0xBA..=0xC0).contains(&vk) // OEM punctuation
        || (0xDB..=0xDE).contains(&vk) // OEM punctuation (brackets/quotes)
    // Numpad operators (`* + - . /`) are deliberately NOT listed: they are
    // calculator / spreadsheet keys — the same class as the hotbar digits —
    // so a hotkey bound to one fires instantly.
}

/// Typing Guard state. Written by the keyboard-hook thread, read by the
/// keyboard-hook thread (hotkey gate) and the UI status poll — plain
/// atomics, no locks, no cross-thread coupling.
pub struct TypingGuard {
    /// Lockout window in ms (0 = disabled).
    pause_ms: AtomicU64,
    /// Timestamp of the last real text keypress (0 = never).
    last_text_ms: AtomicU64,
    /// How many text keypresses have armed the lockout. Diagnostics only:
    /// lets the UI prove the low-level hook is actually delivering keys.
    text_events: AtomicU64,
    /// Deadline (ms) of a toggle press held back for confirmation
    /// (0 = nothing pending).
    pending_deadline: AtomicU64,
}

impl TypingGuard {
    pub fn new(pause_ms: u32) -> Self {
        TypingGuard {
            pause_ms: AtomicU64::new(u64::from(pause_ms)),
            last_text_ms: AtomicU64::new(0),
            text_events: AtomicU64::new(0),
            pending_deadline: AtomicU64::new(0),
        }
    }

    /// Update the lockout window.
    ///
    /// The armed timestamp is dropped **only when the value actually
    /// changes**. This matters because `set_config` runs on every config
    /// save, and the stats engine saves the config every 10 seconds while
    /// clicking — unconditionally clearing here would silently release a
    /// lockout that is currently in effect.
    pub fn set_pause_ms(&self, pause_ms: u32) {
        let previous = self.pause_ms.load(Ordering::Relaxed);
        self.pause_ms.store(u64::from(pause_ms), Ordering::Relaxed);
        if previous != u64::from(pause_ms) {
            self.last_text_ms.store(0, Ordering::Relaxed);
        }
    }

    pub fn pause_ms(&self) -> u32 {
        self.pause_ms.load(Ordering::Relaxed) as u32
    }

    pub fn is_enabled(&self) -> bool {
        self.pause_ms.load(Ordering::Relaxed) > 0
    }

    /// Number of text keypresses seen since launch (diagnostics).
    pub fn text_events(&self) -> u64 {
        self.text_events.load(Ordering::Relaxed)
    }

    /// Record a real text keypress (called from the LL-keyboard hook).
    ///
    /// MUST be called *after* the hotkey groups have been evaluated for the
    /// same key-down, otherwise a hotkey that is itself a typing key would
    /// lock itself out forever.
    ///
    /// Also cancels any in-flight toggle confirmation: a second text key
    /// proves the held-back press was a letter inside a word.
    ///
    /// Zero-cost when the guard is disabled.
    pub fn note(&self) {
        if !self.is_enabled() {
            return;
        }
        self.cancel_pending();
        self.text_events.fetch_add(1, Ordering::Relaxed);
        self.last_text_ms.store(now_ms(), Ordering::Relaxed);
    }

    /// Remaining lockout time in ms (0 = hotkeys are free to fire).
    pub fn lock_remaining_ms(&self) -> u64 {
        let window = self.pause_ms.load(Ordering::Relaxed);
        if window == 0 {
            return 0;
        }
        let last = self.last_text_ms.load(Ordering::Relaxed);
        if last == 0 {
            return 0;
        }
        window.saturating_sub(now_ms().wrapping_sub(last))
    }

    /// `true` while the user is still typing — the toggle hotkey must be
    /// ignored so a letter inside a word cannot switch the clicker on.
    pub fn hotkeys_locked(&self) -> bool {
        self.lock_remaining_ms() > 0
    }

    /// Decision helper for the keyboard hook: may this hotkey press fire?
    pub fn allows_hotkey(&self) -> bool {
        !self.hotkeys_locked()
    }

    /* ── toggle confirmation (ambiguous single-key hotkeys) ───────── */

    /// Try to open a confirmation window for a matched toggle press.
    ///
    /// Returns `true` when the caller should fire the toggle immediately.
    /// Returns `false` when the press was handled by the guard — either
    /// rejected outright (still typing) or held back for confirmation, in
    /// which case the caller polls [`TypingGuard::poll_pending`].
    ///
    /// A press on a key that typing cannot produce (`F6`, side mouse button)
    /// always fires instantly: it cannot be a letter inside a word, so adding
    /// latency there would only make deliberate hotkeys feel laggy.
    ///
    /// A disabled guard (`pause_ms == 0`) is fully transparent: no lockout and
    /// no confirmation delay — the feature must disappear when switched off.
    pub fn try_arm_toggle(&self, now_ms: u64, confirm_ms: u64, typable: bool) -> bool {
        if !self.is_enabled() {
            return true;
        }
        if self.hotkeys_locked() {
            // Still inside the "you are typing" window. Re-arm it so a burst
            // of text cannot squeeze a toggle through a momentary gap.
            self.last_text_ms.store(now_ms, Ordering::Relaxed);
            return false;
        }
        if !typable || confirm_ms == 0 {
            return true;
        }
        self.pending_deadline
            .store(now_ms.wrapping_add(confirm_ms), Ordering::Relaxed);
        false
    }

    /// Drop a pending confirmation.
    ///
    /// Called from [`TypingGuard::note`] for every text key: that extra key is
    /// exactly the proof that the held-back press was a letter inside a word.
    pub fn cancel_pending(&self) {
        self.pending_deadline.store(0, Ordering::Relaxed);
    }

    /// `true` while a press is being held back for confirmation.
    pub fn is_toggle_pending(&self) -> bool {
        self.pending_deadline.load(Ordering::Relaxed) != 0
    }

    /// Resolve a pending confirmation whose deadline has passed.
    ///
    /// Returns `Some(true)` when the press is confirmed and the toggle must
    /// fire, `Some(false)` when it was cancelled, `None` while still waiting.
    ///
    /// Wrapping-safe deadline comparison: the difference is treated as signed,
    /// so the check stays correct across the u64 wrap.
    pub fn poll_pending(&self, now_ms: u64) -> Option<bool> {
        let deadline = self.pending_deadline.load(Ordering::Relaxed);
        if deadline == 0 {
            return None;
        }
        if (now_ms.wrapping_sub(deadline) as i64) < 0 {
            return None; // deadline not reached yet
        }
        self.pending_deadline.store(0, Ordering::Relaxed);
        // Typing that started during the hold-back window also cancels: the
        // lockout is the authoritative "fingers are still typing" signal.
        Some(!self.hotkeys_locked())
    }
}

impl Default for TypingGuard {
    fn default() -> Self {
        TypingGuard::new(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gameplay_keys_never_count_as_typing() {
        // WASD + QERF movement/action cluster.
        for vk in [0x41u16, 0x57, 0x53, 0x44, 0x51, 0x45, 0x52, 0x46] {
            assert!(
                !is_text_keypress_vk(vk),
                "gameplay key 0x{vk:02X} must not arm the typing freeze"
            );
        }
        // Space / Tab / Esc / modifiers.
        for vk in [
            0x20u16, 0x09, 0x1B, 0x10, 0x11, 0x12, 0x5B, 0x5C, 0xA0, 0xA5,
        ] {
            assert!(!is_text_keypress_vk(vk), "0x{vk:02X} must be ignored");
        }
        // Hotbar digits, function keys, arrows, numpad.
        for vk in [
            0x30u16, 0x39, 0x70, 0x87, 0x25, 0x26, 0x27, 0x28, 0x60, 0x69, 0x6A,
        ] {
            assert!(!is_text_keypress_vk(vk), "0x{vk:02X} must be ignored");
        }
    }

    #[test]
    fn chat_letters_and_enter_count_as_typing() {
        // T Y U H G — opening chat and typing a message.
        for vk in [0x54u16, 0x59, 0x55, 0x48, 0x47] {
            assert!(
                is_text_keypress_vk(vk),
                "letter 0x{vk:02X} must arm the typing freeze"
            );
        }
        // Enter (send), Backspace/Delete (edit), punctuation.
        for vk in [0x0D_u16, 0x08, 0x2E, 0xBA, 0xBF, 0xC0, 0xDB, 0xDE] {
            assert!(is_text_keypress_vk(vk), "0x{vk:02X} must be counted");
        }
    }

    /// The classifier is a blocklist: any key we did not explicitly classify
    /// as gameplay/navigation still counts as typing. This is what keeps
    /// exotic layouts and media keys from silently disarming the guard.
    #[test]
    fn unlisted_keys_count_as_typing_by_default() {
        for vk in [
            0xE2u16, // OEM_102 — the extra key on European keyboards
            0xA6,    // BrowserBack
            0xB3,    // MediaPlayPause
            0xC1,    // unmapped OEM
            0x5A,    // Z — a letter that is not part of the gameplay cluster
            0x0D,    // Enter
        ] {
            assert!(
                is_text_keypress_vk(vk),
                "unlisted key 0x{vk:02X} must count as typing"
            );
        }
        // A zero scancode is noise, not a keypress.
        assert!(!is_text_keypress_vk(0));
    }

    /// `is_typable_vk` decides whether a hotkey needs the confirmation window.
    /// Getting this wrong is what makes the guard feel broken: a typable key
    /// that is not delayed lets the toggle fire inside a word.
    #[test]
    fn typable_classifier_flags_ambiguous_hotkeys() {
        // Ambiguous: typing produces these, so they need confirmation.
        for vk in [
            0x52u16, // R — the default toggle and a common word letter
            0x4B,    // K — the alternate default toggle
            0x31,    // 1 — hotbar in game, but also a chat digit
            0xBE,    // OEM period
            0x0D,    // Enter
            0x20,    // Space
            0x08,    // Backspace
        ] {
            assert!(is_typable_vk(vk), "0x{vk:02X} must need confirmation");
        }
        // Unambiguous: typing cannot produce these, so no delay is added.
        for vk in [0x76u16, 0x70, 0x1B, 0x2D, 0x6A, 0x11] {
            assert!(!is_typable_vk(vk), "0x{vk:02X} must fire instantly");
        }
    }

    #[test]
    fn guard_is_inert_until_armed() {
        let guard = TypingGuard::new(0);
        assert!(!guard.is_enabled());
        guard.note();
        assert!(!guard.hotkeys_locked(), "disabled guard must never lock");
        assert_eq!(guard.lock_remaining_ms(), 0);
        assert!(guard.allows_hotkey());

        // Armed but nothing typed yet = hotkey must work immediately.
        let armed = TypingGuard::new(TYPING_FREEZE_MS);
        assert!(armed.is_enabled());
        assert!(!armed.hotkeys_locked());
        assert!(armed.allows_hotkey());
    }

    #[test]
    fn note_locks_the_hotkey_and_disabling_releases_it() {
        let guard = TypingGuard::new(TYPING_FREEZE_MS);
        guard.note();
        assert!(guard.hotkeys_locked());
        assert!(!guard.allows_hotkey());
        let remaining = guard.lock_remaining_ms();
        assert!(
            remaining > 0 && remaining <= u64::from(TYPING_FREEZE_MS),
            "remaining {remaining} must sit inside the window"
        );

        // Disabling releases the lockout immediately.
        guard.set_pause_ms(0);
        assert!(!guard.is_enabled());
        assert!(guard.allows_hotkey());
    }

    #[test]
    fn lockout_expires_so_the_hotkey_works_again() {
        // Short window keeps the test fast while exercising the real clock.
        let guard = TypingGuard::new(60);
        guard.note();
        assert!(!guard.allows_hotkey(), "hotkey must be ignored right after typing");

        std::thread::sleep(std::time::Duration::from_millis(80));
        assert!(
            guard.allows_hotkey(),
            "after the window elapses the hotkey must fire again"
        );
    }

    /// The exact bug this feature exists for: the user types `go rush B` and
    /// the `r` of `rush` is the toggle hotkey. The `g`/`o` typed a moment
    /// earlier must lock it out, otherwise the clicker switches on mid-word.
    #[test]
    fn letter_inside_a_word_does_not_toggle_the_clicker() {
        let guard = TypingGuard::new(TYPING_FREEZE_MS);

        // "go " typed: G and O arm the lockout, space is a gameplay key.
        assert!(is_text_keypress_vk(0x47), "G is typing");
        assert!(is_text_keypress_vk(0x4F), "O is typing");
        assert!(!is_text_keypress_vk(0x20), "Space is a gameplay key");
        guard.note();

        // ...and now the user hits R (the toggle hotkey) inside "rush".
        assert!(!is_text_keypress_vk(0x52), "R must never arm the lockout itself");
        assert!(
            !guard.allows_hotkey(),
            "R inside a word must not switch the clicker on"
        );
    }

    #[test]
    fn note_counts_text_events_for_diagnostics() {
        let guard = TypingGuard::new(TYPING_FREEZE_MS);
        assert_eq!(guard.text_events(), 0);
        guard.note();
        guard.note();
        assert_eq!(guard.text_events(), 2);

        // Disabled guard stays at zero-cost and does not count.
        let off = TypingGuard::new(0);
        off.note();
        assert_eq!(off.text_events(), 0);
    }

    /* ── toggle confirmation ──────────────────────────────────────── */

    /// The case the lockout alone cannot catch: `R` is the FIRST letter of a
    /// word, so nothing armed the lockout before the press. The press must be
    /// held back, and the next letter must cancel it.
    #[test]
    fn first_letter_of_a_word_is_held_back_then_cancelled() {
        let guard = TypingGuard::new(TYPING_FREEZE_MS);
        let t0 = 1_000_000;

        // User starts "rush": R is the toggle hotkey and a bare typable key.
        let fire_now = guard.try_arm_toggle(t0, 200, true);
        assert!(!fire_now, "a typable hotkey must not fire instantly");
        assert!(guard.is_toggle_pending());

        // 'u' arrives 90 ms later -> it was a letter inside a word.
        guard.note();
        assert!(!guard.is_toggle_pending(), "the next letter cancels the press");

        // Nothing to resolve, and the toggle never fired.
        assert_eq!(guard.poll_pending(t0 + 500), None);
    }

    /// A deliberate press is isolated: the window elapses with no further
    /// typing and the toggle fires.
    #[test]
    fn isolated_press_is_confirmed_after_the_window() {
        let guard = TypingGuard::new(TYPING_FREEZE_MS);
        let t0 = 2_000_000;

        assert!(!guard.try_arm_toggle(t0, 200, true));
        assert!(guard.is_toggle_pending());

        // Still inside the window: nothing to resolve yet.
        assert_eq!(guard.poll_pending(t0 + 100), None);
        // Window elapsed -> confirmed, exactly once.
        assert_eq!(guard.poll_pending(t0 + 250), Some(true));
        assert_eq!(guard.poll_pending(t0 + 300), None, "must fire only once");
    }

    /// A press on a key that typing cannot produce (`F6`) fires with zero
    /// delay — there is nothing ambiguous to confirm.
    #[test]
    fn non_typable_hotkey_fires_instantly() {
        let guard = TypingGuard::new(TYPING_FREEZE_MS);
        assert!(guard.try_arm_toggle(3_000_000, 200, false));
        assert!(!guard.is_toggle_pending());
    }

    /// While the lockout is active the toggle is rejected outright, and the
    /// lockout is re-armed so a burst of text cannot squeeze a press through
    /// a momentary gap.
    #[test]
    fn press_during_typing_is_rejected_and_refreshes_the_lockout() {
        let guard = TypingGuard::new(TYPING_FREEZE_MS);
        guard.note();

        assert!(!guard.try_arm_toggle(crate::guard::now_ms(), 200, true));
        assert!(
            !guard.is_toggle_pending(),
            "a press while typing must be rejected, not deferred"
        );
        assert!(guard.hotkeys_locked(), "and the lockout must stay armed");
    }

    /// With the guard disabled nothing is ever deferred: the feature must be
    /// completely transparent when the user turns it off.
    #[test]
    fn disabled_guard_never_defers_a_toggle() {
        let guard = TypingGuard::new(0);
        assert!(guard.try_arm_toggle(4_000_000, 200, true));
        assert!(!guard.is_toggle_pending());
    }

    /// Regression: `set_config` runs on every config save, and the stats
    /// engine saves the config every 10 s while clicking. Re-pushing the
    /// SAME value must never release a lockout that is in effect.
    #[test]
    fn repushing_same_value_does_not_release_active_lockout() {
        let guard = TypingGuard::new(TYPING_FREEZE_MS);
        guard.note();
        assert!(guard.hotkeys_locked());

        // Routine config save with an unchanged value.
        guard.set_pause_ms(TYPING_FREEZE_MS);
        assert!(
            guard.hotkeys_locked(),
            "an unchanged config push must not release the lockout"
        );

        // Changing the value is allowed to re-arm from scratch.
        guard.set_pause_ms(1200);
        assert!(guard.allows_hotkey(), "a changed window restarts the guard");
    }

    /// Verification: Space bar is treated as a gameplay jump key, so jumping
    /// in-game never blocks hotkeys or interrupts active clicking.
    #[test]
    fn space_is_gameplay_key_and_does_not_interrupt_clicking() {
        assert!(!is_text_keypress_vk(0x20), "Space must be treated as a gameplay key");
    }
}
