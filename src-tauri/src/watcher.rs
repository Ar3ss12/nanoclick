//! Observer file watcher for the config directory (no locks, no rewrites).
//!
//! Polling `metadata().mtime` every second, debounced so a half-written file
//! (user still typing in Notepad) is judged only after it settles. Our OWN
//! atomic writes (tmp+rename) are ignored via a grace window. NEVER heals,
//! locks, or rewrites files — only classifies for toasts. Healing is boot-only.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How often the observer thread polls file mtimes.
pub const POLL_INTERVAL_MS: u64 = 1000;
/// Settle time: a file must be stable this long before we judge it.
pub const DEBOUNCE_MS: u64 = 750;
/// Ignore window after OUR OWN write (tmp+rename shows up as a change).
pub const OWN_WRITE_GRACE_MS: u64 = 2500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileHealth {
    Unchanged,
    ChangedValid,
    ChangedInvalid,
    IgnoredOwnWrite,
    Missing,
}

#[derive(Debug)]
struct WatchedFile {
    path: PathBuf,
    last_seen_mtime: Option<std::time::SystemTime>,
    pending_since: Option<Instant>,
    own_write_at: Option<Instant>,
    validate: fn(&str) -> bool,
}

impl WatchedFile {
    fn new(path: PathBuf, validate: fn(&str) -> bool) -> Self {
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        WatchedFile { path, last_seen_mtime: mtime, pending_since: None, own_write_at: None, validate }
    }
}

pub struct Observer {
    files: Mutex<HashMap<String, WatchedFile>>,
}

impl Observer {
    pub fn new() -> Self {
        Observer { files: Mutex::new(HashMap::new()) }
    }

    /// Register a file under a short key (`"config"`, `"macros"`).
    pub fn watch(&self, key: &str, path: PathBuf, validate: fn(&str) -> bool) {
        if let Ok(mut files) = self.files.lock() {
            files.insert(key.to_string(), WatchedFile::new(path, validate));
        }
    }

    /// Call right AFTER our own atomic save so the echo is suppressed.
    /// IMPORTANT: call BEFORE the write happens (or the FS may collapse both
    /// writes into one mtime tick and there is no visible echo to swallow).
    /// `poll_once` then treats the next mtime movement inside the grace
    /// window as ours and returns `IgnoredOwnWrite` instead of a toast.
    pub fn mark_own_write(&self, key: &str) {
        if let Ok(mut files) = self.files.lock() {
            if let Some(f) = files.get_mut(key) {
                f.own_write_at = Some(Instant::now());
                // Do NOT fast-forward the baseline here: the echo hasn't
                // landed yet, and adopting the pre-write mtime would make us
                // blind to it. Baseline advances in `poll_once` when the echo
                // is swallowed.
                f.pending_since = None;
            }
        }
    }

    /// One poll step for a single key. Pure classifier, no I/O beyond
    /// metadata+read of the watched file itself.
    pub fn poll_once(&self, key: &str) -> FileHealth {
        let mut files = match self.files.lock() {
            Ok(g) => g,
            Err(_) => return FileHealth::Missing,
        };
        let f = match files.get_mut(key) {
            Some(f) => f,
            None => return FileHealth::Missing,
        };
        if !f.path.exists() {
            return FileHealth::Missing;
        }
        let mtime = match std::fs::metadata(&f.path).and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => return FileHealth::Missing,
        };
        if Some(mtime) == f.last_seen_mtime {
            f.pending_since = None;
            return FileHealth::Unchanged;
        }
        // Our own echo inside the grace window: swallow once, adopt baseline.
        if let Some(t) = f.own_write_at {
            if t.elapsed() < Duration::from_millis(OWN_WRITE_GRACE_MS) {
                f.last_seen_mtime = Some(mtime);
                f.pending_since = None;
                return FileHealth::IgnoredOwnWrite;
            }
            f.own_write_at = None;
        }
        // Debounce: judge only after the mtime stops moving (user typing).
        let now = Instant::now();
        match f.pending_since {
            None => {
                f.pending_since = Some(now);
                FileHealth::Unchanged
            }
            Some(since) if since.elapsed() < Duration::from_millis(DEBOUNCE_MS) => {
                FileHealth::Unchanged
            }
            Some(_) => {
                f.pending_since = None;
                f.last_seen_mtime = Some(mtime);
                let valid = std::fs::read_to_string(&f.path)
                    .map(|s| (f.validate)(&s))
                    .unwrap_or(false);
                if valid { FileHealth::ChangedValid } else { FileHealth::ChangedInvalid }
            }
        }
    }

    /// Poll every registered key; returns `(key, health)` for loud verdicts.
    pub fn poll_all(&self) -> Vec<(String, FileHealth)> {
        let keys: Vec<String> = match self.files.lock() {
            Ok(g) => g.keys().cloned().collect(),
            Err(_) => return Vec::new(),
        };
        let mut out = Vec::new();
        for k in keys {
            match self.poll_once(&k) {
                FileHealth::Unchanged | FileHealth::IgnoredOwnWrite | FileHealth::Missing => {}
                h => out.push((k, h)),
            }
        }
        out
    }
}

impl Default for Observer {
    fn default() -> Self {
        Self::new()
    }
}

/// Strict validity for `config.json` bytes (no repair: watcher judges).
pub fn config_bytes_valid(raw: &str) -> bool {
    let t = raw.trim().trim_start_matches('\u{feff}');
    if t.is_empty() {
        return false;
    }
    match serde_json::from_str::<serde_json::Value>(t) {
        Ok(v) => crate::defaults::migrate_and_validate(v).is_ok(),
        Err(_) => false,
    }
}

/// Strict validity for `macros.json` bytes.
pub fn macros_bytes_valid(raw: &str) -> bool {
    let t = raw.trim().trim_start_matches('\u{feff}');
    if t.is_empty() {
        return false;
    }
    serde_json::from_str::<crate::persistence::macros::MacroStore>(t).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn always_valid(_: &str) -> bool { true }
    fn always_invalid(_: &str) -> bool { false }

    fn tmp_file(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nanoclick_watch_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join(name)
    }

    #[test]
    fn own_write_echo_is_swallowed() {
        let p = tmp_file("own.json");
        let _ = std::fs::write(&p, "{}");
        let o = Observer::new();
        o.watch("k", p.clone(), always_valid);
        // Mark BEFORE our write (production order: mark -> save -> echo).
        o.mark_own_write("k");
        // Force a distinct mtime tick: NTFS granularity can collapse
        // back-to-back writes, so nudge the file into the next tick.
        std::thread::sleep(std::time::Duration::from_millis(25));
        let _ = std::fs::write(&p, "{\"a\":1}");
        assert_eq!(o.poll_once("k"), FileHealth::IgnoredOwnWrite);
        // Swallowed echo adopts the baseline: next poll is quiet.
        assert_eq!(o.poll_once("k"), FileHealth::Unchanged);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn invalid_change_surfaces_only_after_settle() {
        let p = tmp_file("inv.json");
        let _ = std::fs::write(&p, "{}");
        let o = Observer::new();
        o.watch("k", p.clone(), always_invalid);
        // Distinct mtime tick: NTFS granularity can collapse back-to-back writes.
        std::thread::sleep(std::time::Duration::from_millis(25));
        let _ = std::fs::write(&p, "{broken");
        assert_eq!(o.poll_once("k"), FileHealth::Unchanged);
        {
            let mut files = o.files.lock().unwrap();
            let f = files.get_mut("k").unwrap();
            f.pending_since = Some(Instant::now() - Duration::from_millis(DEBOUNCE_MS + 50));
        }
        assert_eq!(o.poll_once("k"), FileHealth::ChangedInvalid);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn valid_change_surfaces_after_settle() {
        let p = tmp_file("ok.json");
        let _ = std::fs::write(&p, "{}");
        let o = Observer::new();
        o.watch("k", p.clone(), always_valid);
        // Distinct mtime tick (same NTFS reason as above).
        std::thread::sleep(std::time::Duration::from_millis(25));
        let _ = std::fs::write(&p, "{\"a\":1}");
        assert_eq!(o.poll_once("k"), FileHealth::Unchanged);
        {
            let mut files = o.files.lock().unwrap();
            let f = files.get_mut("k").unwrap();
            f.pending_since = Some(Instant::now() - Duration::from_millis(DEBOUNCE_MS + 50));
        }
        assert_eq!(o.poll_once("k"), FileHealth::ChangedValid);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn no_change_stays_quiet() {
        let p = tmp_file("quiet.json");
        let _ = std::fs::write(&p, "{}");
        let o = Observer::new();
        o.watch("k", p.clone(), always_valid);
        assert_eq!(o.poll_once("k"), FileHealth::Unchanged);
        assert_eq!(o.poll_once("k"), FileHealth::Unchanged);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn config_validator_accepts_good_rejects_broken() {
        // Embedded default is the ground truth of "valid": it must pass.
        // Truncated / empty bytes are never valid.
        assert!(config_bytes_valid(crate::defaults::EMBEDDED_DEFAULT_CONFIG_JSON));
        assert!(!config_bytes_valid("{\"a\":1}."));
        assert!(!config_bytes_valid(""));
    }
}
