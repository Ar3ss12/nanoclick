pub use crate::config_manager::AppConfig;

/// Hard ceiling for the auto-stop duration: 999 hours.
///
/// The UI offers ms / sec / min / hour with fractional values, so "999 hour" is
/// the largest thing a user can type. The bound lives here (not in the UI) so
/// every other path — a hand-edited config, an imported preset bundle, a stale
/// value from an older build — is clamped to something the click loop can wait
/// for without overflowing `Duration::from_millis` arithmetic.
pub const MAX_STOP_DURATION_MS: u64 = 3_596_400_000;

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
    /// Rare biological hesitation probability (0.0 = off, default 0.02).
    pub outlier_prob: f64,
    /// Biometric technique hint (Phase A: UI only): auto/single/butterfly/drag.
    pub technique: String,
    pub hotkey_toggle: String,
    pub hotkey_mode_switch: String,
    pub hotkey_emergency_stop: String,
    pub hotkey_speed_up: String,
    pub hotkey_slow_down: String,
    pub hotkey_capture_pos: String,
    pub hotkey_record_toggle: bool,
    pub hotkey_preset_slots: Vec<String>,
    pub hotkey_record: String,
    pub start_delay_ms: u64,
    pub stop_duration_ms: u64,
    pub stop_time_epoch_sec: i64,
    /// WHICH auto-stop trigger is armed: "none" | "duration" | "wallclock".
    /// Both values above are always carried, so switching back restores what the
    /// user typed — but the click loop must only ever act on ONE of them, and the
    /// backend cannot guess which of two non-zero values was meant.
    pub stop_mode: String,
    pub gui_lock_ms: u64,
    pub hotkey_debounce_ms: u32,
    pub active_mode: String,
    /// Typing Guard: a toggle hotkey pressed within this window after real
    /// text input is ignored (a letter inside a word, not a deliberate switch).
    pub typing_pause_ms: u32,
    /// Focus Guard: auto-pause when the foreground app CHANGES mid-run
    /// (Alt+Tab, click on another window, Win key, system toast).
    pub pause_on_focus_loss: bool,
    /// App-scope filter mode: "everywhere" | "whitelist" | "blacklist".
    pub app_filter_mode: String,
    /// Lowercase process image names for the app filter (e.g. "discord.exe").
    pub app_filter_list: Vec<String>,
    /// Optional multi-point click sequence. When non-empty the engine
    /// visits each point in order with the per-point delay, looping
    /// until stopped. Empty falls back to the legacy single-point
    /// behaviour (fixed_x/fixed_y + jitter_radius_px).
    pub sequence_points: Vec<crate::config_manager::SequencePoint>,
    pub visual_ripple: bool,
    pub presets: Vec<crate::config_manager::PresetItem>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            cps: 10.0,
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
            jitter_radius_px: 0,
            outlier_prob: 0.02,
            technique: "auto".into(),
            hotkey_toggle: "R / K".into(),
            hotkey_mode_switch: "Ctrl+Alt+M".into(),
            hotkey_emergency_stop: "Escape".into(),
            hotkey_speed_up: "Ctrl+=".into(),
            hotkey_slow_down: "Ctrl+-".into(),
            hotkey_capture_pos: "Ctrl+P".into(),
            hotkey_record_toggle: true,
            hotkey_record: "Ctrl+Shift+R".into(),
            hotkey_preset_slots: Vec::new(),
            start_delay_ms: 0,
            stop_duration_ms: 0,
            stop_time_epoch_sec: 0,
            stop_mode: "none".into(),
            gui_lock_ms: 1500,
            hotkey_debounce_ms: 80,
            active_mode: "autoclicker".into(),
            typing_pause_ms: 600,
            pause_on_focus_loss: false,
            app_filter_mode: "everywhere".into(),
            app_filter_list: Vec::new(),
            sequence_points: Vec::new(),
            visual_ripple: true,
            presets: Vec::new(),
        }
    }
}

impl From<AppConfig> for Config {
    fn from(app_cfg: AppConfig) -> Self {
        // Resolved BEFORE the struct literal: the literal moves the String fields
        // of `app_cfg.engine` (`button`, `repeat_mode`, ...), so the engine cannot
        // be borrowed as a whole inside it (E0382 partial move).
        let stop_duration_ms = resolve_stop_duration_ms(&app_cfg.engine);
        let stop_time_epoch_sec = parse_stop_time_str_next(&app_cfg.engine.stop_time_str);
        let stop_mode = resolve_stop_mode(
            &app_cfg.engine.stop_mode,
            stop_duration_ms,
            stop_time_epoch_sec,
        );
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
            outlier_prob: normalize_outlier_prob(app_cfg.engine.outlier_prob),
            technique: normalize_technique(&app_cfg.engine.technique),
            hotkey_toggle: app_cfg.hotkeys.toggle,
            hotkey_mode_switch: app_cfg.hotkeys.mode_switch,
            hotkey_emergency_stop: app_cfg.hotkeys.emergency_stop,
            hotkey_speed_up: app_cfg.hotkeys.speed_up,
            hotkey_slow_down: app_cfg.hotkeys.slow_down,
            hotkey_capture_pos: app_cfg.hotkeys.capture_pos,
            hotkey_record_toggle: app_cfg.hotkeys.record_toggle,
            hotkey_preset_slots: app_cfg.hotkeys.preset_hotkeys.clone(),
            hotkey_record: app_cfg.hotkeys.record_hotkey,
            start_delay_ms: app_cfg.engine.start_delay_ms,
            stop_duration_ms,
            stop_time_epoch_sec,
            stop_mode,
            gui_lock_ms: app_cfg.engine.gui_lock_ms,
            hotkey_debounce_ms: app_cfg.engine.hotkey_debounce_ms,
            active_mode: app_cfg.active_mode,
            typing_pause_ms: app_cfg.ui.typing_pause_ms,
            pause_on_focus_loss: app_cfg.ui.pause_on_focus_loss,
            app_filter_mode: normalize_app_filter_mode(&app_cfg.ui.app_filter_mode),
            app_filter_list: normalize_app_filter_list(&app_cfg.ui.app_filter_list),
            sequence_points: app_cfg.engine.sequence_points.clone(),
            visual_ripple: app_cfg.ui.visual_ripple,
            presets: app_cfg.presets.clone(),
        }
    }
}

/// Normalize app-filter mode to one of the three known values.
pub fn normalize_app_filter_mode(s: &str) -> String {
    match s.trim().to_ascii_lowercase().as_str() {
        "whitelist" | "white" | "only" => "whitelist".into(),
        "blacklist" | "black" | "block" => "blacklist".into(),
        _ => "everywhere".into(),
    }
}

/// Normalize exe list: trim, lowercase, drop empties, dedupe (sorted).
pub fn normalize_app_filter_list(list: &[String]) -> Vec<String> {
    let mut out: Vec<String> = list
        .iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Auto-stop duration in ms for the click loop, clamped to
/// [`MAX_STOP_DURATION_MS`].
///
/// `stop_duration_min` is the pre-1.3 field (whole minutes, `u32`). A config
/// written by an older build still carries the user's intent there, so it is the
/// migration fallback whenever the ms field is zero — this is the single place
/// that resolves the two, and the page mirrors the same rule so the first save
/// after an upgrade cannot write 0 over the user's timer.
pub fn resolve_stop_duration_ms(engine: &crate::config_manager::EngineSettings) -> u64 {
    let ms = if engine.stop_duration_ms > 0 {
        engine.stop_duration_ms
    } else {
        u64::from(engine.stop_duration_min).saturating_mul(60_000)
    };
    ms.min(MAX_STOP_DURATION_MS)
}

/// Normalize a stored `stop_mode` string to one of the three legal values —
/// `"none" | "duration" | "wallclock"`.
///
/// Accepts the aliases the UI/config may have produced over time so an upgrade
/// cannot silently disarm a timer ("at", "clock", "time" → wallclock; "after",
/// "min", "ms", "sec" → duration). Anything unrecognized becomes `"none"`: a
/// hand-edited or corrupted config must never arm a timer the UI cannot show.
///
/// The page keeps its own copy of the same three values (the `STOP_MODES` array
/// in `main.js`) because the two sides of the bridge cannot share code.
pub fn normalize_stop_mode(mode: &str) -> String {
    match mode.trim().to_ascii_lowercase().as_str() {
        "duration" | "after" | "timer" | "min" | "ms" | "sec" => "duration".into(),
        "wallclock" | "at" | "clock" | "time" | "stop_at" => "wallclock".into(),
        _ => "none".into(),
    }
}

/// Resolve the armed trigger. An explicit mode wins **only if the trigger it
/// names actually holds a value** — otherwise the other field takes over, and
/// with both empty the answer is `"none"`.
///
/// Two reasons, both learned the hard way:
///
/// * a config written before `stop_mode` existed deserializes to `"none"` while
///   carrying BOTH values, and treating that as "nothing armed" silently
///   disarmed a timer the user had deliberately set (the pre-1.3 whole-minutes
///   bug in a new costume) — so the presence of a value is the intent, duration
///   first, exactly as the old behaviour where the earlier deadline stopped the run;
/// * the page paints the same rule (`resolveStopMode`), and an armed-yet-empty
///   trigger would make the scheduler stop for a reason the card says is off.
pub fn resolve_stop_mode(mode: &str, stop_duration_ms: u64, stop_time_epoch_sec: i64) -> String {
    let explicit = normalize_stop_mode(mode);
    let duration_armed = stop_duration_ms > 0;
    let wallclock_armed = stop_time_epoch_sec > 0;
    match explicit.as_str() {
        "duration" if duration_armed => "duration".into(),
        "wallclock" if wallclock_armed => "wallclock".into(),
        _ if duration_armed => "duration".into(),
        _ if wallclock_armed => "wallclock".into(),
        _ => "none".into(),
    }
}

/// Ceiling for the biological-hesitation probability: 0.10 (10%).
pub const MAX_OUTLIER_PROB: f64 = 0.10;

/// Default hesitation probability (2% of intervals stretch to 2-3x base).
pub const DEFAULT_OUTLIER_PROB: f64 = 0.02;

/// Clamp a stored hesitation probability: NaN/negative/infinite -> default,
/// above the ceiling -> ceiling. `0.0` stays `0.0` (switch-off).
pub fn normalize_outlier_prob(p: f64) -> f64 {
    if !p.is_finite() || p < 0.0 {
        return DEFAULT_OUTLIER_PROB;
    }
    if p > MAX_OUTLIER_PROB {
        return MAX_OUTLIER_PROB;
    }
    p
}

/// Normalize the biometric technique hint: auto/single/butterfly/drag.
/// Anything else falls back to "auto" (Phase A: UI badge only).
pub fn normalize_technique(s: &str) -> String {
    match s.trim().to_ascii_lowercase().as_str() {
        "single" => "single".into(),
        "butterfly" | "double" => "butterfly".into(),
        "drag" => "drag".into(),
        _ => "auto".into(),
    }
}

/// Resolve which biometric technique the CPS badge shows.
/// Pinned (non-auto) always wins; auto maps 0-15 -> single,
/// 15-25 -> butterfly, 25+ -> drag.
#[allow(dead_code)]
pub fn resolve_technique(technique: &str, cps: f64) -> String {
    let pinned = normalize_technique(technique);
    if pinned != "auto" {
        return pinned;
    }
    if cps <= 15.0 {
        "single".into()
    } else if cps <= 25.0 {
        "butterfly".into()
    } else {
        "drag".into()
    }
}

/// Parse "HH:MM" (24-hour) into `(hour, minute)`; `None` when empty/malformed.
fn parse_hhmm(s: &str) -> Option<(i64, i64)> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut parts = s.split(':');
    let h: i64 = parts.next().and_then(|p| p.trim().parse().ok())?;
    let m: i64 = parts.next().and_then(|p| p.trim().parse().ok())?;
    if !(0..=23).contains(&h) || !(0..=59).contains(&m) {
        return None;
    }
    Some((h, m))
}

fn now_unix_secs() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

/// Core of both "stop at" parsers with the clock INJECTED (tests pin it, the
/// production callers pass `now_unix_secs()`), so the roll-over rule is provable
/// without waiting for a wall clock.
fn stop_time_target(now: i64, h: i64, m: i64, roll_to_tomorrow: bool) -> i64 {
    let today_midnight = now - (now % 86_400);
    let target = today_midnight + h * 3600 + m * 60;
    if target < now {
        if roll_to_tomorrow {
            target + 86_400
        } else {
            0
        }
    } else {
        target
    }
}

/// Parse "HH:MM" into the Unix-epoch second the run should stop at.
///
/// A time that has already passed today means **tomorrow** (the next occurrence)
/// rather than "disabled" — this is what the config feeds the click loop.
///
/// The retired rule ("already past today" → `0`) silently disarmed the field, and
/// because `Config::from` recomputes it on every save, any unrelated setting
/// change wiped a timer the user had just set while the UI kept showing ACTIVE
/// (the page read its own input). `stop_time_target` still takes the roll-over as
/// a flag, so the old rule stays pinned by a test instead of being forgotten.
pub fn parse_stop_time_str_next(s: &str) -> i64 {
    match parse_hhmm(s) {
        Some((h, m)) => stop_time_target(now_unix_secs(), h, m, true),
        None => 0,
    }
}

#[cfg(test)]
mod stop_timer_tests {
    use super::*;
    use crate::config_manager::EngineSettings;

    fn engine_with(stop_duration_ms: u64, legacy_minutes: u32) -> EngineSettings {
        EngineSettings {
            stop_duration_ms,
            stop_duration_min: legacy_minutes,
            ..Default::default()
        }
    }

    /// The ms field is authoritative and the ceiling is enforced HERE, not only
    /// by the input's min/max attributes (a hand-edited config or an imported
    /// preset bundle never went through the page's number input).
    #[test]
    fn resolve_stop_duration_ms_uses_ms_and_clamps_to_the_ceiling() {
        assert_eq!(resolve_stop_duration_ms(&engine_with(1_500, 0)), 1_500);
        assert_eq!(resolve_stop_duration_ms(&engine_with(0, 0)), 0);
        assert_eq!(
            resolve_stop_duration_ms(&engine_with(MAX_STOP_DURATION_MS + 1, 0)),
            MAX_STOP_DURATION_MS
        );
        assert_eq!(
            resolve_stop_duration_ms(&engine_with(u64::MAX, 0)),
            MAX_STOP_DURATION_MS
        );
    }

    /// A config written before the unit picker carried whole MINUTES only.
    /// Dropping that to 0 is how an auto-stop "disappears" on upgrade, so the
    /// legacy field is the migration fallback — never an override.
    #[test]
    fn resolve_stop_duration_ms_migrates_the_legacy_minutes_field() {
        assert_eq!(resolve_stop_duration_ms(&engine_with(0, 10)), 600_000);
        assert_eq!(resolve_stop_duration_ms(&engine_with(0, 1_440)), 86_400_000);
        assert_eq!(resolve_stop_duration_ms(&engine_with(1_500, 10)), 1_500);
        // The fallback is clamped too: u32::MAX minutes must not overflow into a
        // `Duration` the click loop cannot wait for.
        assert_eq!(
            resolve_stop_duration_ms(&engine_with(0, u32::MAX)),
            MAX_STOP_DURATION_MS
        );
    }

    /// "Stop at" means the NEXT occurrence. The clock is injected, so this is
    /// deterministic: 1_700_000_000 = 2023-11-14 22:13:20 UTC.
    #[test]
    fn stop_at_rolls_a_passed_time_into_tomorrow() {
        let now: i64 = 1_700_000_000;
        let midnight = now - (now % 86_400);

        // Ahead of `now` → today, in both variants.
        for roll in [false, true] {
            assert_eq!(stop_time_target(now, 23, 0, roll), midnight + 23 * 3600);
        }
        // Already passed today → "tomorrow" for the live rule. The retired rule
        // (flag = false) must keep answering 0: pinned here so the two can never
        // silently converge into one behaviour.
        assert_eq!(
            stop_time_target(now, 21, 0, true),
            midnight + 21 * 3600 + 86_400
        );
        assert_eq!(stop_time_target(now, 21, 0, false), 0);
    }

    /// Malformed / empty input stays "no stop time" — the roll-over must not turn
    /// a typo into a 24-hour timer.
    #[test]
    fn stop_at_rejects_malformed_input() {
        for bad in ["", "   ", "nonsense", "25:00", "12:60", "12", "12:", ":30"] {
            assert!(parse_hhmm(bad).is_none(), "parse_hhmm accepted {bad:?}");
            assert_eq!(
                parse_stop_time_str_next(bad),
                0,
                "the roll-over accepted {bad:?}"
            );
        }
        assert_eq!(parse_hhmm("23:59"), Some((23, 59)));
        assert!(parse_stop_time_str_next("23:59") > 0);
    }

    /// ONE armed trigger per run. Two non-zero deadlines are ambiguous, so the
    /// discriminator must exist and it must never arm a trigger that has no
    /// value — an armed-yet-empty trigger makes the loop stop for a reason the
    /// card shows as off.
    #[test]
    fn stop_mode_arms_exactly_one_trigger() {
        // Aliases a config or the UI may have produced over time.
        assert_eq!(normalize_stop_mode("duration"), "duration");
        assert_eq!(normalize_stop_mode("after"), "duration");
        assert_eq!(normalize_stop_mode("  WALLCLOCK "), "wallclock");
        assert_eq!(normalize_stop_mode("at"), "wallclock");
        // Anything unknown must NOT arm a timer: a hand-edited config is not a
        // reason to stop the user's run.
        for junk in ["", "  ", "both", "sometimes", "true"] {
            assert_eq!(normalize_stop_mode(junk), "none", "junk {junk:?}");
        }

        // The explicit mode wins while its trigger holds a value.
        assert_eq!(resolve_stop_mode("duration", 1_500, 0), "duration");
        assert_eq!(resolve_stop_mode("wallclock", 0, 1_700_000_000), "wallclock");
        // …and loses to the other value the moment its own is cleared, so a run
        // is never left disarmable while a usable deadline sits in the config.
        assert_eq!(resolve_stop_mode("duration", 0, 1_700_000_000), "wallclock");
        assert_eq!(resolve_stop_mode("wallclock", 1_500, 0), "duration");
        // Both empty → nothing armed, whatever the stored mode claims.
        assert_eq!(resolve_stop_mode("duration", 0, 0), "none");
        assert_eq!(resolve_stop_mode("none", 0, 0), "none");
        // A legacy config (no `stop_mode`) carrying a value is NOT disarmed: the
        // value itself is the intent, duration first. This is the pre-1.3
        // whole-minutes bug wearing a new costume.
        assert_eq!(resolve_stop_mode("none", 1_500, 0), "duration");
        assert_eq!(resolve_stop_mode("none", 0, 1_700_000_000), "wallclock");
        assert_eq!(resolve_stop_mode("none", 1_500, 1_700_000_000), "duration");
    }

    #[test]
    fn outlier_prob_normalizes_to_default_and_ceiling() {
        assert_eq!(normalize_outlier_prob(0.02), 0.02);
        assert_eq!(normalize_outlier_prob(0.0), 0.0); // switch-off survives
        assert_eq!(normalize_outlier_prob(DEFAULT_OUTLIER_PROB), DEFAULT_OUTLIER_PROB);
        assert_eq!(normalize_outlier_prob(0.5), MAX_OUTLIER_PROB);
        assert_eq!(normalize_outlier_prob(f64::INFINITY), DEFAULT_OUTLIER_PROB);
        assert_eq!(normalize_outlier_prob(f64::NAN), DEFAULT_OUTLIER_PROB);
        assert_eq!(normalize_outlier_prob(-0.01), DEFAULT_OUTLIER_PROB);
    }

    #[test]
    fn technique_resolves_auto_bands_and_pins() {
        assert_eq!(resolve_technique("auto", 8.0), "single");
        assert_eq!(resolve_technique("auto", 15.0), "single");
        assert_eq!(resolve_technique("auto", 15.5), "butterfly");
        assert_eq!(resolve_technique("auto", 25.0), "butterfly");
        assert_eq!(resolve_technique("auto", 25.5), "drag");
        assert_eq!(resolve_technique("auto", 120.0), "drag");
        // Pinned always wins over CPS.
        assert_eq!(resolve_technique("single", 120.0), "single");
        assert_eq!(resolve_technique("butterfly", 5.0), "butterfly");
        assert_eq!(resolve_technique("drag", 5.0), "drag");
        // Legacy alias + junk fall back honestly.
        assert_eq!(normalize_technique("double"), "butterfly");
        assert_eq!(normalize_technique("  DRAG "), "drag");
        assert_eq!(normalize_technique("turbo"), "auto");
    }
}
