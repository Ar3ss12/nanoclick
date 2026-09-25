//! Smart Guard — isolated safety features that gate the click loop.
//!
//! Both guards live in their own modules so `scheduler.rs` stays a thin
//! orchestration layer and this logic never bloats the click hot path:
//!
//! * [`typing`]     — Typing Guard: short click freeze after real text
//!                    input. Gameplay keys (WASD, hotbar, arrows, ...)
//!                    are deliberately ignored so gaming never pauses.
//! * [`app_filter`] — App/window scope filter (everywhere | whitelist |
//!                    blacklist) driven by the foreground process name.
//!
//! Cost contract for the click loop:
//! * Typing Guard poll = 2 atomic loads, zero syscalls.
//! * App filter poll = 1 atomic load while disabled; when enabled the
//!   foreground lookup goes through [`ForegroundCache`] (300 ms TTL) so
//!   even a 150 CPS loop pays the 2 syscalls at most ~3 times/second.

pub mod app_filter;
pub mod focus_watch;
pub mod typing;

pub use app_filter::{AppFilter, ForegroundCache};
pub use focus_watch::{spawn_focus_watcher, FocusWatchStop};
pub use typing::{is_text_keypress_vk, is_typable_vk, TypingGuard, TYPING_FREEZE_MS};

/// Smart Guard default freeze window for the UI checkbox (milliseconds).
pub fn default_typing_freeze_ms() -> u32 {
    TYPING_FREEZE_MS
}

/// How long a typable toggle hotkey is held back before it is accepted.
///
/// The guard alone cannot see the hotkey letter itself: when `R` is the first
/// letter of `rush`, nothing has armed the lockout yet. Delaying the toggle
/// lets the very next letter cancel it, so an isolated press (a deliberate
/// switch) still fires while a press inside a word never does.
///
/// Delay for toggle confirmation (0 = instant firing when not typing).
/// The confirmation delay was removed per user feedback to eliminate latency
/// and unify control around two visible UI parameters:
/// 1) Typing Lockout (ms)
/// 2) Toggle Response Time (ms)
pub const TOGGLE_CONFIRM_MS: u64 = 0;

/// Wall-clock milliseconds since the Unix epoch.
///
/// Only differences are ever used (guard windows), so `wrap` arithmetic at
/// the call sites keeps this correct across any clock oddity.
pub(crate) fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Poll-while-blocked interval of the click loop (milliseconds).
///
/// Used ONLY when the app filter blocks: freeze the loop cheaply instead of
/// hammering the foreground lookup. Small enough to resume instantly, large
/// enough to stay off the CPU. (Focus-loss reaction time is owned by the
/// watcher thread — [`FOCUS_WATCH_MS`] — not by this constant.)
pub const GUARD_POLL_MS: u64 = 50;

/// Poll interval of the FocusGuard watcher thread (milliseconds).
///
/// The click loop sleeps most of its life inside `wait_until` (up to a full
/// click interval, ~1000 ms at 1 CPS, plus hold/start-delay sleeps), so a
/// check that lives only inside the loop reaches the user late. The watcher
/// polls the foreground directly (no TTL cache) and wakes the sleeper via
/// the shared stop event: worst-case reaction ≈ this value. 50 ms keeps the
/// extra syscall rate at ~20/s while the guard is ON; the thread exists only
/// for the duration of a guarded run.
pub const FOCUS_WATCH_MS: u64 = 50;
