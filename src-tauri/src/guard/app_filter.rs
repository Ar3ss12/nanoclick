//! App / window scope filter — decide whether the clicker may fire in the
//! currently focused application.
//!
//! Three modes, all opt-in:
//! * [`FilterMode::Everywhere`] — default, zero overhead, no syscalls.
//! * [`FilterMode::Blacklist`]  — allow everywhere except the listed apps.
//! * [`FilterMode::Whitelist`]  — allow *only* in the listed apps.
//!
//! Entries are process image names (`discord.exe`, `javaw.exe`), normalized
//! to lowercase by [`crate::config::normalize_app_filter_list`] so a user
//! typing `Discord.EXE` still matches.
//!
//! # Cost
//! Resolving the foreground process costs 2 syscalls, which is far too much
//! per click at 150 CPS. [`ForegroundCache`] therefore memoizes the lookup
//! for [`FOREGROUND_TTL_MS`].
//!
//! # Fail-open
//! Whenever the foreground process cannot be determined (lock screen, an
//! elevated process we cannot query, our own window) the filter allows the
//! click. A guard that silently kills clicking is worse than a guard that
//! occasionally lets one through — the user always keeps the emergency stop.

use super::now_ms;
use crate::config::normalize_app_filter_list;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

/// How long a foreground-process lookup is trusted before re-querying.
pub const FOREGROUND_TTL_MS: u64 = 300;

/// Which applications the clicker is allowed to operate in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    /// No restriction (default).
    Everywhere,
    /// Only the listed apps are allowed.
    Whitelist,
    /// Listed apps are blocked, everything else is allowed.
    Blacklist,
}

impl FilterMode {
    /// Parse a persisted config string (aliases tolerated).
    pub fn from_config_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "whitelist" | "white" | "only" => FilterMode::Whitelist,
            "blacklist" | "black" | "block" => FilterMode::Blacklist,
            _ => FilterMode::Everywhere,
        }
    }

    /// Canonical string persisted to the config file.
    pub fn as_config_str(self) -> &'static str {
        match self {
            FilterMode::Whitelist => "whitelist",
            FilterMode::Blacklist => "blacklist",
            FilterMode::Everywhere => "everywhere",
        }
    }

    fn encode(self) -> u32 {
        match self {
            FilterMode::Everywhere => 0,
            FilterMode::Whitelist => 1,
            FilterMode::Blacklist => 2,
        }
    }

    fn decode(v: u32) -> Self {
        match v {
            1 => FilterMode::Whitelist,
            2 => FilterMode::Blacklist,
            _ => FilterMode::Everywhere,
        }
    }

    /// Pure decision — fully unit-testable, no state, no syscalls.
    ///
    /// `fg_exe` is the lowercase foreground image name, or `None` when it
    /// could not be determined (fail-open — see module docs).
    pub fn allows(self, list: &[String], fg_exe: Option<&str>) -> bool {
        match (self, fg_exe) {
            (FilterMode::Everywhere, _) | (_, None) => true,
            (FilterMode::Whitelist, Some(exe)) => list.iter().any(|item| item == exe),
            (FilterMode::Blacklist, Some(exe)) => !list.iter().any(|item| item == exe),
        }
    }
}

impl Default for FilterMode {
    fn default() -> Self {
        FilterMode::Everywhere
    }
}

/// Live app-filter state shared between the click loop and the UI writes.
pub struct AppFilter {
    mode: AtomicU32,
    list: Mutex<Vec<String>>,
}

impl AppFilter {
    pub fn new(mode: &str, list: &[String]) -> Self {
        AppFilter {
            mode: AtomicU32::new(FilterMode::from_config_str(mode).encode()),
            list: Mutex::new(normalize_app_filter_list(list)),
        }
    }

    pub fn set(&self, mode: &str, list: &[String]) {
        self.mode
            .store(FilterMode::from_config_str(mode).encode(), Ordering::Relaxed);
        if let Ok(mut guard) = self.list.lock() {
            *guard = normalize_app_filter_list(list);
        }
    }

    pub fn mode(&self) -> FilterMode {
        FilterMode::decode(self.mode.load(Ordering::Relaxed))
    }

    pub fn list(&self) -> Vec<String> {
        self.list.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// `true` when a filter is actually configured (drives the cache: a
    /// disabled filter must never pay for a foreground lookup).
    pub fn is_active(&self) -> bool {
        self.mode() != FilterMode::Everywhere
    }

    /// Decision using the current mode + list.
    ///
    /// Our own window always fails open (control panel, never a click
    /// target): a whitelist that does not list us must not freeze the loop
    /// while the user is pressing buttons in NanoClick.
    pub fn allows(&self, fg_exe: Option<&str>) -> bool {
        let mode = self.mode();
        if mode == FilterMode::Everywhere {
            return true;
        }
        match fg_exe {
            None => true, // fail-open: lock screen / elevated
            Some(exe) if crate::platform::is_own_exe(exe) => true,
            Some(exe) => mode.allows(&self.list(), Some(exe)),
        }
    }
}

impl Default for AppFilter {
    fn default() -> Self {
        AppFilter::new("everywhere", &[])
    }
}

/// TTL-memoized `platform::get_foreground_process_name()`.
///
/// Lives on the click-loop thread — no locking contention, the mutex is
/// uncontended by construction.
#[derive(Default)]
pub struct ForegroundCache {
    slot: Mutex<CachedForeground>,
}

#[derive(Default)]
struct CachedForeground {
    exe: Option<String>,
    at_ms: u64,
}

impl ForegroundCache {
    pub fn new() -> Self {
        ForegroundCache::default()
    }

    /// Lowercase foreground image name, cached for [`FOREGROUND_TTL_MS`].
    pub fn exe(&self) -> Option<String> {
        let now = now_ms();
        if let Ok(guard) = self.slot.lock() {
            if guard.at_ms != 0 && now.wrapping_sub(guard.at_ms) < FOREGROUND_TTL_MS {
                return guard.exe.clone();
            }
        }
        let fresh = crate::platform::get_foreground_process_name();
        if let Ok(mut guard) = self.slot.lock() {
            guard.exe = fresh.clone();
            guard.at_ms = now;
        }
        fresh
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Vec<String> {
        normalize_app_filter_list(&items.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn mode_strings_roundtrip_and_accept_aliases() {
        for (label, expected) in [
            ("whitelist", FilterMode::Whitelist),
            ("white", FilterMode::Whitelist),
            ("WHITELIST", FilterMode::Whitelist),
            ("blacklist", FilterMode::Blacklist),
            ("block", FilterMode::Blacklist),
            ("everywhere", FilterMode::Everywhere),
            ("", FilterMode::Everywhere),
            ("garbage", FilterMode::Everywhere),
        ] {
            let mode = FilterMode::from_config_str(label);
            assert_eq!(mode, expected, "label {label:?}");
            // Canonical form must survive a re-parse.
            assert_eq!(FilterMode::from_config_str(mode.as_config_str()), mode);
            assert_eq!(FilterMode::decode(mode.encode()), mode);
        }
        assert_eq!(FilterMode::default(), FilterMode::Everywhere);
    }

    #[test]
    fn everywhere_mode_never_blocks() {
        let listed = list(&["discord.exe"]);
        assert!(FilterMode::Everywhere.allows(&listed, Some("discord.exe")));
        assert!(FilterMode::Everywhere.allows(&listed, Some("javaw.exe")));
        assert!(FilterMode::Everywhere.allows(&listed, None));
    }

    #[test]
    fn blacklist_blocks_only_listed_apps() {
        let blocked = list(&["discord.exe", "telegram.exe"]);
        assert!(!FilterMode::Blacklist.allows(&blocked, Some("discord.exe")));
        assert!(!FilterMode::Blacklist.allows(&blocked, Some("telegram.exe")));
        assert!(FilterMode::Blacklist.allows(&blocked, Some("javaw.exe")));
    }

    #[test]
    fn whitelist_allows_only_listed_apps() {
        let allowed = list(&["javaw.exe"]);
        assert!(FilterMode::Whitelist.allows(&allowed, Some("javaw.exe")));
        assert!(!FilterMode::Whitelist.allows(&allowed, Some("chrome.exe")));
        // Empty whitelist blocks everything deterministic.
        assert!(!FilterMode::Whitelist.allows(&[], Some("chrome.exe")));
    }

    #[test]
    fn unknown_foreground_fails_open() {
        // A guard that silently kills clicking is worse than one that
        // occasionally lets a click through.
        let strict = list(&["javaw.exe"]);
        assert!(FilterMode::Whitelist.allows(&strict, None));
        assert!(FilterMode::Blacklist.allows(&strict, None));
    }

    #[test]
    fn matching_is_case_insensitive_via_normalization() {
        let mixed = vec!["  Discord.EXE ".to_string(), "TELEGRAM.exe".to_string()];
        let filter = AppFilter::new("blacklist", &mixed);
        assert_eq!(filter.list(), vec!["discord.exe", "telegram.exe"]);
        assert!(!filter.allows(Some("discord.exe")));
        assert!(filter.allows(Some("chrome.exe")));
    }

    #[test]
    fn state_switch_and_disable_release_the_block() {
        let filter = AppFilter::new("everywhere", &[]);
        assert!(!filter.is_active());

        filter.set("blacklist", &["discord.exe".to_string()]);
        assert!(filter.is_active());
        assert_eq!(filter.mode(), FilterMode::Blacklist);
        assert!(!filter.allows(Some("discord.exe")));

        // Switching back to "everywhere" must fully release the block.
        filter.set("everywhere", &["discord.exe".to_string()]);
        assert!(!filter.is_active());
        assert!(filter.allows(Some("discord.exe")));
    }

    #[test]
    fn blacklist_entries_are_deduped_and_sorted() {
        let messy = vec![
            "Discord.exe".to_string(),
            "discord.exe".to_string(),
            "  ".to_string(),
            "chrome.exe".to_string(),
        ];
        let filter = AppFilter::new("blacklist", &messy);
        assert_eq!(filter.list(), vec!["chrome.exe", "discord.exe"]);
    }

    #[test]
    fn foreground_cache_reuses_recent_lookup() {
        let cache = ForegroundCache::new();
        // First call queries the platform; both calls must agree because the
        // TTL window prevents a second query.
        let first = cache.exe();
        let second = cache.exe();
        assert_eq!(first, second);
    }

    #[test]
    fn own_window_always_fails_open() {
        use crate::platform::own_exe_name;
        let Some(own) = own_exe_name() else {
            return;
        };
        // A whitelist that does not list us must not freeze the loop while
        // the user is pressing buttons in NanoClick (control panel).
        let white = AppFilter::new("whitelist", &["javaw.exe".to_string()]);
        assert!(white.allows(Some(own.as_str())));
        let black = AppFilter::new("blacklist", &["discord.exe".to_string()]);
        assert!(black.allows(Some(own.as_str())));
    }
}
