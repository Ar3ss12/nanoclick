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

/// Get the path to `macros.json` next to `app_config.json`.
pub fn macros_path() -> PathBuf {
    let dirs = directories::ProjectDirs::from("com", "nanoclick", "NanoClick")
        .expect("ProjectDirs resolve");
    dirs.config_dir().join(MACROS_FILE)
}

/// Load all saved macros from disk.
pub fn load_macros() -> Vec<Macro> {
    let path = macros_path();
    if !path.exists() {
        return Vec::new();
    }
    match fs::read_to_string(&path) {
        Ok(s) => match parse_macro_store(&s) {
            Ok(store) => store.macros,
            Err(e) => {
                eprintln!("[macros] failed to parse {:?}: {}", path, e);
                Vec::new()
            }
        },
        Err(e) => {
            eprintln!("[macros] failed to read {:?}: {}", path, e);
            Vec::new()
        }
    }
}

pub fn parse_macro_store(json: &str) -> Result<MacroStore, String> {
    serde_json::from_str::<MacroStore>(json).map_err(|e| e.to_string())
}

/// Persist all macros to disk.
pub fn save_macros(macros: &[Macro]) -> Result<(), String> {
    let path = macros_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {:?}: {}", parent, e))?;
    }
    let store = MacroStore {
        macros: macros.to_vec(),
    };
    let json = serde_json::to_string_pretty(&store).map_err(|e| format!("serialize: {}", e))?;
    fs::write(&path, json).map_err(|e| format!("write {:?}: {}", path, e))?;
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

    fn load_from_json_or_empty(json: &str) -> Vec<Macro> {
        parse_macro_store(json)
            .map(|store| store.macros)
            .unwrap_or_default()
    }
}
