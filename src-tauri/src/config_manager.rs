use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

pub const CONFIG_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineSettings {
    pub target_cps: f64,
    pub jitter_percent: f64,
    pub click_limit: u32,
    pub jitter_radius_px: u32,
    pub button: String,
    #[serde(default = "default_click_type")]
    pub click_type: String, // "single", "double", "hold"
    #[serde(default = "default_position_mode")]
    pub position_mode: String, // "cursor", "fixed"
    #[serde(default = "default_100")]
    pub fixed_x: i32,
    #[serde(default = "default_100")]
    pub fixed_y: i32,
    #[serde(default = "default_repeat_mode")]
    pub repeat_mode: String, // "unlimited", "repeat"
    #[serde(default)]
    pub repeat_count: u32,
    #[serde(default = "default_500")]
    pub hold_duration_ms: u64,
    #[serde(default = "default_1000")]
    pub hold_interval_ms: u64,
    #[serde(default = "default_1000")]
    pub repeat_interval_ms: u64,
    pub start_delay_ms: u64,
    #[serde(default)]
    pub stop_duration_min: u32,
    #[serde(default)]
    pub stop_time_str: String,
    pub gui_lock_ms: u64,
}

fn default_click_type() -> String {
    "single".into()
}
fn default_position_mode() -> String {
    "cursor".into()
}
fn default_repeat_mode() -> String {
    "unlimited".into()
}
fn default_100() -> i32 {
    100
}
fn default_500() -> u64 {
    500
}
fn default_1000() -> u64 {
    1000
}

impl Default for EngineSettings {
    fn default() -> Self {
        EngineSettings {
            target_cps: 29.0,
            jitter_percent: 7.5,
            click_limit: 0,
            jitter_radius_px: 3,
            button: "left".into(),
            click_type: "single".into(),
            position_mode: "cursor".into(),
            fixed_x: 100,
            fixed_y: 100,
            repeat_mode: "unlimited".into(),
            repeat_count: 0,
            hold_duration_ms: 500,
            hold_interval_ms: 1000,
            repeat_interval_ms: 1000,
            start_delay_ms: 0,
            stop_duration_min: 0,
            stop_time_str: String::new(),
            gui_lock_ms: 1500,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotkeySettings {
    #[serde(default = "default_toggle")]
    pub toggle: String,
    #[serde(default = "default_mode_switch")]
    pub mode_switch: String,
    #[serde(default = "default_emergency_stop")]
    pub emergency_stop: String,
    #[serde(default = "default_speed_up")]
    pub speed_up: String,
    #[serde(default = "default_slow_down")]
    pub slow_down: String,
    #[serde(default = "default_capture_pos")]
    pub capture_pos: String,
    /// Enables the global macro recording hotkey.
    #[serde(default = "default_record_toggle")]
    pub record_toggle: bool,
    #[serde(default = "default_record_hotkey")]
    pub record_hotkey: String,
}

fn default_toggle() -> String {
    "R / K".into()
}
fn default_mode_switch() -> String {
    "Ctrl+Alt+M".into()
}
fn default_emergency_stop() -> String {
    "Escape".into()
}
fn default_speed_up() -> String {
    "Ctrl+=".into()
}
fn default_slow_down() -> String {
    "Ctrl+-".into()
}
fn default_capture_pos() -> String {
    "Ctrl+P".into()
}
fn default_record_toggle() -> bool {
    true
}
fn default_record_hotkey() -> String {
    "Ctrl+Shift+R".into()
}

impl Default for HotkeySettings {
    fn default() -> Self {
        HotkeySettings {
            toggle: "R / K".into(),
            mode_switch: "Ctrl+Alt+M".into(),
            emergency_stop: "Escape".into(),
            speed_up: "Ctrl+=".into(),
            slow_down: "Ctrl+-".into(),
            capture_pos: "Ctrl+P".into(),
            record_toggle: true,
            record_hotkey: "Ctrl+Shift+R".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiSettings {
    pub always_on_top: bool,
    pub mode: String,
    pub sound_feedback: bool,
    pub visual_ripple: bool,
    #[serde(default)]
    pub start_minimized: bool,
    #[serde(default)]
    pub autostart: bool,
    #[serde(default = "default_true")]
    pub minimize_to_tray: bool,
    #[serde(default = "default_true")]
    pub show_notifications: bool,
    #[serde(default)]
    pub pause_on_focus_loss: bool,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_accent")]
    pub accent_color: String,
}

fn default_true() -> bool {
    true
}
fn default_theme() -> String {
    "cyberpunk".into()
}
fn default_accent() -> String {
    "#06b6d4".into()
}

impl Default for UiSettings {
    fn default() -> Self {
        UiSettings {
            always_on_top: false,
            mode: "floating_hud".into(),
            sound_feedback: false,
            visual_ripple: true,
            start_minimized: false,
            autostart: false,
            minimize_to_tray: true,
            show_notifications: true,
            pause_on_focus_loss: false,
            theme: "cyberpunk".into(),
            accent_color: "#06b6d4".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetItem {
    pub id: String,
    pub name: String,
    pub description: String,
    pub icon: String,
    pub target_cps: f64,
    pub jitter_percent: f64,
    pub click_limit: u32,
    #[serde(default = "default_button")]
    pub button: String,
    #[serde(default = "default_click_type")]
    pub click_type: String,
    #[serde(default = "default_position_mode")]
    pub position_mode: String,
    #[serde(default = "default_100")]
    pub fixed_x: i32,
    #[serde(default = "default_100")]
    pub fixed_y: i32,
    #[serde(default = "default_500")]
    pub hold_duration_ms: u64,
    #[serde(default = "default_1000")]
    pub hold_interval_ms: u64,
    #[serde(default = "default_3")]
    pub jitter_radius_px: u32,
    #[serde(default = "default_repeat_mode")]
    pub repeat_mode: String,
    #[serde(default)]
    pub repeat_count: u32,
    #[serde(default = "default_1000")]
    pub repeat_interval_ms: u64,
    #[serde(default)]
    pub start_delay_ms: u64,
    #[serde(default)]
    pub stop_duration_min: u32,
    #[serde(default)]
    pub stop_time_str: String,
    #[serde(default)]
    pub is_default: bool,
}

fn default_button() -> String {
    "left".into()
}

fn default_3() -> u32 {
    3
}

fn default_presets() -> Vec<PresetItem> {
    vec![
        PresetItem {
            id: "fast_cps".into(),
            name: "Fast CPS".into(),
            description: "29 CPS | 7.5% Jitter | Single Left".into(),
            icon: "⚡".into(),
            target_cps: 29.0,
            jitter_percent: 7.5,
            click_limit: 0,
            click_type: "single".into(),
            button: "left".into(),
            position_mode: "cursor".into(),
            fixed_x: 100,
            fixed_y: 100,
            hold_duration_ms: 500,
            hold_interval_ms: 1000,
            jitter_radius_px: 3,
            repeat_mode: "unlimited".into(),
            repeat_count: 0,
            repeat_interval_ms: 1000,
            start_delay_ms: 0,
            stop_duration_min: 0,
            stop_time_str: String::new(),
            is_default: true,
        },
        PresetItem {
            id: "gaming_boost".into(),
            name: "Gaming Boost".into(),
            description: "15 CPS | 5.0% Jitter | Single Left".into(),
            icon: "🎮".into(),
            target_cps: 15.0,
            jitter_percent: 5.0,
            click_limit: 0,
            click_type: "single".into(),
            button: "left".into(),
            position_mode: "cursor".into(),
            fixed_x: 100,
            fixed_y: 100,
            hold_duration_ms: 500,
            hold_interval_ms: 1000,
            jitter_radius_px: 3,
            repeat_mode: "unlimited".into(),
            repeat_count: 0,
            repeat_interval_ms: 1000,
            start_delay_ms: 0,
            stop_duration_min: 0,
            stop_time_str: String::new(),
            is_default: true,
        },
        PresetItem {
            id: "human_emulation".into(),
            name: "Human Emulation".into(),
            description: "8 CPS | 15.0% Jitter | Single Left".into(),
            icon: "👤".into(),
            target_cps: 8.0,
            jitter_percent: 15.0,
            click_limit: 0,
            click_type: "single".into(),
            button: "left".into(),
            position_mode: "cursor".into(),
            fixed_x: 100,
            fixed_y: 100,
            hold_duration_ms: 500,
            hold_interval_ms: 1000,
            jitter_radius_px: 3,
            repeat_mode: "unlimited".into(),
            repeat_count: 0,
            repeat_interval_ms: 1000,
            start_delay_ms: 0,
            stop_duration_min: 0,
            stop_time_str: String::new(),
            is_default: true,
        },
        PresetItem {
            id: "afk_farm".into(),
            name: "AFK Farm".into(),
            description: "2 CPS | 2.0% Jitter | Single Left".into(),
            icon: "🌾".into(),
            target_cps: 2.0,
            jitter_percent: 2.0,
            click_limit: 0,
            click_type: "single".into(),
            button: "left".into(),
            position_mode: "cursor".into(),
            fixed_x: 100,
            fixed_y: 100,
            hold_duration_ms: 500,
            hold_interval_ms: 1000,
            jitter_radius_px: 3,
            repeat_mode: "unlimited".into(),
            repeat_count: 0,
            repeat_interval_ms: 1000,
            start_delay_ms: 0,
            stop_duration_min: 0,
            stop_time_str: String::new(),
            is_default: true,
        },
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub schema_version: u32,
    pub first_run: bool,
    pub active_mode: String, // "autoclicker" or "work"
    pub engine: EngineSettings,
    pub hotkeys: HotkeySettings,
    pub ui: UiSettings,
    #[serde(default = "default_presets")]
    pub presets: Vec<PresetItem>,
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            schema_version: CONFIG_SCHEMA_VERSION,
            first_run: true,
            active_mode: "autoclicker".into(),
            engine: EngineSettings::default(),
            hotkeys: HotkeySettings::default(),
            ui: UiSettings::default(),
            presets: default_presets(),
        }
    }
}

pub struct ConfigManager {
    config_path: PathBuf,
}

impl ConfigManager {
    pub fn new() -> Self {
        let proj_dirs = ProjectDirs::from("com", "nanoclick", "NanoClick");
        let config_dir = match proj_dirs {
            Some(dirs) => dirs.config_dir().to_path_buf(),
            None => PathBuf::from(".nanoclick"),
        };

        if !config_dir.exists() {
            let _ = fs::create_dir_all(&config_dir);
        }

        let config_path = config_dir.join("config.json");
        ConfigManager { config_path }
    }

    pub fn get_config_path(&self) -> String {
        self.config_path.to_string_lossy().to_string()
    }

    pub fn load(&self) -> AppConfig {
        if self.config_path.exists() {
            if let Ok(content) = fs::read_to_string(&self.config_path) {
                if let Ok(value) = serde_json::from_str::<Value>(&content) {
                    if let Ok((config, migrated)) = migrate_config(value) {
                        if migrated {
                            let _ = self.save(&config);
                        }
                        return config;
                    }
                }
            }
        }

        let default_config = AppConfig::default();
        let _ = self.save(&default_config);
        default_config
    }

    pub fn save(&self, config: &AppConfig) -> Result<(), String> {
        if let Some(parent) = self.config_path.parent() {
            if !parent.exists() {
                let _ = fs::create_dir_all(parent);
            }
        }

        let json = serde_json::to_string_pretty(config)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;
        fs::write(&self.config_path, json).map_err(|e| {
            format!(
                "Failed to write config file to {:?}: {}",
                self.config_path, e
            )
        })?;
        Ok(())
    }
}

/// Normalize configs written by pre-versioned NanoClick builds and add the
/// current schema marker. Unknown fields remain harmlessly ignored by serde.
fn migrate_config(mut value: Value) -> Result<(AppConfig, bool), String> {
    let object = value
        .as_object_mut()
        .ok_or_else(|| "Config root must be a JSON object".to_string())?;
    let mut migrated = false;
    let version = object
        .get("schema_version")
        .and_then(Value::as_u64)
        .unwrap_or(0) as u32;

    if version == 0 {
        migrated = true;
        if let Some(hotkeys) = object.get_mut("hotkeys").and_then(Value::as_object_mut) {
            copy_legacy_key(hotkeys, "start_stop", "toggle");
            copy_legacy_key(hotkeys, "mode", "mode_switch");
            copy_legacy_key(hotkeys, "recording", "record_hotkey");
        }
        if let Some(engine) = object.get_mut("engine").and_then(Value::as_object_mut) {
            copy_legacy_key(engine, "cps", "target_cps");
        }
    }

    if version < CONFIG_SCHEMA_VERSION {
        object.insert("schema_version".into(), Value::from(CONFIG_SCHEMA_VERSION));
        migrated = true;
    }

    let config = serde_json::from_value(Value::Object(object.clone()))
        .map_err(|e| format!("Failed to parse config: {}", e))?;
    Ok((config, migrated))
}

fn copy_legacy_key(object: &mut serde_json::Map<String, Value>, old: &str, new: &str) {
    if !object.contains_key(new) {
        if let Some(value) = object.get(old).cloned() {
            object.insert(new.into(), value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_config_is_migrated_and_versioned() {
        let mut value = serde_json::to_value(AppConfig::default()).unwrap();
        let object = value.as_object_mut().unwrap();
        object.remove("schema_version");
        let engine = object.get_mut("engine").unwrap().as_object_mut().unwrap();
        engine.remove("target_cps");
        engine.insert("cps".into(), Value::from(17.0));
        let hotkeys = object.get_mut("hotkeys").unwrap().as_object_mut().unwrap();
        hotkeys.remove("toggle");
        hotkeys.remove("mode_switch");
        hotkeys.remove("record_hotkey");
        hotkeys.insert("start_stop".into(), Value::from("F6"));
        hotkeys.insert("mode".into(), Value::from("Ctrl+M"));
        hotkeys.insert("recording".into(), Value::from("Ctrl+R"));
        let (config, migrated) = migrate_config(value).expect("legacy config should migrate");
        assert!(migrated);
        assert_eq!(config.schema_version, CONFIG_SCHEMA_VERSION);
        assert_eq!(config.engine.target_cps, 17.0);
        assert_eq!(config.hotkeys.toggle, "F6");
        assert_eq!(config.hotkeys.mode_switch, "Ctrl+M");
        assert_eq!(config.hotkeys.record_hotkey, "Ctrl+R");
    }

    #[test]
    fn current_config_does_not_migrate() {
        let value = serde_json::to_value(AppConfig::default()).unwrap();
        let (_, migrated) = migrate_config(value).expect("current config should parse");
        assert!(!migrated);
    }
}
