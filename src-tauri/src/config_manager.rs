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
    #[serde(default = "default_hotkey_debounce_ms")]
    pub hotkey_debounce_ms: u32,
    /// Optional multi-point click sequence. When non-empty the engine
    /// visits each point in order with its per-point delay. Per-run
    /// override lives on each `PresetItem.points` (UI populates this
    /// when a preset with `points` is selected).
    #[serde(default)]
    pub sequence_points: Vec<SequencePoint>,
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
            jitter_percent: 5.0,
            click_limit: 0,
            jitter_radius_px: 0,
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
            hotkey_debounce_ms: 80,
            sequence_points: Vec::new(),
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
    /// Per-slot preset hotkeys: index 0 = preset slot 1 ... 8 = slot 9.
    /// Empty string disables that slot.
    #[serde(default)]
    pub preset_hotkeys: Vec<String>,
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
            preset_hotkeys: Vec::new(),
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
    pub show_hud: bool,
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
            show_hud: false,
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

/// Auto-switch rule: when the foreground window title contains
/// `title_contains` (case-insensitive), apply preset `preset_id`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ImageTrigger {
    /// Screen X coordinate of the pixel to watch.
    pub x: i32,
    /// Screen Y coordinate of the pixel to watch.
    pub y: i32,
    /// Target RGBA color (0xRRGGBBAA). The click loop polls this point
    /// while running and stops when the pixel matches.
    pub color_rgba: u32,
    /// Per-channel tolerance (0..=255). The comparison accepts a pixel
    /// when every channel is within this distance of the target.
    #[serde(default = "default_image_trigger_tolerance")]
    pub tolerance: u32,
    /// Sampling interval in milliseconds (50..=2000). Faster = more
    /// responsive but more CPU on the GDI path.
    #[serde(default = "default_image_trigger_poll_ms")]
    pub poll_ms: u32,
    /// Optional human-readable label shown in the UI.
    #[serde(default)]
    pub label: String,
}

fn default_image_trigger_tolerance() -> u32 {
    12
}
fn default_image_trigger_poll_ms() -> u32 {
    120
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppProfile {
    pub title_contains: String,
    pub preset_id: String,
    #[serde(default)]
    pub enabled: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SequencePoint {
    pub x: i32,
    pub y: i32,
    /// Delay in milliseconds AFTER clicking this point before moving to
    /// the next one (or repeating the cycle). 0 means "no extra wait".
    #[serde(default)]
    pub delay_ms: u32,
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
    /// Optional multi-point sequence. When this is non-empty the
    /// scheduler visits each point in order with the per-point delay.
    /// When empty the engine falls back to the legacy single-point
    /// behaviour (fixed_x/fixed_y + jitter_radius).
    #[serde(default)]
    pub points: Vec<SequencePoint>,
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
            jitter_radius_px: 0,
            repeat_mode: "unlimited".into(),
            repeat_count: 0,
            repeat_interval_ms: 1000,
            start_delay_ms: 0,
            stop_duration_min: 0,
            stop_time_str: String::new(),
            is_default: true,
            points: Vec::new(),
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
            jitter_radius_px: 0,
            repeat_mode: "unlimited".into(),
            repeat_count: 0,
            repeat_interval_ms: 1000,
            start_delay_ms: 0,
            stop_duration_min: 0,
            stop_time_str: String::new(),
            is_default: true,
            points: Vec::new(),
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
            jitter_radius_px: 0,
            repeat_mode: "unlimited".into(),
            repeat_count: 0,
            repeat_interval_ms: 1000,
            start_delay_ms: 0,
            stop_duration_min: 0,
            stop_time_str: String::new(),
            is_default: true,
            points: Vec::new(),
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
            jitter_radius_px: 0,
            repeat_mode: "unlimited".into(),
            repeat_count: 0,
            repeat_interval_ms: 1000,
            start_delay_ms: 0,
            stop_duration_min: 0,
            stop_time_str: String::new(),
            is_default: true,
            points: Vec::new(),
        },
    ]
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatHistoryPoint {
    #[serde(default)]
    pub timestamp: u64,
    #[serde(default)]
    pub clicks: u64,
    #[serde(default)]
    pub active_ms: u64,
    #[serde(default)]
    pub avg_cps: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsConfig {
    #[serde(default)]
    pub total_clicks: u64,
    #[serde(default)]
    pub total_active_ms: u64,
    #[serde(default)]
    pub total_sessions: u64,
    #[serde(default)]
    pub presets_applied: u64,
    #[serde(default)]
    pub max_cps: f64,
    #[serde(default)]
    pub history: Vec<StatHistoryPoint>,
}

impl Default for StatsConfig {
    fn default() -> Self {
        StatsConfig {
            total_clicks: 0,
            total_active_ms: 0,
            total_sessions: 0,
            presets_applied: 0,
            max_cps: 0.0,
            history: Vec::new(),
        }
    }
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
    /// Window-title -> preset auto-switch rules (checked every 500 ms).
    #[serde(default)]
    pub app_profiles: Vec<AppProfile>,
    /// Optional pixel-watch trigger that stops the clicker when a screen
    /// pixel matches a target color. Single-slot for v1; multi-slot can
    /// be added later if needed.
    #[serde(default)]
    pub image_trigger: Option<ImageTrigger>,
    /// Persistent statistics tracking session and all-time metrics.
    #[serde(default)]
    pub stats: StatsConfig,
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
            app_profiles: Vec::new(),
            image_trigger: None,
            stats: StatsConfig::default(),
        }
    }
}

pub struct ConfigManager {
    config_path: PathBuf,
}

impl ConfigManager {
    pub fn new() -> Self {
        let portable = std::env::args().any(|a| a == "--portable")
            || std::path::Path::new("nanoclick.ini").exists();
        Self::with_portable(portable)
    }

    pub fn with_portable(portable: bool) -> Self {
        let config_dir = if portable {
            // Portable mode: keep config next to the executable so the
            // whole app can be moved/copied to a USB stick.
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                .map(|p| p.join("nanoclick_data"))
                .unwrap_or_else(|| PathBuf::from("./nanoclick_data"))
        } else {
            match ProjectDirs::from("com", "nanoclick", "NanoClick") {
                Some(dirs) => dirs.config_dir().to_path_buf(),
                None => PathBuf::from(".nanoclick"),
            }
        };

        if !config_dir.exists() {
            let _ = fs::create_dir_all(&config_dir);
        }

        let config_path = config_dir.join("config.json");
        ConfigManager { config_path }
    }

    /// True when running in portable mode (--portable flag or nanoclick.ini present).
    pub fn is_portable() -> bool {
        std::env::args().any(|a| a == "--portable")
            || std::path::Path::new("nanoclick.ini").exists()
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

        // Atomic write: serialize to a temp file first, then rename over the
        // real config. A crash/BSOD mid-write can no longer leave a truncated
        // config.json behind - rename is atomic on NTFS.
        let tmp_path = self.config_path.with_extension("json.tmp");
        fs::write(&tmp_path, json)
            .map_err(|e| format!("Failed to write temp config file to {:?}: {}", tmp_path, e))?;
        fs::rename(&tmp_path, &self.config_path).map_err(|e| {
            format!(
                "Failed to replace config file {:?}: {}",
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

    #[test]
    fn stats_config_default_initialization() {
        let stats = StatsConfig::default();
        assert_eq!(stats.total_clicks, 0);
        assert_eq!(stats.total_sessions, 0);
        assert_eq!(stats.total_active_ms, 0);
        assert!(stats.history.is_empty());
    }

    #[test]
    fn stats_config_serialization_roundtrip() {
        let mut stats = StatsConfig::default();
        stats.total_clicks = 12345;
        stats.total_sessions = 42;
        stats.total_active_ms = 3600000;
        stats.max_cps = 55.5;
        stats.history.push(StatHistoryPoint {
            timestamp: 100000,
            clicks: 250,
            active_ms: 10000,
            avg_cps: 25.0,
        });
        let json = serde_json::to_string(&stats).unwrap();
        let restored: StatsConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.total_clicks, 12345);
        assert_eq!(restored.total_sessions, 42);
        assert_eq!(restored.max_cps, 55.5);
        assert_eq!(restored.history.len(), 1);
        assert_eq!(restored.history[0].clicks, 250);
    }

    #[test]
    fn stats_config_preserves_values_across_config_save() {
        let mut config = AppConfig::default();
        config.stats.total_clicks = 9999;
        let json = serde_json::to_string(&config).unwrap();
        let restored: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.stats.total_clicks, 9999);
    }

    #[test]
    fn legacy_config_migration_retains_stats_defaults() {
        let mut value = serde_json::to_value(AppConfig::default()).unwrap();
        value.as_object_mut().unwrap().remove("stats");
        let (config, _) = migrate_config(value).unwrap();
        assert_eq!(config.stats.total_clicks, 0);
    }
}

fn default_hotkey_debounce_ms() -> u32 {
    80
}
