//! # Persistence — disk storage for the preset library.
//!
//! Presets live in their own file (`presets.json`) next to `config.json` and `macros.json`, with
//! their own last-good snapshot.
//!
//! They used to be one element of `AppConfig` (`config.json.presets`), so the only way to save a
//! preset was to rewrite the whole config — and a page that saved a stale (or factory) copy of
//! the config replaced the entire library behind the user's back. The fingerprint of that failure
//! is a `config.json` holding exactly the four factory presets. A per-library store with its own
//! snapshot removes the class: no config write can touch it, and this module is its only writer.
//!
//! Reference: `docs/MACRO_ARCHITECTURE.md` §7 (the same scheme `macros.json` uses).

use crate::config_manager::PresetItem;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const PRESETS_FILE: &str = "presets.json";
const PRESETS_LAST_GOOD_FILE: &str = "presets.last_good.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PresetStore {
    /// The whole preset library, in display order.
    pub presets: Vec<PresetItem>,
}

/// Outcome of a load, so the boot sequence can report what happened (mirrors the macro store:
/// a broken `presets.json` heals to ITS OWN snapshot, never to a config or the factory list).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresetsHealAction {
    LoadedExisting,
    CreatedMissing,
    RecoveredFromLastGood { backup_path: PathBuf },
    ResetEmpty { backup_path: PathBuf },
}

fn presets_last_good_path(live: &std::path::Path) -> PathBuf {
    match live.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join(PRESETS_LAST_GOOD_FILE),
        _ => PathBuf::from(PRESETS_LAST_GOOD_FILE),
    }
}

fn presets_quarantine_path(live: &std::path::Path) -> PathBuf {
    let parent = live.parent().unwrap_or_else(|| std::path::Path::new("."));
    let stem = live
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(PRESETS_FILE);
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    parent.join(format!("{stem}.broken-{epoch}"))
}

/// Get the path to `presets.json` next to `config.json` / `macros.json`.
pub fn presets_path() -> PathBuf {
    // Mirror the config directory chosen by ConfigManager so portable mode keeps everything
    // (config + presets + macros) next to the executable.
    if crate::config_manager::ConfigManager::is_portable() {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .map(|p| p.join("nanoclick_data").join(PRESETS_FILE))
            .unwrap_or_else(|| PathBuf::from("./nanoclick_data").join(PRESETS_FILE))
    } else {
        let dirs = directories::ProjectDirs::from("com", "nanoclick", "NanoClick")
            .expect("ProjectDirs resolve");
        dirs.config_dir().join(PRESETS_FILE)
    }
}

/// Strict validity check for the file watcher (`presets.json` edited outside the app).
pub fn parse_preset_store(json: &str) -> Result<PresetStore, String> {
    serde_json::from_str::<PresetStore>(json).map_err(|e| e.to_string())
}

/// Write the golden snapshot (tmp+rename). Called only after the live store parsed cleanly or was
/// written by us, so a later corruption restores THIS library, not an older one.
fn write_presets_snapshot(path: &std::path::Path, store: &PresetStore) {
    let snap = presets_last_good_path(path);
    let json = match serde_json::to_string_pretty(store) {
        Ok(j) => j,
        Err(_) => return,
    };
    let tmp = snap.with_extension("json.tmp");
    if fs::write(&tmp, json).is_ok() {
        let _ = fs::rename(&tmp, &snap);
    }
}

fn load_presets_snapshot(path: &std::path::Path) -> Option<PresetStore> {
    let snap = presets_last_good_path(path);
    let raw = fs::read_to_string(&snap).ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    // Strict parse only: a snapshot that needs repair is not "known good".
    parse_preset_store(raw.trim().trim_start_matches('\u{feff}')).ok()
}

fn write_store_at(path: &std::path::Path, presets: &[PresetItem]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir {:?}: {}", parent, e))?;
        }
    }
    let store = PresetStore {
        presets: presets.to_vec(),
    };
    let json = serde_json::to_string_pretty(&store).map_err(|e| format!("serialize: {}", e))?;
    // Atomic write: temp + rename, so a crash mid-write cannot truncate the library
    // (same pattern as config_manager::save and save_macros).
    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, json).map_err(|e| format!("write {:?}: {}", tmp_path, e))?;
    fs::rename(&tmp_path, path)
        .map_err(|e| format!("rename {:?} -> {:?}: {}", tmp_path, path, e))?;
    // Our own write is proven good by construction: snapshot it now.
    write_presets_snapshot(path, &store);
    Ok(())
}

fn load_at(path: &std::path::Path) -> (Vec<PresetItem>, PresetsHealAction) {
    let raw = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return (Vec::new(), PresetsHealAction::CreatedMissing),
    };
    if let Ok(store) = parse_preset_store(raw.trim().trim_start_matches('\u{feff}')) {
        write_presets_snapshot(path, &store);
        return (store.presets, PresetsHealAction::LoadedExisting);
    }
    // Broken: quarantine the bytes (visible and editable, never hidden), then heal.
    let quarantine = presets_quarantine_path(path);
    let _ = fs::copy(path, &quarantine);
    if let Some(good) = load_presets_snapshot(path) {
        let _ = write_store_at(path, &good.presets);
        return (
            good.presets,
            PresetsHealAction::RecoveredFromLastGood {
                backup_path: quarantine,
            },
        );
    }
    eprintln!(
        "[presets] unrecoverable, no snapshot; reset to empty. Quarantine: {:?}",
        quarantine
    );
    (
        Vec::new(),
        PresetsHealAction::ResetEmpty {
            backup_path: quarantine,
        },
    )
}

/// Load the library, healing a broken file from its own snapshot.
pub fn load_presets_healed() -> (Vec<PresetItem>, PresetsHealAction) {
    load_at(&presets_path())
}

/// Load the library (the commands' entry point).
pub fn load_presets() -> Vec<PresetItem> {
    load_presets_healed().0
}

/// Persist the whole library (atomic; refreshes the snapshot).
pub fn save_presets(presets: &[PresetItem]) -> Result<(), String> {
    write_store_at(&presets_path(), presets)
}

/// Create the store on the first boot of this build, from the legacy `config.json.presets` array
/// — the caller passes that array, because only it holds the loaded config. Returns the number of
/// presets written, or `None` when the store already existed: an existing store is the authority
/// and is never overwritten by the config copy.
pub fn ensure_store_seeded(legacy: &[PresetItem]) -> Result<Option<usize>, String> {
    let path = presets_path();
    if path.exists() {
        return Ok(None);
    }
    // A legacy config always carries the field; when it somehow does not, the factory list is the
    // honest starting point (the same list `AppConfig`'s serde default falls back to).
    let seed: Vec<PresetItem> = if legacy.is_empty() {
        crate::config_manager::default_presets()
    } else {
        legacy.to_vec()
    };
    save_presets(&seed)?;
    Ok(Some(seed.len()))
}

/// Add or replace ONE preset by id, returning the new library.
pub fn upsert_preset(p: PresetItem) -> Result<Vec<PresetItem>, String> {
    let mut all = load_presets();
    match all.iter_mut().find(|x| x.id == p.id) {
        Some(existing) => *existing = p,
        None => all.push(p),
    }
    save_presets(&all)?;
    Ok(all)
}

/// Remove one preset by id, returning the new library.
pub fn delete_preset(id: &str) -> Result<Vec<PresetItem>, String> {
    let mut all = load_presets();
    all.retain(|x| x.id != id);
    save_presets(&all)?;
    Ok(all)
}

/// Replace the whole library (import, duplicate, reorder), returning what was stored.
pub fn replace_presets(presets: &[PresetItem]) -> Result<Vec<PresetItem>, String> {
    save_presets(presets)?;
    Ok(load_presets())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A live-file path in a private temp dir (the public helpers write to the real config dir,
    /// which a unit test must never touch).
    fn temp_live(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nanoclick_presets_{}_{}",
            std::process::id(),
            tag
        ));
        let _ = fs::create_dir_all(&dir);
        let live = dir.join(PRESETS_FILE);
        let _ = fs::remove_file(&live);
        let _ = fs::remove_file(presets_last_good_path(&live));
        live
    }

    #[test]
    fn store_roundtrip_keeps_the_library() {
        let live = temp_live("roundtrip");
        let mut list = crate::config_manager::default_presets();
        list[0].name = "Mine".into();
        write_store_at(&live, &list).unwrap();
        let (loaded, action) = load_at(&live);
        assert_eq!(action, PresetsHealAction::LoadedExisting);
        assert_eq!(loaded.len(), list.len());
        assert_eq!(loaded[0].name, "Mine");
        // A clean load refreshes the snapshot, so a later corruption restores THIS library.
        assert!(presets_last_good_path(&live).exists());
    }

    #[test]
    fn corrupt_store_heals_from_its_own_snapshot() {
        let live = temp_live("heal");
        let mut list = crate::config_manager::default_presets();
        list.truncate(2);
        list[1].name = "Second".into();
        write_store_at(&live, &list).unwrap();
        fs::write(&live, "{ not json").unwrap();
        let (loaded, action) = load_at(&live);
        assert_eq!(loaded.len(), 2, "the snapshot must come back, not an empty store");
        assert_eq!(loaded[1].name, "Second");
        assert!(matches!(
            action,
            PresetsHealAction::RecoveredFromLastGood { .. }
        ));
        // The broken bytes stay on disk for inspection, never silently deleted.
        assert!(presets_quarantine_path(&live).exists());
    }

    #[test]
    fn missing_store_reports_created_missing() {
        let live = temp_live("missing");
        let (loaded, action) = load_at(&live);
        assert!(loaded.is_empty());
        assert_eq!(action, PresetsHealAction::CreatedMissing);
    }
}
