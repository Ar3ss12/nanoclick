pub use crate::config_manager::AppConfig;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Config {
    pub cps: f64,
    pub random_percent: f64,
    pub click_limit: u32,
    pub button: String,
    pub click_type: String,
    pub position_mode: String,
    pub fixed_x: i32,
    pub fixed_y: i32,
    pub repeat_mode: String,
    pub repeat_count: u32,
    pub hold_duration_ms: u64,
    pub hold_interval_ms: u64,
    pub repeat_interval_ms: u64,
    pub jitter_radius_px: u32,
    pub hotkey_toggle: String,
    pub hotkey_mode_switch: String,
    pub hotkey_emergency_stop: String,
    pub hotkey_speed_up: String,
    pub hotkey_slow_down: String,
    pub hotkey_capture_pos: String,
    pub hotkey_record_toggle: bool,
    pub hotkey_record: String,
    pub start_delay_ms: u64,
    pub stop_duration_ms: u64,
    pub stop_time_epoch_sec: i64,
    pub gui_lock_ms: u64,
    pub hotkey_debounce_ms: u32,
    pub active_mode: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            cps: 29.0,
            random_percent: 7.5,
            click_limit: 0,
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
            jitter_radius_px: 3,
            hotkey_toggle: "R / K".into(),
            hotkey_mode_switch: "Ctrl+Alt+M".into(),
            hotkey_emergency_stop: "Escape".into(),
            hotkey_speed_up: "Ctrl+=".into(),
            hotkey_slow_down: "Ctrl+-".into(),
            hotkey_capture_pos: "Ctrl+P".into(),
            hotkey_record_toggle: true,
            hotkey_record: "Ctrl+Shift+R".into(),
            start_delay_ms: 0,
            stop_duration_ms: 0,
            stop_time_epoch_sec: 0,
            gui_lock_ms: 1500,
            hotkey_debounce_ms: 80,
            active_mode: "autoclicker".into(),
        }
    }
}

impl From<AppConfig> for Config {
    fn from(app_cfg: AppConfig) -> Self {
        Config {
            cps: app_cfg.engine.target_cps,
            random_percent: app_cfg.engine.jitter_percent,
            click_limit: app_cfg.engine.click_limit,
            button: app_cfg.engine.button,
            click_type: app_cfg.engine.click_type,
            position_mode: app_cfg.engine.position_mode,
            fixed_x: app_cfg.engine.fixed_x,
            fixed_y: app_cfg.engine.fixed_y,
            repeat_mode: app_cfg.engine.repeat_mode,
            repeat_count: app_cfg.engine.repeat_count,
            hold_duration_ms: app_cfg.engine.hold_duration_ms,
            hold_interval_ms: app_cfg.engine.hold_interval_ms,
            repeat_interval_ms: app_cfg.engine.repeat_interval_ms,
            jitter_radius_px: app_cfg.engine.jitter_radius_px,
            hotkey_toggle: app_cfg.hotkeys.toggle,
            hotkey_mode_switch: app_cfg.hotkeys.mode_switch,
            hotkey_emergency_stop: app_cfg.hotkeys.emergency_stop,
            hotkey_speed_up: app_cfg.hotkeys.speed_up,
            hotkey_slow_down: app_cfg.hotkeys.slow_down,
            hotkey_capture_pos: app_cfg.hotkeys.capture_pos,
            hotkey_record_toggle: app_cfg.hotkeys.record_toggle,
            hotkey_record: app_cfg.hotkeys.record_hotkey,
            start_delay_ms: app_cfg.engine.start_delay_ms,
            stop_duration_ms: (app_cfg.engine.stop_duration_min as u64).saturating_mul(60_000),
            stop_time_epoch_sec: parse_stop_time_str(&app_cfg.engine.stop_time_str),
            gui_lock_ms: app_cfg.engine.gui_lock_ms,
            hotkey_debounce_ms: app_cfg.engine.hotkey_debounce_ms,
            active_mode: app_cfg.active_mode,
        }
    }
}

/// Parse "HH:MM" 24-hour clock into a Unix-epoch seconds value for *today*.
/// Returns 0 when the input is empty or malformed (treated as "no stop time").
pub fn parse_stop_time_str(s: &str) -> i64 {
    let s = s.trim();
    if s.is_empty() {
        return 0;
    }
    let mut parts = s.split(':');
    let h: i64 = match parts.next().and_then(|p| p.trim().parse().ok()) {
        Some(v) => v,
        None => return 0,
    };
    let m: i64 = match parts.next().and_then(|p| p.trim().parse().ok()) {
        Some(v) => v,
        None => return 0,
    };
    if !(0..=23).contains(&h) || !(0..=59).contains(&m) {
        return 0;
    }
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let now_h = (now / 3600) % 24;
    let now_m = (now / 60) % 60;
    let now_s = now % 60;
    let today_midnight = now - (now_h * 3600 + now_m * 60 + now_s);
    let target = today_midnight + h * 3600 + m * 60;
    if target < now {
        0
    } else {
        target
    }
}
