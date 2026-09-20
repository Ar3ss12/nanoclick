//! # Persistence — disk storage for macros and presets.
//!
//! Macros live in their own file (`macros.json`) so that they can be
//! added/removed independently of the main AppConfig. This keeps Presets
//! (engine config) and Macros (recorded/built sequences) clearly separated.
//!
//! Reference: `docs/MACRO_ARCHITECTURE.md` §7.

use crate::core::Macro;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const MACROS_FILE: &str = "macros.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MacroStore {
    /// All saved macros, indexed by UUID.
    pub macros: Vec<Macro>,
}

/// Snapshot / quarantine outcome for `macros.json` (mirrors the config LKG scheme,
/// but a broken macro store heals to an EMPTY store, never to factory config).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacrosHealAction {
    LoadedExisting,
    CreatedMissing,
    RecoveredFromLastGood { backup_path: PathBuf },
    ResetEmpty { backup_path: PathBuf },
}

const MACROS_LAST_GOOD_FILE: &str = "macros.last_good.json";

fn macros_last_good_path(live: &std::path::Path) -> PathBuf {
    match live.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join(MACROS_LAST_GOOD_FILE),
        _ => PathBuf::from(MACROS_LAST_GOOD_FILE),
    }
}

fn macros_quarantine_path(live: &std::path::Path) -> PathBuf {
    let parent = live.parent().unwrap_or_else(|| std::path::Path::new("."));
    let stem = live
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(MACROS_FILE);
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    parent.join(format!("{stem}.broken-{epoch}"))
}

/// Harden a path READONLY+HIDDEN on Windows (best-effort, black-box snapshot).
/// Shared pattern with `defaults::harden_last_good` (duplicated: no shared
/// helper exists yet and persistence must not depend on defaults internals).
#[cfg(target_os = "windows")]
fn harden_snapshot(path: &std::path::Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_READONLY,
        FILE_FLAGS_AND_ATTRIBUTES,
    };
    use windows::core::PCWSTR;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let _ = SetFileAttributesW(
            PCWSTR::from_raw(wide.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(
                FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_READONLY.0,
            ),
        );
    }
}

#[cfg(target_os = "windows")]
fn unharden_snapshot(path: &std::path::Path) {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        SetFileAttributesW, FILE_ATTRIBUTE_NORMAL, FILE_FLAGS_AND_ATTRIBUTES,
    };
    use windows::core::PCWSTR;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let _ = SetFileAttributesW(
            PCWSTR::from_raw(wide.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(FILE_ATTRIBUTE_NORMAL.0),
        );
    }
}

#[cfg(not(target_os = "windows"))]
fn harden_snapshot(_path: &std::path::Path) {}
#[cfg(not(target_os = "windows"))]
fn unharden_snapshot(_path: &std::path::Path) {}

/// Get the path to `macros.json` next to `app_config.json`.
pub fn macros_path() -> PathBuf {
    // Mirror the config directory chosen by ConfigManager so portable
    // mode keeps everything (config + macros) next to the executable.
    if crate::config_manager::ConfigManager::is_portable() {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .map(|p| p.join("nanoclick_data").join(MACROS_FILE))
            .unwrap_or_else(|| PathBuf::from("./nanoclick_data").join(MACROS_FILE))
    } else {
        let dirs = directories::ProjectDirs::from("com", "nanoclick", "NanoClick")
            .expect("ProjectDirs resolve");
        dirs.config_dir().join(MACROS_FILE)
    }
}

/// Load all saved macros from disk with LKG self-healing (no silent loss):
/// clean parse refreshes `macros.last_good.json`; unparseable store is
/// quarantined with a timestamp and healed from the snapshot (or reset to an
/// EMPTY store when no snapshot exists). Returns the action for notices.
pub fn load_macros_healed() -> (Vec<Macro>, MacrosHealAction) {
    let path = macros_path();
    if !path.exists() {
        return (Vec::new(), MacrosHealAction::CreatedMissing);
    }
    match fs::read_to_string(&path) {
        Ok(s) if !s.trim().is_empty() => match parse_macro_store(&s) {
            Ok(store) => {
                write_macros_snapshot(&path, &store);
                (store.macros, MacrosHealAction::LoadedExisting)
            }
            Err(_) => recover_macros_corrupted(&path),
        },
        _ => recover_macros_corrupted(&path),
    }
}

fn recover_macros_corrupted(path: &std::path::Path) -> (Vec<Macro>, MacrosHealAction) {
    // Quarantine stays VISIBLE + writable: user evidence, never hardened.
    let quarantine = macros_quarantine_path(path);
    let _ = fs::copy(path, &quarantine);
    if let Some(good) = load_macros_snapshot(path) {
        let json = serde_json::to_string_pretty(&good).unwrap_or_default();
        if !json.is_empty() {
            let tmp = path.with_extension("json.tmp");
            if fs::write(&tmp, json).is_ok() {
                let _ = fs::rename(&tmp, path);
            }
        }
        eprintln!("[macros] unrecoverable; restored last-good snapshot. Quarantine: {:?}", quarantine);
        return (good.macros, MacrosHealAction::RecoveredFromLastGood { backup_path: quarantine });
    }
    let empty = serde_json::to_string_pretty(&MacroStore::default()).unwrap_or_default();
    if !empty.is_empty() {
        let tmp = path.with_extension("json.tmp");
        if fs::write(&tmp, empty).is_ok() {
            let _ = fs::rename(&tmp, path);
        }
    }
    eprintln!("[macros] unrecoverable, no snapshot; reset to empty. Quarantine: {:?}", quarantine);
    (Vec::new(), MacrosHealAction::ResetEmpty { backup_path: quarantine })
}

/// Write the golden snapshot of the macro store (tmp+rename, then harden).
/// Called ONLY after the live store parsed cleanly or was saved by us.
fn write_macros_snapshot(path: &std::path::Path, store: &MacroStore) {
    let snap = macros_last_good_path(path);
    unharden_snapshot(&snap);
    let json = match serde_json::to_string_pretty(store) {
        Ok(j) => j,
        Err(_) => return,
    };
    let tmp = snap.with_extension("json.tmp");
    if fs::write(&tmp, json).is_ok() && fs::rename(&tmp, &snap).is_ok() {
        harden_snapshot(&snap);
    }
}

fn load_macros_snapshot(path: &std::path::Path) -> Option<MacroStore> {
    let snap = macros_last_good_path(path);
    let raw = fs::read_to_string(&snap).ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    // Strict parse only: a snapshot that needs repair is not "known good".
    serde_json::from_str::<MacroStore>(raw.trim().trim_start_matches('\u{feff}')).ok()
}

/// Load all saved macros from disk.
pub fn load_macros() -> Vec<Macro> {
    load_macros_healed().0
}

pub fn parse_macro_store(json: &str) -> Result<MacroStore, String> {
    serde_json::from_str::<MacroStore>(json).map_err(|e| e.to_string())
}

/// Persist all macros to disk (atomic tmp+rename) and refresh the golden
/// snapshot, so a later corruption restores THIS state, not an older one.
pub fn save_macros(macros: &[Macro]) -> Result<(), String> {
    let path = macros_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {:?}: {}", parent, e))?;
    }
    let store = MacroStore {
        macros: macros.to_vec(),
    };
    let json = serde_json::to_string_pretty(&store).map_err(|e| format!("serialize: {}", e))?;
    // Atomic write: temp file + rename so a crash mid-write can't leave a
    // truncated macros.json (same pattern as config_manager::save).
    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, json).map_err(|e| format!("write {:?}: {}", tmp_path, e))?;
    fs::rename(&tmp_path, &path)
        .map_err(|e| format!("rename {:?} -> {:?}: {}", tmp_path, path, e))?;
    // Our own write is proven good by construction: snapshot it now.
    write_macros_snapshot(&path, &store);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::action::{KeyCode, Modifiers, MouseButton};
    use crate::core::{Action, RepeatMode};

    fn dummy() -> Macro {
        Macro {
            id: "test-1".into(),
            name: "Test".into(),
            icon: "🎬".into(),
            actions: vec![
                Action::MouseClick {
                    button: MouseButton::Left,
                    count: 1,
                },
                Action::Wait { ms: 100 },
                Action::KeyPress {
                    key: KeyCode(0x41),
                    mods: Modifiers {
                        ctrl: false,
                        shift: false,
                        alt: false,
                        win: false,
                    },
                },
            ],
            repeat: RepeatMode::Once,
            enabled: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    #[test]
    fn macro_store_roundtrip() {
        let m = dummy();
        let store = MacroStore {
            macros: vec![m.clone()],
        };
        let json = serde_json::to_string(&store).unwrap();
        let parsed: MacroStore = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.macros.len(), 1);
        assert_eq!(parsed.macros[0].id, m.id);
        assert_eq!(parsed.macros[0].actions.len(), 3);
    }

    #[test]
    fn corrupted_macro_json_is_rejected_without_panicking() {
        assert!(parse_macro_store("{ definitely not json").is_err());
        assert!(load_from_json_or_empty("{ definitely not json").is_empty());
    }

    #[test]
    fn long_macro_roundtrips_all_actions() {
        let mut m = dummy();
        m.actions = (0..2_000).map(|_| Action::Wait { ms: 25 }).collect();
        let json = serde_json::to_string(&MacroStore { macros: vec![m] }).unwrap();
        let parsed = parse_macro_store(&json).unwrap();
        assert_eq!(parsed.macros[0].actions.len(), 2_000);
    }

    fn tmp_store_path(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("nanoclick_macros_{}_{}", std::process::id(), tag));
        let _ = std::fs::create_dir_all(&dir);
        dir.join("macros.json")
    }

    #[test]
    fn macros_lkg_restores_snapshot_not_empty() {
        let live = tmp_store_path("lkg");
        let _ = std::fs::remove_file(live.with_file_name("macros.last_good.json"));
        // Simulate a good store on disk, then heal-load to seed the snapshot.
        let store = MacroStore { macros: vec![dummy()] };
        let json = serde_json::to_string_pretty(&store).unwrap();
        std::fs::write(&live, &json).unwrap();
        // Seed via the same path the app uses (parse then snapshot).
        let parsed: MacroStore = serde_json::from_str(&json).unwrap();
        super::write_macros_snapshot(&live, &parsed);
        // Break the live file: healed load must restore the snapshot.
        std::fs::write(&live, "{broken...").unwrap();
        let (macros, action) = super::recover_macros_corrupted(&live);
        assert_eq!(macros.len(), 1);
        assert_eq!(macros[0].id, "test-1");
        match action {
            MacrosHealAction::RecoveredFromLastGood { backup_path } => assert!(backup_path.exists()),
            other => panic!("expected LKG recovery, got {:?}", other),
        }
        let _ = std::fs::remove_file(&live);
        let _ = std::fs::remove_file(live.with_file_name("macros.last_good.json"));
    }

    #[test]
    fn macros_missing_snapshot_resets_empty_with_quarantine() {
        let live = tmp_store_path("empty");
        let _ = std::fs::remove_file(live.with_file_name("macros.last_good.json"));
        std::fs::write(&live, "{broken...").unwrap();
        let (macros, action) = super::recover_macros_corrupted(&live);
        assert!(macros.is_empty());
        match action {
            MacrosHealAction::ResetEmpty { backup_path } => assert!(backup_path.exists()),
            other => panic!("expected empty reset, got {:?}", other),
        }
        let _ = std::fs::remove_file(&live);
    }

    fn load_from_json_or_empty(json: &str) -> Vec<Macro> {
        parse_macro_store(json)
            .map(|store| store.macros)
            .unwrap_or_default()
    }
}
