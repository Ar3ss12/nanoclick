use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
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
            target_cps: 10.0,
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
    /// Smart key memory time-to-live in milliseconds (100–5000ms).
    #[serde(default = "default_key_ttl_ms")]
    pub key_ttl_ms: u64,
    /// Whether smart key memory is active during hotkey recording.
    #[serde(default = "default_smart_record")]
    pub smart_record: bool,
}

fn default_key_ttl_ms() -> u64 {
    500
}
fn default_smart_record() -> bool {
    true
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
            key_ttl_ms: default_key_ttl_ms(),
            smart_record: default_smart_record(),
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
    /// Remember main window position across restarts (BEHAVIOR card).
    /// Default ON: desktop QoL, zero risk (sanitized against virtual screen).
    #[serde(default = "default_true")]
    pub remember_window_position: bool,
    /// Last known main-window position (physical px). Written on every
    /// `save_app_config` from `outer_position()` when remember is ON;
    /// restored at boot only if inside the current virtual screen.
    #[serde(default)]
    pub window_x: Option<i32>,
    #[serde(default)]
    pub window_y: Option<i32>,
    /// Typing Guard: while the user is typing, a toggle hotkey pressed within
    /// this window is ignored — a single-key hotkey sitting inside a word
    /// ("go rush B" → the `r`) must not switch the clicker on. Gameplay keys
    /// (WASD, hotbar digits, arrows, modifiers, ...) never arm it.
    /// Value = lockout window in ms (0 = disabled). UI checkbox maps to 0/600.
    #[serde(default)]
    pub typing_pause_ms: u32,
    /// App-scope filter: "everywhere" (default) | "whitelist" | "blacklist".
    #[serde(default = "default_app_filter_mode")]
    pub app_filter_mode: String,
    /// Process image names (lowercase, e.g. "discord.exe") for the filter.
    #[serde(default)]
    pub app_filter_list: Vec<String>,
    #[serde(default)]
    pub always_run_as_admin: bool,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_accent")]
    pub accent_color: String,
    #[serde(default = "default_language")]
    pub language: String,
}

fn default_true() -> bool {
    true
}
fn default_app_filter_mode() -> String {
    "everywhere".into()
}
fn default_theme() -> String {
    "cyberpunk".into()
}
fn default_accent() -> String {
    "#06b6d4".into()
}
fn default_language() -> String {
    "ua".into()
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
            remember_window_position: true,
            window_x: None,
            window_y: None,
            typing_pause_ms: 600,
            app_filter_mode: default_app_filter_mode(),
            app_filter_list: Vec::new(),
            always_run_as_admin: false,
            theme: "cyberpunk".into(),
            accent_color: "#06b6d4".into(),
            language: default_language(),
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

    /// Raw path handle for the observer watcher registration.
    pub fn config_path(&self) -> PathBuf {
        self.config_path.clone()
    }

    /// Dedicated file path for statistics persistence across reinstalls and resets.
    pub fn stats_path(&self) -> PathBuf {
        self.config_path.with_file_name("stats.json")
    }

    pub fn load(&self) -> AppConfig {
        self.load_with_action().0
    }

    /// Same as `load()` but also returns the boot-time healing action so the
    /// UI can show the mandatory-OK modal (repaired / recovered / migrated).
    pub fn load_with_action(&self) -> (AppConfig, crate::defaults::SelfHealingAction) {
        let (mut app_config, action) = crate::defaults::ensure_config_file(&self.config_path);

        // Fallback recovery: if total_clicks is 0, check if a dedicated stats.json
        // backup exists (e.g. fresh install/reinstall or config reset), and restore it.
        // NEVER touches stats.json quarantine: stats is append-only telemetry,
        // the LKG/quarantine scheme covers config.json only.
        if app_config.stats.total_clicks == 0 {
            let stats_file = self.stats_path();
            if stats_file.exists() {
                if let Ok(content) = fs::read_to_string(&stats_file) {
                    if let Ok(recovered_stats) = serde_json::from_str::<StatsConfig>(&content) {
                        if recovered_stats.total_clicks > 0 || recovered_stats.total_sessions > 0 {
                            app_config.stats = recovered_stats;
                        }
                    }
                }
            }
        }

        (app_config, action)
    }

    pub fn reset_to_defaults(&self) -> Result<AppConfig, String> {
        let mut default_cfg = crate::defaults::reset_to_defaults(&self.config_path)?;
        let stats_file = self.stats_path();
        if stats_file.exists() {
            if let Ok(content) = fs::read_to_string(&stats_file) {
                if let Ok(recovered_stats) = serde_json::from_str::<StatsConfig>(&content) {
                    if recovered_stats.total_clicks > 0 || recovered_stats.total_sessions > 0 {
                        default_cfg.stats = recovered_stats;
                        let _ = self.save(&default_cfg);
                    }
                }
            }
        }
        Ok(default_cfg)
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

        // Also persist a dedicated stats.json backup in the config directory.
        // This ensures statistics survive even if config.json is reset,
        // re-created, or the application is reinstalled.
        if let Ok(stats_json) = serde_json::to_string_pretty(&config.stats) {
            let stats_file = self.stats_path();
            let stats_tmp = stats_file.with_extension("json.tmp");
            if fs::write(&stats_tmp, stats_json).is_ok() {
                let _ = fs::rename(&stats_tmp, &stats_file);
            }
        }

        // Our own write is proven good: refresh LKG snapshot so a later
        // corruption restores THIS state. The observer's grace window
        // (mark below happens in save callers) swallows the echo.
        crate::defaults::refresh_last_good_snapshot(&self.config_path, config);

        Ok(())
    }
}

#[allow(unused_imports)]
pub use crate::defaults::migrate_config;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

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

    #[test]
    fn hotkey_settings_key_ttl_and_smart_record_serialization() {
        let mut config = AppConfig::default();
        config.hotkeys.key_ttl_ms = 750;
        config.hotkeys.smart_record = false;

        let json = serde_json::to_string(&config).unwrap();
        let restored: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.hotkeys.key_ttl_ms, 750);
        assert!(!restored.hotkeys.smart_record);
    }

    #[test]
    fn stats_backup_and_fallback_recovery() {
        let temp_dir = std::env::temp_dir().join(format!("nanoclick_test_stats_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let _ = fs::create_dir_all(&temp_dir);
        let config_file = temp_dir.join("config.json");
        let cm = ConfigManager { config_path: config_file.clone() };

        let mut cfg = AppConfig::default();
        cfg.stats.total_clicks = 8888;
        cfg.stats.total_sessions = 15;
        cm.save(&cfg).expect("save should succeed");

        // stats.json must exist next to config.json
        assert!(cm.stats_path().exists(), "stats.json should be created on save");

        // Simulate config reset (fresh install/wipe of config.json with stats = 0)
        let mut wiped_config = AppConfig::default();
        wiped_config.stats.total_clicks = 0;
        let _ = fs::write(&config_file, serde_json::to_string_pretty(&wiped_config).unwrap());

        // load() should detect total_clicks == 0 and restore from stats.json
        let loaded = cm.load();
        assert_eq!(loaded.stats.total_clicks, 8888);
        assert_eq!(loaded.stats.total_sessions, 15);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn always_run_as_admin_serialization_roundtrip() {
        let mut config = AppConfig::default();
        assert!(!config.ui.always_run_as_admin);
        config.ui.always_run_as_admin = true;

        let json = serde_json::to_string(&config).unwrap();
        let restored: AppConfig = serde_json::from_str(&json).unwrap();
        assert!(restored.ui.always_run_as_admin);
    }

    #[test]
    fn language_serialization_roundtrip() {
        let mut config = AppConfig::default();
        assert_eq!(config.ui.language, "ua");
        config.ui.language = "en".into();

        let json = serde_json::to_string(&config).unwrap();
        let restored: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.ui.language, "en");
    }
}

fn default_hotkey_debounce_ms() -> u32 {
    80
}
