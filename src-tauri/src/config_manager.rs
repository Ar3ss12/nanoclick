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
    /// Rare biological hesitation: probability (0.0 = off, default 0.02)
    /// that one interval stretches to 2-3x base. Stateless, Zero-Jitter safe.
    #[serde(default = "default_outlier_prob")]
    pub outlier_prob: f64,
    /// Biometric technique hint: "auto" | "single" | "butterfly" | "drag".
    /// Phase A: UI badge + persistence only, engine stays Single.
    #[serde(default = "default_technique")]
    pub technique: String,
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
    /// Auto-stop after this many milliseconds of RUNNING (the scheduler
    /// snapshots it at run start). Stored in ms so the unit picker in the UI
    /// (ms / sec / min / hour) can offer fractional values without a lossy
    /// round-trip through whole minutes. Hard ceiling:
    /// `scheduler::MAX_STOP_DURATION_MS` (999 h).
    #[serde(default)]
    pub stop_duration_ms: u64,
    /// The unit the UI used to display/enter `stop_duration_ms`:
    /// "ms" | "sec" | "min" | "hour". Persisted so "1.5 sec" does not come back
    /// as "1.5 min" after a restart. Display-only — the engine never reads it.
    #[serde(default = "default_stop_duration_unit")]
    pub stop_duration_unit: String,
    /// LEGACY (pre-1.3): whole minutes, superseded by `stop_duration_ms`.
    /// Kept ONLY as a migration input read by `Config::from` / the page, which
    /// convert it to ms; the UI writes 0 here on every save. Removing the field
    /// would make old configs fail deserialization instead of migrating.
    #[serde(default)]
    pub stop_duration_min: u32,
    #[serde(default)]
    pub stop_time_str: String,
    /// WHICH auto-stop trigger is armed: "none" | "duration" | "wallclock".
    ///
    /// Both values stay in the config, so switching the active trigger restores
    /// what the user typed in the other field. Without this discriminator the
    /// backend cannot honour "only one runs at a time": two non-zero values are
    /// ambiguous and the loop would act on whichever deadline came first.
    #[serde(default = "default_stop_mode")]
    pub stop_mode: String,
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
fn default_stop_duration_unit() -> String {
    "sec".into()
}
fn default_stop_mode() -> String {
    "none".into()
}
fn default_outlier_prob() -> f64 {
    0.02
}
fn default_technique() -> String {
    "auto".into()
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
            outlier_prob: default_outlier_prob(),
            technique: default_technique(),
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
            stop_duration_ms: 0,
            stop_duration_unit: default_stop_duration_unit(),
            stop_duration_min: 0,
            stop_time_str: String::new(),
            stop_mode: default_stop_mode(),
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
    pub visual_ripple: bool,
    #[serde(default)]
    pub show_hud: bool,
    #[serde(default)]
    pub start_minimized: bool,
    #[serde(default)]
    pub autostart: bool,
    #[serde(default = "default_true")]
    pub minimize_to_tray: bool,
    /// Deep sleep in the tray (opt-in, default OFF): instead of merely hiding the
    /// window, DESTROY the main WebView while the app lives in the tray.
    /// MEASURED with `scripts/measure-ram.ps1` (private working set = the Task
    /// Manager metric, Windows 10): the tree with the interface alive weighs
    /// ~117–123 MB (host `nanoclick.exe` ~5 MB + six `msedgewebview2.exe` helpers);
    /// deep sleep leaves **5.3 MB** (and 0% CPU) — the host alone, helpers 6 → 0 (−96 %).
    /// Hiding the window alone changes nothing (122.8 MB, same processes). The window is
    /// rebuilt on the next tray click and shown only after the fresh page reports
    /// boot success (Zero-Flash). Unsaved macro-editor drafts live only in the
    /// page, so they are lost; the backend silently falls back to plain hiding
    /// while a run, a macro or a recording is in flight.
    #[serde(default)]
    pub deep_sleep_to_tray: bool,
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
    /// Last known main-window SIZE (physical px), same lifecycle as the position.
    /// Without it a deep-sleep rebuild would snap back to the geometry from
    /// `tauri.conf.json` and a resized window would lose its size.
    #[serde(default)]
    pub window_w: Option<i32>,
    #[serde(default)]
    pub window_h: Option<i32>,
    /// Reopen the window the way the app was closed (BEHAVIOR card, default ON).
    /// With this on, a restart replays the last state: the window comes back if it
    /// was open at exit, and the app starts in the tray if that is where it was
    /// living. An explicit `start_minimized` wins over it.
    #[serde(default = "default_true")]
    pub remember_last_window_state: bool,
    /// Was the main window visible when the app last went away? Owned by the
    /// BACKEND: stamped from the live window on every hide/show transition and at
    /// the start of a shutdown. A page never observes a tray hide, so its copy of
    /// this field is always stale — it must not be able to write it back.
    #[serde(default = "default_true")]
    pub window_was_visible: bool,
    /// Typing Guard: while the user is typing, a toggle hotkey pressed within
    /// this window is ignored — a single-key hotkey sitting inside a word
    /// ("go rush B" → the `r`) must not switch the clicker on. Gameplay keys
    /// (WASD, hotbar digits, arrows, modifiers, ...) never arm it.
    /// Value = lockout window in ms (0 = disabled). UI checkbox maps to 0/600.
    #[serde(default)]
    pub typing_pause_ms: u32,
    /// Keys treated as gameplay by the Typing Guard (layer B of
    /// `docs/KEY_POLICY.md`): they never arm the lockout. Empty = the hardcoded
    /// seed only. Keys the user bound to a hotkey are ignored automatically
    /// (layer A) no matter what this list says, and cannot be removed by editing
    /// it — that ordering is pinned by a unit test.
    #[serde(default)]
    pub typing_ignore_keys: Vec<String>,
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
            visual_ripple: true,
            show_hud: false,
            start_minimized: false,
            autostart: false,
            minimize_to_tray: true,
            deep_sleep_to_tray: false,
            show_notifications: true,
            pause_on_focus_loss: false,
            remember_window_position: true,
            window_x: None,
            window_y: None,
            window_w: None,
            window_h: None,
            remember_last_window_state: true,
            window_was_visible: true,
            typing_pause_ms: 600,
            typing_ignore_keys: Vec::new(),
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

/// Which groups of settings a preset captures when it is applied.
///
/// `engine` is ON by default so every preset written before the scope existed
/// keeps its historic meaning (a preset was always "the engine configuration").
/// The other three groups are opt-in: a preset that carries them is a full
/// working profile, and applying it may change global hotkeys, Smart Guard rules
/// or the look of the app — so the user has to ask for that explicitly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PresetScope {
    #[serde(default = "default_true")]
    pub engine: bool,
    #[serde(default)]
    pub guards: bool,
    #[serde(default)]
    pub hotkeys: bool,
    #[serde(default)]
    pub ui: bool,
}

impl Default for PresetScope {
    fn default() -> Self {
        PresetScope {
            engine: true,
            guards: false,
            hotkeys: false,
            ui: false,
        }
    }
}

fn default_preset_scope() -> PresetScope {
    PresetScope::default()
}

/// Smart-Guard / app-scope settings captured by a preset (`scope.guards`).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct GuardSnapshot {
    #[serde(default)]
    pub typing_pause_ms: u32,
    #[serde(default)]
    pub pause_on_focus_loss: bool,
    #[serde(default = "default_app_filter_mode")]
    pub app_filter_mode: String,
    #[serde(default)]
    pub app_filter_list: Vec<String>,
}

/// Global hotkey bindings captured by a preset (`scope.hotkeys`).
///
/// The nine `preset_hotkeys` slots are deliberately NOT part of this snapshot:
/// a slot maps to a preset id, so storing the slots inside a preset would be
/// recursive — a preset could rebind (or unbind) its own activation key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HotkeySnapshot {
    pub toggle: String,
    pub mode_switch: String,
    pub emergency_stop: String,
    pub speed_up: String,
    pub slow_down: String,
    pub capture_pos: String,
    pub record_toggle: bool,
    pub record_hotkey: String,
    /// Smart key memory TTL (100–5000 ms).
    pub key_ttl_ms: u64,
    pub smart_record: bool,
    /// Toggle response time (ms) — stored in `EngineSettings` in the config,
    /// but it belongs to this group because only the hotkey layer reads it.
    pub hotkey_debounce_ms: u32,
}

impl HotkeySnapshot {
    /// Capture every binding the hotkey layer owns except the preset slots.
    /// `hotkey_debounce_ms` lives in `EngineSettings`, hence the extra argument.
    pub fn from_settings(h: &HotkeySettings, hotkey_debounce_ms: u32) -> Self {
        HotkeySnapshot {
            toggle: h.toggle.clone(),
            mode_switch: h.mode_switch.clone(),
            emergency_stop: h.emergency_stop.clone(),
            speed_up: h.speed_up.clone(),
            slow_down: h.slow_down.clone(),
            capture_pos: h.capture_pos.clone(),
            record_toggle: h.record_toggle,
            record_hotkey: h.record_hotkey.clone(),
            key_ttl_ms: h.key_ttl_ms,
            smart_record: h.smart_record,
            hotkey_debounce_ms,
        }
    }
}

/// Appearance / feedback preferences captured by a preset (`scope.ui`).
///
/// Lifecycle settings (tray, deep sleep, autostart, window geometry, admin
/// elevation) are deliberately excluded: applying a preset must never make the
/// window vanish into the tray or switch on an autostart entry behind the
/// user's back.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PresetUiSnapshot {
    pub theme: String,
    pub accent_color: String,
    pub language: String,
    pub always_on_top: bool,
    pub visual_ripple: bool,
    pub show_hud: bool,
    pub show_notifications: bool,
}

impl PresetUiSnapshot {
    pub fn from_ui(u: &UiSettings) -> Self {
        PresetUiSnapshot {
            theme: u.theme.clone(),
            accent_color: u.accent_color.clone(),
            language: u.language.clone(),
            always_on_top: u.always_on_top,
            visual_ripple: u.visual_ripple,
            show_hud: u.show_hud,
            show_notifications: u.show_notifications,
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
    #[serde(default = "default_outlier_prob")]
    pub outlier_prob: f64,
    #[serde(default = "default_technique")]
    pub technique: String,
    #[serde(default = "default_repeat_mode")]
    pub repeat_mode: String,
    #[serde(default)]
    pub repeat_count: u32,
    #[serde(default = "default_1000")]
    pub repeat_interval_ms: u64,
    #[serde(default)]
    pub start_delay_ms: u64,
    /// Auto-stop this preset after this many ms of running (see
    /// `EngineSettings::stop_duration_ms`).
    #[serde(default)]
    pub stop_duration_ms: u64,
    /// LEGACY (pre-1.3) whole minutes of `PresetItem::stop_duration_ms`; kept as
    /// a migration input only.
    #[serde(default)]
    pub stop_duration_min: u32,
    #[serde(default)]
    pub stop_time_str: String,
    /// WHICH auto-stop trigger this preset arms: "none" | "duration" |
    /// "wallclock" (see `EngineSettings::stop_mode`). Both values are kept, so
    /// applying a preset restores what the other field held; this field says
    /// which of the two the preset actually arms.
    #[serde(default = "default_stop_mode")]
    pub stop_mode: String,
    #[serde(default)]
    pub is_default: bool,
    /// Optional multi-point sequence. When this is non-empty the
    /// scheduler visits each point in order with the per-point delay.
    /// When empty the engine falls back to the legacy single-point
    /// behaviour (fixed_x/fixed_y + jitter_radius).
    #[serde(default)]
    pub points: Vec<SequencePoint>,
    /// Global hotkey bound directly to this preset (e.g. "Mouse4", "F7", "Ctrl+Mouse5").
    #[serde(default)]
    pub hotkey: String,
    /// Which groups of settings this preset captures. Defaults to engine-only,
    /// which is exactly what every preset written before the field existed meant.
    #[serde(default = "default_preset_scope")]
    pub scope: PresetScope,
    /// Smart Guard snapshot, present only when `scope.guards` is on.
    #[serde(default)]
    pub guard_settings: Option<GuardSnapshot>,
    /// Global hotkey snapshot, present only when `scope.hotkeys` is on.
    #[serde(default)]
    pub hotkey_settings: Option<HotkeySnapshot>,
    /// Appearance snapshot, present only when `scope.ui` is on.
    #[serde(default)]
    pub ui_settings: Option<PresetUiSnapshot>,
    /// GUI lock duration (ms). Lives in the engine config (the Start button's
    /// anti-double-click window), so a preset that captures the engine owns it.
    #[serde(default = "default_gui_lock_ms")]
    pub gui_lock_ms: u64,
    /// Unit the preset's `stop_duration_ms` is displayed in: ms/sec/min/hour.
    /// Display-only, exactly like `EngineSettings::stop_duration_unit`.
    #[serde(default = "default_stop_duration_unit")]
    pub stop_duration_unit: String,
    /// Free-form grouping label for the Presets grid (Combat / Utility / ...).
    #[serde(default = "default_preset_category")]
    pub category: String,
}

fn default_gui_lock_ms() -> u64 {
    1500
}

fn default_preset_category() -> String {
    "utility".into()
}

impl Default for PresetItem {
    fn default() -> Self {
        PresetItem {
            id: String::new(),
            name: "Default Preset".into(),
            description: String::new(),
            icon: "⚡".into(),
            target_cps: 10.0,
            jitter_percent: 5.0,
            click_limit: 0,
            button: default_button(),
            click_type: default_click_type(),
            position_mode: default_position_mode(),
            fixed_x: default_100(),
            fixed_y: default_100(),
            hold_duration_ms: default_500(),
            hold_interval_ms: default_1000(),
            jitter_radius_px: default_3(),
            outlier_prob: default_outlier_prob(),
            technique: default_technique(),
            repeat_mode: default_repeat_mode(),
            repeat_count: 0,
            repeat_interval_ms: default_1000(),
            start_delay_ms: 0,
            stop_duration_ms: 0,
            stop_duration_min: 0,
            stop_time_str: String::new(),
            stop_mode: default_stop_mode(),
            is_default: false,
            points: Vec::new(),
            hotkey: String::new(),
            scope: default_preset_scope(),
            guard_settings: None,
            hotkey_settings: None,
            ui_settings: None,
            gui_lock_ms: default_gui_lock_ms(),
            stop_duration_unit: default_stop_duration_unit(),
            category: default_preset_category(),
        }
    }
}

fn default_button() -> String {
    "left".into()
}

fn default_3() -> u32 {
    3
}

pub(crate) fn default_presets() -> Vec<PresetItem> {
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
            outlier_prob: default_outlier_prob(),
            technique: default_technique(),
            repeat_mode: "unlimited".into(),
            repeat_count: 0,
            repeat_interval_ms: 1000,
            start_delay_ms: 0,
            stop_duration_ms: 0,
            stop_duration_min: 0,
            stop_time_str: String::new(),
            stop_mode: default_stop_mode(),
            is_default: true,
            points: Vec::new(),
            hotkey: String::new(),
            scope: default_preset_scope(),
            guard_settings: None,
            hotkey_settings: None,
            ui_settings: None,
            gui_lock_ms: default_gui_lock_ms(),
            stop_duration_unit: default_stop_duration_unit(),
            category: "combat".into(),
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
            outlier_prob: default_outlier_prob(),
            technique: default_technique(),
            repeat_mode: "unlimited".into(),
            repeat_count: 0,
            repeat_interval_ms: 1000,
            start_delay_ms: 0,
            stop_duration_ms: 0,
            stop_duration_min: 0,
            stop_time_str: String::new(),
            stop_mode: default_stop_mode(),
            is_default: true,
            points: Vec::new(),
            hotkey: String::new(),
            scope: default_preset_scope(),
            guard_settings: None,
            hotkey_settings: None,
            ui_settings: None,
            gui_lock_ms: default_gui_lock_ms(),
            stop_duration_unit: default_stop_duration_unit(),
            category: "utility".into(),
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
            outlier_prob: default_outlier_prob(),
            technique: default_technique(),
            repeat_mode: "unlimited".into(),
            repeat_count: 0,
            repeat_interval_ms: 1000,
            start_delay_ms: 0,
            stop_duration_ms: 0,
            stop_duration_min: 0,
            stop_time_str: String::new(),
            stop_mode: default_stop_mode(),
            is_default: true,
            points: Vec::new(),
            hotkey: String::new(),
            scope: default_preset_scope(),
            guard_settings: None,
            hotkey_settings: None,
            ui_settings: None,
            gui_lock_ms: default_gui_lock_ms(),
            stop_duration_unit: default_stop_duration_unit(),
            category: "utility".into(),
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
            outlier_prob: default_outlier_prob(),
            technique: default_technique(),
            repeat_mode: "unlimited".into(),
            repeat_count: 0,
            repeat_interval_ms: 1000,
            start_delay_ms: 0,
            stop_duration_ms: 0,
            stop_duration_min: 0,
            stop_time_str: String::new(),
            stop_mode: default_stop_mode(),
            is_default: true,
            points: Vec::new(),
            hotkey: String::new(),
            scope: default_preset_scope(),
            guard_settings: None,
            hotkey_settings: None,
            ui_settings: None,
            gui_lock_ms: default_gui_lock_ms(),
            stop_duration_unit: default_stop_duration_unit(),
            category: "utility".into(),
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
    /// Window-title -> preset auto-switch rules. The poller is **mtime-gated**
    /// (2026-09-27): `config.json` is only read when it actually changed, the
    /// foreground lookup only runs when the rule set changed, and the loop exits
    /// once shutdown latches. Before that it called `load()` every 1500 ms no
    /// matter what — and `load()` used to rewrite `config.last_good.json` on every
    /// clean parse, i.e. ~40 disk writes a minute while the app sat in the tray.
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

/// Set once the session's pre-write snapshot has been attempted, so the FIRST write of
/// a process is the one that copies the previous config aside.
static SESSION_SNAPSHOT_DONE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub struct ConfigManager {
    config_path: PathBuf,
    /// Set once at boot so `save()` can mark its OWN write before it lands.
    ///
    /// The six writers that used to forget `mark_own_write` (onboarding, reset,
    /// HUD toggle, mode toggle, image trigger, backup import) each produced a
    /// false "File changed outside NanoClick" toast deterministically. Marking
    /// inside `save()` makes the class unrepeatable instead of relying on every
    /// call site remembering.
    observer: std::sync::OnceLock<std::sync::Arc<crate::watcher::Observer>>,
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
        ConfigManager {
            config_path,
            observer: std::sync::OnceLock::new(),
        }
    }

    /// Attach the boot observer so every `save()` marks its own write.
    /// Called once from `lib.rs` right after the observer is constructed;
    /// `OnceLock` keeps it idempotent and makes forgetting it harmless.
    pub fn attach_observer(&self, observer: std::sync::Arc<crate::watcher::Observer>) {
        let _ = self.observer.set(observer);
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

    /// Copy the live `config.json` aside **once per process**, before this session's
    /// first write: `config.json.bak-<epoch>` (the naming `backup_corrupted_file` uses).
    ///
    /// Insurance, not hygiene. A bug that writes a wrong-but-VALID config — the
    /// pre-hydration module default is the case that cost a real profile — leaves no
    /// corrupt file for the boot repair to notice, so the previous state has to be kept
    /// somewhere BEFORE the first overwrite. Returns the path only on the creating call.
    pub fn snapshot_before_first_save(&self) -> Option<PathBuf> {
        use std::sync::atomic::Ordering;
        if SESSION_SNAPSHOT_DONE.swap(true, Ordering::AcqRel) {
            return None;
        }
        if !self.config_path.exists() {
            return None;
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let dest = self.config_path.with_extension(format!("json.bak-{stamp}"));
        match fs::copy(&self.config_path, &dest) {
            Ok(_) => Some(dest),
            Err(e) => {
                eprintln!("[config] session snapshot failed: {e}");
                None
            }
        }
    }

    pub fn save(&self, config: &AppConfig) -> Result<(), String> {
        if let Some(parent) = self.config_path.parent() {
            if !parent.exists() {
                let _ = fs::create_dir_all(parent);
            }
        }

        // Mark BEFORE the write: the observer can only see our echo on its next
        // poll, and it must already know the write is ours by then. Doing it here
        // rather than at each call site is what makes "someone forgot to mark"
        // unrepeatable — six writers did forget it, and each produced a false
        // "File changed outside NanoClick" toast on every invocation.
        if let Some(observer) = self.observer.get() {
            observer.mark_own_write("config");
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
        let cm = ConfigManager {
            config_path: config_file.clone(),
            observer: std::sync::OnceLock::new(),
        };

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
