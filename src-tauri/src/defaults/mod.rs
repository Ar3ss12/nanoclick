use crate::config_manager::{
    AppConfig, AppProfile, ImageTrigger, PresetItem, StatsConfig, CONFIG_SCHEMA_VERSION,
};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const EMBEDDED_DEFAULT_CONFIG_JSON: &str = include_str!("default_config.json");

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelfHealingAction {
    LoadedExisting,
    CreatedFresh,
    RepairedAndPatched {
        backup_path: PathBuf,
        details: Vec<String>,
    },
    CorruptedAndRecovered {
        backup_path: PathBuf,
        /// Where the final config came from: last-known-good snapshot or factory default.
        restored_from_last_good: bool,
    },
    Migrated,
}

/// File name of the golden snapshot next to `config.json`.
/// Written ONLY on clean parse; restored on unrecoverable corruption.
/// Protected on Windows with READONLY+HIDDEN ("black box": invisible in
/// Explorer by default, Ctrl+S in Notepad bounces off).
pub const LAST_GOOD_FILE_NAME: &str = "config.last_good.json";

/// Best-effort hardening of the golden snapshot on Windows:
/// READONLY (blocks accidental Ctrl+S) + HIDDEN (out of sight in Explorer).
/// Never fails the save path — a snapshot without attributes is still valid.
#[cfg(target_os = "windows")]
fn harden_last_good(path: &Path) {
    use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_READONLY};
    set_file_attrs(path, FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_READONLY.0);
}

#[cfg(not(target_os = "windows"))]
fn harden_last_good(_path: &Path) {}

/// Best-effort unharden before WE rewrite the snapshot ourselves.
/// Without this, rename/replace onto a readonly target dies with
/// ERROR_ACCESS_DENIED (5) on Windows.
#[cfg(target_os = "windows")]
fn unharden_last_good(path: &Path) {
    use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
    set_file_attrs(path, FILE_ATTRIBUTE_NORMAL.0);
}

#[cfg(not(target_os = "windows"))]
fn unharden_last_good(_path: &Path) {}

/// Raw Win32 attribute write. Uses `encode_wide` + `PCWSTR::from_raw` so no
/// UTF-8/HSTRING conversion can ever fail on a weird user profile path.
#[cfg(target_os = "windows")]
fn set_file_attrs(path: &Path, attrs: u32) {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_FLAGS_AND_ATTRIBUTES};
    use windows::core::PCWSTR;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let _ = SetFileAttributesW(
            PCWSTR::from_raw(wide.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(attrs),
        );
    }
}

/// Path of the golden snapshot living next to the live config.
pub fn last_good_path(config_path: &Path) -> PathBuf {
    match config_path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.join(LAST_GOOD_FILE_NAME),
        _ => PathBuf::from(LAST_GOOD_FILE_NAME),
    }
}

/// Refresh the golden snapshot from an already-validated in-memory config
/// (e.g. right after OUR OWN atomic save). Skips re-parsing: the caller
/// guarantees validity. Best-effort, never fails the save path.
pub fn refresh_last_good_snapshot(config_path: &Path, config: &AppConfig) {
    write_last_good_snapshot(config_path, config);
}

/// Persist the golden snapshot atomically (tmp+rename), then harden it
/// (readonly+hidden, best-effort). The tmp file is created WITHOUT attributes
/// so we never lock ourselves out on the next cycle.
fn write_last_good_snapshot(dir_holder: &Path, config: &AppConfig) {
    let snap_path = last_good_path(dir_holder);
    unharden_last_good(&snap_path);
    if write_config_atomic(&snap_path, config).is_ok() {
        harden_last_good(&snap_path);
    }
}

/// Load the golden snapshot if it parses cleanly (no repair allowed —
/// a snapshot that needs repair is not "known good").
fn load_last_good_snapshot(config_path: &Path) -> Option<AppConfig> {
    let snap_path = last_good_path(config_path);
    let raw = fs::read_to_string(&snap_path).ok()?;
    if raw.trim().is_empty() {
        return None;
    }
    let val: Value = serde_json::from_str(raw.trim().trim_start_matches('\u{feff}')).ok()?;
    match migrate_and_validate(val) {
        Ok((cfg, _)) => Some(cfg),
        Err(_) => None,
    }
}

/// Parse the embedded default configuration template into an `AppConfig`.
/// Falls back to `AppConfig::default()` in the impossible event of a deserialization error.
pub fn get_embedded_default_config() -> AppConfig {
    serde_json::from_str::<AppConfig>(EMBEDDED_DEFAULT_CONFIG_JSON)
        .unwrap_or_else(|e| {
            eprintln!("[defaults] Embedded config JSON deserialization failed: {e}. Using Rust default.");
            AppConfig::default()
        })
}

/// Atomically write an AppConfig to disk via a temporary file and rename.
pub fn write_config_atomic(config_path: &Path, config: &AppConfig) -> Result<(), String> {
    if let Some(parent) = config_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config directory {:?}: {}", parent, e))?;
        }
    }

    let json = serde_json::to_string_pretty(config)
        .map_err(|e| format!("Failed to serialize config: {}", e))?;

    let tmp_path = config_path.with_extension("json.tmp");
    fs::write(&tmp_path, json)
        .map_err(|e| format!("Failed to write temporary config to {:?}: {}", tmp_path, e))?;

    fs::rename(&tmp_path, config_path)
        .map_err(|e| format!("Failed to atomically replace {:?}: {}", config_path, e))?;

    Ok(())
}

/// Create a timestamped backup of a corrupted or existing config file.
/// Produces both `config.json.bak` and a unique `config.json.bak-YYYYMMDD-HHMMSS`.
pub fn backup_corrupted_file(config_path: &Path) -> Option<PathBuf> {
    if !config_path.exists() {
        return None;
    }

    let parent = config_path.parent().unwrap_or_else(|| Path::new("."));
    let stem = config_path.file_name().and_then(|s| s.to_str()).unwrap_or("config.json");

    let now_epoch = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let stamped_name = format!("{stem}.bak-{now_epoch}");
    let stamped_path = parent.join(stamped_name);
    let standard_bak = parent.join(format!("{stem}.bak"));

    // Copy to both standard .bak and unique stamped path
    let _ = fs::copy(config_path, &standard_bak);
    let _ = fs::copy(config_path, &stamped_path);

    Some(standard_bak)
}

/// Attempt to fix minor JSON syntax issues:
/// - Trailing commas before `}` or `]`
/// - Truncated files with unclosed `}` or `]`
pub fn try_repair_json_syntax(raw: &str) -> Option<Value> {
    let trimmed = raw.trim().trim_start_matches('\u{feff}');
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(val) = serde_json::from_str::<Value>(trimmed) {
        return Some(val);
    }

    let mut cleaned = String::with_capacity(trimmed.len() + 16);
    let mut in_string = false;
    let mut escape = false;
    let chars: Vec<char> = trimmed.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let c = chars[i];
        if in_string {
            cleaned.push(c);
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }

        if c == '"' {
            in_string = true;
            cleaned.push(c);
            i += 1;
            continue;
        }

        if c == ',' {
            let mut j = i + 1;
            while j < len && chars[j].is_whitespace() {
                j += 1;
            }
            if j < len && (chars[j] == '}' || chars[j] == ']') {
                i += 1;
                continue;
            }
        }

        cleaned.push(c);
        i += 1;
    }

    if let Ok(val) = serde_json::from_str::<Value>(&cleaned) {
        return Some(val);
    }

    let mut open_braces = 0i32;
    let mut open_brackets = 0i32;
    in_string = false;
    escape = false;

    for &c in &chars {
        if in_string {
            if escape {
                escape = false;
            } else if c == '\\' {
                escape = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        if c == '"' {
            in_string = true;
            continue;
        }
        match c {
            '{' => open_braces += 1,
            '}' => open_braces = (open_braces - 1).max(0),
            '[' => open_brackets += 1,
            ']' => open_brackets = (open_brackets - 1).max(0),
            _ => {}
        }
    }

    if open_braces > 0 || open_brackets > 0 {
        let mut balanced = cleaned;
        for _ in 0..open_brackets {
            balanced.push(']');
        }
        for _ in 0..open_braces {
            balanced.push('}');
        }
        if let Ok(val) = serde_json::from_str::<Value>(&balanced) {
            return Some(val);
        }
    }

    None
}

/// Intelligent Self-Healing & Patching:
/// Overlays user-defined settings onto the default configuration template.
/// If any field is missing or has an invalid type/value, ONLY that field is replaced with the default.
/// Valid user hotkeys, themes, presets, and stats are preserved.
pub fn repair_and_patch_value(user_val: &Value) -> Result<(AppConfig, Vec<String>), String> {
    let user_obj = match user_val.as_object() {
        Some(obj) => obj,
        None => return Err("Config root must be a JSON object".into()),
    };

    let mut details = Vec::new();
    let mut default_val: Value = serde_json::from_str(EMBEDDED_DEFAULT_CONFIG_JSON)
        .map_err(|e| format!("Failed to parse embedded default: {e}"))?;
    let default_obj = default_val
        .as_object_mut()
        .ok_or_else(|| "Default config root must be a JSON object".to_string())?;

    // 1. schema_version
    default_obj.insert("schema_version".into(), Value::from(CONFIG_SCHEMA_VERSION));

    // 2. first_run
    if let Some(b) = user_obj.get("first_run").and_then(Value::as_bool) {
        default_obj.insert("first_run".into(), Value::from(b));
    } else {
        default_obj.insert("first_run".into(), Value::from(false));
    }

    // 3. active_mode
    if let Some(m) = user_obj.get("active_mode").and_then(Value::as_str) {
        if matches!(m, "autoclicker" | "work") {
            default_obj.insert("active_mode".into(), Value::from(m));
        } else {
            details.push(format!("active_mode '{m}' invalid; repaired to 'autoclicker'"));
        }
    }

    // 4. engine settings
    if let (Some(default_engine), Some(user_engine)) = (
        default_obj.get_mut("engine").and_then(Value::as_object_mut),
        user_obj.get("engine").and_then(Value::as_object),
    ) {
        let cps_val = user_engine
            .get("target_cps")
            .and_then(Value::as_f64)
            .or_else(|| user_engine.get("cps").and_then(Value::as_f64));

        if let Some(cps) = cps_val {
            if cps > 0.0 && cps <= 1000.0 && cps.is_finite() {
                default_engine.insert("target_cps".into(), Value::from(cps));
            } else {
                details.push(format!("engine.target_cps {cps} out of range; repaired to 10.0"));
            }
        } else if user_engine.contains_key("target_cps") || user_engine.contains_key("cps") {
            details.push("engine.target_cps invalid type; repaired to 10.0".into());
        }

        if let Some(j) = user_engine.get("jitter_percent").and_then(Value::as_f64) {
            if (0.0..=100.0).contains(&j) && j.is_finite() {
                default_engine.insert("jitter_percent".into(), Value::from(j));
            }
        }
        if let Some(lim) = user_engine.get("click_limit").and_then(Value::as_u64) {
            default_engine.insert("click_limit".into(), Value::from(lim));
        }
        if let Some(rad) = user_engine.get("jitter_radius_px").and_then(Value::as_u64) {
            default_engine.insert("jitter_radius_px".into(), Value::from(rad));
        }
        if let Some(btn) = user_engine.get("button").and_then(Value::as_str) {
            if matches!(btn, "left" | "right" | "middle") {
                default_engine.insert("button".into(), Value::from(btn));
            } else {
                details.push(format!("engine.button '{btn}' invalid; repaired to 'left'"));
            }
        }
        if let Some(ct) = user_engine.get("click_type").and_then(Value::as_str) {
            if matches!(ct, "single" | "double" | "hold") {
                default_engine.insert("click_type".into(), Value::from(ct));
            } else {
                details.push(format!("engine.click_type '{ct}' invalid; repaired to 'single'"));
            }
        }
        if let Some(pm) = user_engine.get("position_mode").and_then(Value::as_str) {
            if matches!(pm, "cursor" | "fixed") {
                default_engine.insert("position_mode".into(), Value::from(pm));
            }
        }
        if let Some(fx) = user_engine.get("fixed_x").and_then(Value::as_i64) {
            default_engine.insert("fixed_x".into(), Value::from(fx));
        }
        if let Some(fy) = user_engine.get("fixed_y").and_then(Value::as_i64) {
            default_engine.insert("fixed_y".into(), Value::from(fy));
        }
        if let Some(rm) = user_engine.get("repeat_mode").and_then(Value::as_str) {
            if matches!(rm, "unlimited" | "repeat") {
                default_engine.insert("repeat_mode".into(), Value::from(rm));
            }
        }
        if let Some(rc) = user_engine.get("repeat_count").and_then(Value::as_u64) {
            default_engine.insert("repeat_count".into(), Value::from(rc));
        }
        if let Some(hd) = user_engine.get("hold_duration_ms").and_then(Value::as_u64) {
            default_engine.insert("hold_duration_ms".into(), Value::from(hd));
        }
        if let Some(hi) = user_engine.get("hold_interval_ms").and_then(Value::as_u64) {
            default_engine.insert("hold_interval_ms".into(), Value::from(hi));
        }
        if let Some(ri) = user_engine.get("repeat_interval_ms").and_then(Value::as_u64) {
            default_engine.insert("repeat_interval_ms".into(), Value::from(ri));
        }
        if let Some(sd) = user_engine.get("start_delay_ms").and_then(Value::as_u64) {
            default_engine.insert("start_delay_ms".into(), Value::from(sd));
        }
        if let Some(op) = user_engine.get("outlier_prob").and_then(Value::as_f64) {
            if op.is_finite() && (0.0..=0.10).contains(&op) {
                default_engine.insert("outlier_prob".into(), Value::from(op));
            } else {
                details.push(format!(
                    "engine.outlier_prob {op} out of range; repaired to 0.02"
                ));
                default_engine.insert("outlier_prob".into(), Value::from(0.02));
            }
        } else if user_engine.contains_key("outlier_prob") {
            details.push("engine.outlier_prob invalid type; repaired to 0.02".into());
        }
        if let Some(tq) = user_engine.get("technique").and_then(Value::as_str) {
            let norm = crate::config::normalize_technique(tq);
            if norm == "auto" && tq.trim().to_ascii_lowercase() != "auto" {
                details.push(format!(
                    "engine.technique '{tq}' invalid; repaired to 'auto'"
                ));
            }
            default_engine.insert("technique".into(), Value::from(norm));
        }
        // Auto-stop: `stop_duration_ms` is authoritative, `stop_duration_min` is
        // the pre-1.3 whole-minutes field. Migrating it here keeps the user's
        // timer alive across a repair/reset from an old config instead of
        // silently dropping it to 0 (which is exactly how an auto-stop gets
        // "lost" on upgrade).
        let user_stop_ms = user_engine
            .get("stop_duration_ms")
            .and_then(Value::as_u64)
            .filter(|ms| *ms > 0)
            .or_else(|| {
                user_engine
                    .get("stop_duration_min")
                    .and_then(Value::as_u64)
                    .filter(|min| *min > 0)
                    .map(|min| min.saturating_mul(60_000))
            });
        if let Some(ms) = user_stop_ms {
            default_engine.insert("stop_duration_ms".into(), Value::from(ms));
        }
        if let Some(unit) = user_engine.get("stop_duration_unit").and_then(Value::as_str) {
            default_engine.insert("stop_duration_unit".into(), Value::from(unit));
        }
        if let Some(st) = user_engine.get("stop_time_str").and_then(Value::as_str) {
            default_engine.insert("stop_time_str".into(), Value::from(st));
        }
        // The armed trigger. Normalized rather than trusted: a corrupted or
        // hand-edited string must fall back to a value the UI can render, and a
        // config written before `stop_mode` existed (field absent → "none")
        // must re-derive the arm from the values it carries, or a repair pass
        // would silently disarm a timer the user had deliberately set.
        let stored_mode = user_engine
            .get("stop_mode")
            .and_then(Value::as_str)
            .unwrap_or("none");
        let repaired_mode = crate::config::normalize_stop_mode(stored_mode);
        if repaired_mode != "none" {
            if repaired_mode != stored_mode.trim().to_ascii_lowercase() {
                details.push(format!(
                    "engine.stop_mode '{stored_mode}' invalid; repaired to '{repaired_mode}'"
                ));
            }
            default_engine.insert("stop_mode".into(), Value::from(repaired_mode));
        } else {
            // `stop_time_epoch_sec` is a runtime-only field (it does not exist in
            // the serialized `EngineSettings`), so the wall-clock intent is read
            // from `stop_time_str` — exactly what `Config::from` resolves into
            // the epoch.
            let ms = default_engine
                .get("stop_duration_ms")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let has_wallclock = default_engine
                .get("stop_time_str")
                .and_then(Value::as_str)
                .is_some_and(|s| !s.trim().is_empty());
            let derived = crate::config::resolve_stop_mode(
                "none",
                ms,
                if has_wallclock { 1 } else { 0 },
            );
            default_engine.insert("stop_mode".into(), Value::from(derived));
        }
        if let Some(gl) = user_engine.get("gui_lock_ms").and_then(Value::as_u64) {
            default_engine.insert("gui_lock_ms".into(), Value::from(gl));
        }
        if let Some(deb) = user_engine.get("hotkey_debounce_ms").and_then(Value::as_u64) {
            default_engine.insert("hotkey_debounce_ms".into(), Value::from(deb));
        }
        if let Some(pts) = user_engine.get("sequence_points").and_then(Value::as_array) {
            default_engine.insert("sequence_points".into(), Value::Array(pts.clone()));
        }
    }

    // 5. hotkeys
    if let (Some(default_hotkeys), Some(user_hotkeys)) = (
        default_obj.get_mut("hotkeys").and_then(Value::as_object_mut),
        user_obj.get("hotkeys").and_then(Value::as_object),
    ) {
        let key_mappings = [
            ("toggle", "start_stop"),
            ("mode_switch", "mode"),
            ("emergency_stop", "emergency_stop"),
            ("speed_up", "speed_up"),
            ("slow_down", "slow_down"),
            ("capture_pos", "capture_pos"),
            ("record_hotkey", "recording"),
        ];

        for (canonical, legacy) in key_mappings {
            let user_binding = user_hotkeys
                .get(canonical)
                .and_then(Value::as_str)
                .or_else(|| user_hotkeys.get(legacy).and_then(Value::as_str));

            if let Some(k) = user_binding {
                if !k.trim().is_empty() {
                    default_hotkeys.insert(canonical.into(), Value::from(k));
                } else {
                    details.push(format!("hotkeys.{canonical} empty string; kept default"));
                }
            }
        }

        if let Some(rt) = user_hotkeys.get("record_toggle").and_then(Value::as_bool) {
            default_hotkeys.insert("record_toggle".into(), Value::from(rt));
        }
        if let Some(ph) = user_hotkeys.get("preset_hotkeys").and_then(Value::as_array) {
            default_hotkeys.insert("preset_hotkeys".into(), Value::Array(ph.clone()));
        }
        if let Some(ttl) = user_hotkeys.get("key_ttl_ms").and_then(Value::as_u64) {
            default_hotkeys.insert("key_ttl_ms".into(), Value::from(ttl));
        }
        if let Some(sr) = user_hotkeys.get("smart_record").and_then(Value::as_bool) {
            default_hotkeys.insert("smart_record".into(), Value::from(sr));
        }
    }

    // 6. ui
    if let (Some(default_ui), Some(user_ui)) = (
        default_obj.get_mut("ui").and_then(Value::as_object_mut),
        user_obj.get("ui").and_then(Value::as_object),
    ) {
        let bool_keys = [
            "always_on_top",
            "visual_ripple",
            "show_hud",
            "start_minimized",
            "autostart",
            "minimize_to_tray",
            "deep_sleep_to_tray",
            "show_notifications",
            "pause_on_focus_loss",
            "remember_window_position",
            "always_run_as_admin",
        ];
        for k in bool_keys {
            if let Some(b) = user_ui.get(k).and_then(Value::as_bool) {
                default_ui.insert(k.into(), Value::from(b));
            }
        }
        if let Some(tp) = user_ui.get("typing_pause_ms").and_then(Value::as_u64) {
            default_ui.insert("typing_pause_ms".into(), Value::from(tp));
        }
        if let Some(afm) = user_ui.get("app_filter_mode").and_then(Value::as_str) {
            if matches!(afm, "everywhere" | "whitelist" | "blacklist") {
                default_ui.insert("app_filter_mode".into(), Value::from(afm));
            }
        }
        if let Some(afl) = user_ui.get("app_filter_list").and_then(Value::as_array) {
            default_ui.insert("app_filter_list".into(), Value::Array(afl.clone()));
        }
        for k in ["theme", "accent_color", "language"] {
            if let Some(s) = user_ui.get(k).and_then(Value::as_str) {
                if !s.trim().is_empty() {
                    default_ui.insert(k.into(), Value::from(s));
                }
            }
        }
        // Window position: two Option<i32> scalars, NOT a tuple/array.
        // Non-integer garbage (strings, floats, out-of-i32 huge) -> None
        // (fall back to center), never breaks the save. Clamp to ±32767
        // so a hand-edited 99999999 can't park the restore off-planet
        // (the boot sanitizer re-checks against the virtual screen anyway).
        for k in ["window_x", "window_y"] {
            match user_ui.get(k) {
                None => {
                    default_ui.insert(k.into(), Value::Null);
                }
                Some(Value::Null) => {
                    default_ui.insert(k.into(), Value::Null);
                }
                Some(v) => {
                    if let Some(n) = v.as_i64() {
                        if (-32767..=32767).contains(&n) {
                            default_ui.insert(k.into(), Value::from(n as i32));
                        } else {
                            details.push(format!("ui.{k} {n} out of range; reset to auto-center"));
                            default_ui.insert(k.into(), Value::Null);
                        }
                    } else {
                        details.push(format!("ui.{k} invalid type; reset to auto-center"));
                        default_ui.insert(k.into(), Value::Null);
                    }
                }
            }
        }
        // Window size: same lifecycle as the position, but the range is
        // POSITIVE (a 0 or negative size would make the restored window
        // invisible, and a junk-huge one would fill the screen). Anything
        // outside 200..=32767 falls back to `null` = the conf default size.
        for k in ["window_w", "window_h"] {
            let ok = user_ui
                .get(k)
                .and_then(Value::as_i64)
                .map(|n| (200..=32767).contains(&n))
                .unwrap_or(false);
            if ok {
                default_ui.insert(k.into(), user_ui.get(k).cloned().unwrap_or(Value::Null));
            } else {
                if user_ui.get(k).is_some_and(|v| !v.is_null()) {
                    details.push(format!("ui.{k} unusable as a size; reset to default"));
                }
                default_ui.insert(k.into(), Value::Null);
            }
        }
    }

    // 7. presets
    if let Some(user_presets) = user_obj.get("presets").and_then(Value::as_array) {
        let mut valid_presets = Vec::new();
        for item in user_presets {
            if let Ok(preset) = serde_json::from_value::<PresetItem>(item.clone()) {
                valid_presets.push(preset);
            }
        }
        if !valid_presets.is_empty() {
            if let Ok(presets_val) = serde_json::to_value(&valid_presets) {
                default_obj.insert("presets".into(), presets_val);
            }
        } else {
            details.push("presets array was corrupted; replaced with factory defaults".into());
        }
    }

    // 8. stats
    if let Some(user_stats) = user_obj.get("stats") {
        if let Ok(stats) = serde_json::from_value::<StatsConfig>(user_stats.clone()) {
            if let Ok(stats_val) = serde_json::to_value(&stats) {
                default_obj.insert("stats".into(), stats_val);
            }
        }
    }

    // 9. app_profiles
    if let Some(user_profiles) = user_obj.get("app_profiles").and_then(Value::as_array) {
        let mut valid = Vec::new();
        for item in user_profiles {
            if let Ok(prof) = serde_json::from_value::<AppProfile>(item.clone()) {
                valid.push(prof);
            }
        }
        if let Ok(prof_val) = serde_json::to_value(&valid) {
            default_obj.insert("app_profiles".into(), prof_val);
        }
    }

    // 10. image_trigger
    if let Some(user_it) = user_obj.get("image_trigger") {
        if user_it.is_null() {
            default_obj.insert("image_trigger".into(), Value::Null);
        } else if let Ok(it) = serde_json::from_value::<ImageTrigger>(user_it.clone()) {
            if let Ok(it_val) = serde_json::to_value(&it) {
                default_obj.insert("image_trigger".into(), it_val);
            }
        }
    }

    let repaired_config: AppConfig = serde_json::from_value(default_val)
        .map_err(|e| format!("Repaired config deserialization error: {e}"))?;

    Ok((repaired_config, details))
}

/// Core self-healing entrypoint:
/// 1. If file does not exist -> writes fresh default config atomically (`CreatedFresh`).
/// 2. If file exists but is corrupted -> attempts smart repair/patch first (`RepairedAndPatched`).
/// 3. If file cannot be parsed or repaired at all -> quarantines the broken file
///    with a timestamp, restores the last-known-good snapshot when available,
///    else falls back to factory default (`CorruptedAndRecovered`).
/// 4. If file is valid -> snapshots it as last-known-good (hardened
///    readonly+hidden on Windows), applies schema migration if needed
///    (`Migrated` or `LoadedExisting`).
pub fn ensure_config_file(config_path: &Path) -> (AppConfig, SelfHealingAction) {
    if !config_path.exists() {
        let default_cfg = get_embedded_default_config();
        let _ = write_config_atomic(config_path, &default_cfg);
        write_last_good_snapshot(config_path, &default_cfg);
        return (default_cfg, SelfHealingAction::CreatedFresh);
    }

    let raw_content = match fs::read_to_string(config_path) {
        Ok(c) => c,
        Err(err) => {
            eprintln!("[defaults] Failed to read config file ({err}). Recovering with defaults...");
            return recover_corrupted_file(config_path);
        }
    };

    if raw_content.trim().is_empty() {
        eprintln!("[defaults] Config file is empty. Recovering with defaults...");
        return recover_corrupted_file(config_path);
    }

    // Try parsing directly or via syntax repair
    let parsed_json = match serde_json::from_str::<Value>(&raw_content) {
        Ok(v) => Some(v),
        Err(_) => try_repair_json_syntax(&raw_content),
    };

    let json_val = match parsed_json {
        Some(v) => v,
        None => {
            eprintln!("[defaults] Config syntax is unrecoverable. Recovering with defaults...");
            return recover_corrupted_file(config_path);
        }
    };

    // First check: does it pass migrate_and_validate directly without needing repair?
    if let Ok((cfg, migrated)) = migrate_and_validate(json_val.clone()) {
        if migrated {
            let _ = write_config_atomic(config_path, &cfg);
            write_last_good_snapshot(config_path, &cfg);
            return (cfg, SelfHealingAction::Migrated);
        } else {
            // Clean parse: this state is proven good — refresh the golden snapshot.
            write_last_good_snapshot(config_path, &cfg);
            return (cfg, SelfHealingAction::LoadedExisting);
        }
    }

    // If migrate_and_validate failed: DO NOT discard! Repair and patch!
    match repair_and_patch_value(&json_val) {
        Ok((repaired_cfg, details)) => {
            let backup_path = backup_corrupted_file(config_path)
                .unwrap_or_else(|| config_path.with_extension("json.bak"));
            let _ = write_config_atomic(config_path, &repaired_cfg);
            eprintln!(
                "[defaults] Config repaired and patched successfully ({} issues fixed). Backup saved to {:?}",
                details.len(),
                backup_path
            );
            (
                repaired_cfg,
                SelfHealingAction::RepairedAndPatched {
                    backup_path,
                    details,
                },
            )
        }
        Err(err) => {
            eprintln!("[defaults] Deep repair failed ({err}). Recovering with defaults...");
            recover_corrupted_file(config_path)
        }
    }
}

fn recover_corrupted_file(config_path: &Path) -> (AppConfig, SelfHealingAction) {
    // Quarantine: timestamped copy, VISIBLE and editable — user evidence.
    // Never harden the quarantine; the user must be able to open it.
    let parent = config_path.parent().unwrap_or_else(|| Path::new("."));
    let stem = config_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("config.json");
    let now_epoch = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let quarantine = parent.join(format!("{stem}.broken-{now_epoch}"));
    let _ = fs::copy(config_path, &quarantine);
    let backup_path = backup_corrupted_file(config_path)
        .unwrap_or_else(|| config_path.with_extension("json.bak"));

    // Last Known Good: restore the user's own last working state, not factory zero.
    if let Some(good) = load_last_good_snapshot(config_path) {
        let _ = write_config_atomic(config_path, &good);
        eprintln!(
            "[defaults] Config unrecoverable; restored last-known-good snapshot. Quarantine: {:?}",
            quarantine
        );
        return (
            good,
            SelfHealingAction::CorruptedAndRecovered {
                backup_path: quarantine,
                restored_from_last_good: true,
            },
        );
    }

    let default_cfg = get_embedded_default_config();
    let _ = write_config_atomic(config_path, &default_cfg);
    let _ = backup_path; // legacy .bak kept alongside the timestamped quarantine

    (
        default_cfg,
        SelfHealingAction::CorruptedAndRecovered {
            backup_path: quarantine,
            restored_from_last_good: false,
        },
    )
}

/// Reset an existing configuration on disk back to factory defaults.
/// Preserves a backup of the current state before replacing.
pub fn reset_to_defaults(config_path: &Path) -> Result<AppConfig, String> {
    if config_path.exists() {
        let _ = backup_corrupted_file(config_path);
    }

    let default_cfg = get_embedded_default_config();
    write_config_atomic(config_path, &default_cfg)?;

    Ok(default_cfg)
}

pub(crate) fn migrate_and_validate(mut value: Value) -> Result<(AppConfig, bool), String> {
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
        .map_err(|e| format!("Failed to parse AppConfig: {}", e))?;

    Ok((config, migrated))
}

pub fn migrate_config(value: Value) -> Result<(AppConfig, bool), String> {
    migrate_and_validate(value)
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
    fn embedded_default_json_is_valid_and_safe() {
        let cfg = get_embedded_default_config();
        assert_eq!(cfg.schema_version, 2);
        assert_eq!(cfg.first_run, true);
        assert_eq!(cfg.engine.click_type, "single");
        assert_eq!(cfg.engine.target_cps, 10.0);
        assert!(cfg.presets.len() >= 4);
    }

    #[test]
    fn missing_file_creates_fresh_default() {
        let dir = std::env::temp_dir().join(format!("nanoclick_test_fresh_{}", std::process::id()));
        let config_file = dir.join("config.json");
        let _ = fs::remove_dir_all(&dir);

        let (cfg, action) = ensure_config_file(&config_file);
        assert_eq!(action, SelfHealingAction::CreatedFresh);
        assert!(config_file.exists());
        assert_eq!(cfg.engine.click_type, "single");
        assert_eq!(cfg.engine.target_cps, 10.0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupted_binary_creates_backup_and_recovers() {
        let dir = std::env::temp_dir().join(format!("nanoclick_test_bin_{}", std::process::id()));
        let config_file = dir.join("config.json");
        let _ = fs::create_dir_all(&dir);

        fs::write(&config_file, b"\x00\xFF\xFE\x12\x34\x56").unwrap();

        let (recovered_cfg, action) = ensure_config_file(&config_file);

        match action {
            SelfHealingAction::CorruptedAndRecovered { backup_path, restored_from_last_good } => {
                assert!(backup_path.exists());
                // No snapshot existed in this fresh dir -> factory default.
                assert!(!restored_from_last_good);
            }
            other => panic!("Expected CorruptedAndRecovered action, got {:?}", other),
        }

        assert!(config_file.exists());
        assert_eq!(recovered_cfg.engine.click_type, "single");
        assert_eq!(recovered_cfg.engine.target_cps, 10.0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_file_creates_backup_and_recovers() {
        let dir = std::env::temp_dir().join(format!("nanoclick_test_empty_{}", std::process::id()));
        let config_file = dir.join("config.json");
        let _ = fs::create_dir_all(&dir);

        fs::write(&config_file, "").unwrap();

        let (cfg, action) = ensure_config_file(&config_file);
        match action {
            SelfHealingAction::CorruptedAndRecovered { backup_path, restored_from_last_good } => {
                assert!(backup_path.exists());
                assert!(!restored_from_last_good);
            }
            other => panic!("Expected CorruptedAndRecovered for empty file, got {:?}", other),
        }
        assert_eq!(cfg.engine.target_cps, 10.0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn syntax_repair_handles_trailing_commas() {
        let raw = r#"{
            "schema_version": 2,
            "engine": {
                "target_cps": 15.0,
            },
        }"#;

        let parsed = try_repair_json_syntax(raw).expect("trailing commas should be repaired");
        let (cfg, _details) = repair_and_patch_value(&parsed).expect("repair should succeed");
        assert_eq!(cfg.engine.target_cps, 15.0);
    }

    #[test]
    fn syntax_repair_handles_unclosed_braces() {
        let raw = r#"{
            "schema_version": 2,
            "engine": {
                "target_cps": 22.5"#;

        let parsed = try_repair_json_syntax(raw).expect("unclosed braces should balance");
        let (cfg, _details) = repair_and_patch_value(&parsed).expect("repair should succeed");
        assert_eq!(cfg.engine.target_cps, 22.5);
    }

    #[test]
    fn reset_to_defaults_backs_up_and_resets() {
        let dir = std::env::temp_dir().join(format!("nanoclick_test_reset_{}", std::process::id()));
        let config_file = dir.join("config.json");
        let _ = fs::create_dir_all(&dir);

        let mut custom = get_embedded_default_config();
        custom.engine.target_cps = 77.7;
        write_config_atomic(&config_file, &custom).unwrap();

        let reset_cfg = reset_to_defaults(&config_file).expect("reset should succeed");
        assert_eq!(reset_cfg.engine.target_cps, 10.0);

        // Backup must contain custom 77.7
        let bak = config_file.with_file_name("config.json.bak");
        assert!(bak.exists());
        assert!(fs::read_to_string(&bak).unwrap().contains("77.7"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn window_position_repair_rules() {
        // Valid integers survive; garbage -> None (auto-center), never a crash.
        let raw = r#"{"ui": {"remember_window_position": true, "window_x": 320, "window_y": 240}}"#;
        let val: Value = serde_json::from_str(raw).unwrap();
        let (cfg, _) = repair_and_patch_value(&val).unwrap();
        assert!(cfg.ui.remember_window_position);
        assert_eq!(cfg.ui.window_x, Some(320));
        assert_eq!(cfg.ui.window_y, Some(240));

        // String / float / huge garbage -> None + details entry.
        let raw2 = r#"{"ui": {"window_x": "left", "window_y": 99999999}}"#;
        let val2: Value = serde_json::from_str(raw2).unwrap();
        let (cfg2, details2) = repair_and_patch_value(&val2).unwrap();
        assert_eq!(cfg2.ui.window_x, None);
        assert_eq!(cfg2.ui.window_y, None);
        assert!(details2.iter().any(|d| d.contains("window_x")));
        assert!(details2.iter().any(|d| d.contains("window_y")));

        // Missing keys -> None (fresh profiles center by default).
        let raw3 = r#"{"ui": {}}"#;
        let val3: Value = serde_json::from_str(raw3).unwrap();
        let (cfg3, _) = repair_and_patch_value(&val3).unwrap();
        assert_eq!(cfg3.ui.window_x, None);
        assert_eq!(cfg3.ui.window_y, None);
    }

    #[test]
    fn last_good_snapshot_restores_user_state_not_factory() {
        let dir = std::env::temp_dir().join(format!("nanoclick_test_lkg_{}", std::process::id()));
        let config_file = dir.join("config.json");
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::create_dir_all(&dir);

        // 1. Good boot: custom user state -> snapshot must be written + hardened.
        let mut custom = get_embedded_default_config();
        custom.hotkeys.toggle = "F9".into();
        custom.engine.target_cps = 42.0;
        write_config_atomic(&config_file, &custom).unwrap();
        let (cfg, action) = ensure_config_file(&config_file);
        assert_eq!(action, SelfHealingAction::LoadedExisting);
        assert_eq!(cfg.hotkeys.toggle, "F9");
        let snap = last_good_path(&config_file);
        assert!(snap.exists(), "last_good snapshot must exist after clean parse");
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::ffi::OsStrExt;
            use windows::Win32::Storage::FileSystem::{
                GetFileAttributesW, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_READONLY,
            };
            use windows::core::PCWSTR;
            let wide: Vec<u16> = snap
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let got = unsafe { GetFileAttributesW(PCWSTR::from_raw(wide.as_ptr())) };
            assert_ne!(got, u32::MAX, "snapshot attributes must be readable");
            assert_ne!(got & FILE_ATTRIBUTE_READONLY.0, 0, "snapshot must be readonly-hardened");
            assert_ne!(got & FILE_ATTRIBUTE_HIDDEN.0, 0, "snapshot must be hidden");
        }

        // 2. User breaks the live file (trailing dot = lexer kill).
        fs::write(&config_file, "{\"engine\": {\"target_cps\": 42.0}.").unwrap();
        let (recovered, action2) = ensure_config_file(&config_file);
        match action2 {
            SelfHealingAction::CorruptedAndRecovered { backup_path, restored_from_last_good } => {
                // Quarantine is the broken file, VISIBLE (no hardening).
                assert!(backup_path.exists(), "quarantine must exist");
                assert!(restored_from_last_good, "must restore LKG, not factory");
                #[cfg(target_os = "windows")]
                {
                    use std::os::windows::ffi::OsStrExt;
                    use windows::Win32::Storage::FileSystem::{
                        GetFileAttributesW, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_READONLY,
                    };
                    use windows::core::PCWSTR;
                    let wide: Vec<u16> = backup_path
                        .as_os_str()
                        .encode_wide()
                        .chain(std::iter::once(0))
                        .collect();
                    let got = unsafe { GetFileAttributesW(PCWSTR::from_raw(wide.as_ptr())) };
                    assert_ne!(got, u32::MAX, "quarantine attributes must be readable");
                    assert_eq!(got & FILE_ATTRIBUTE_READONLY.0, 0, "quarantine must stay writable");
                    assert_eq!(got & FILE_ATTRIBUTE_HIDDEN.0, 0, "quarantine must stay visible");
                }
            }
            other => panic!("Expected CorruptedAndRecovered with LKG, got {:?}", other),
        }
        // User state survives: toggle + CPS come back from the snapshot.
        assert_eq!(recovered.hotkeys.toggle, "F9");
        assert_eq!(recovered.engine.target_cps, 42.0);
        // Live file is fixed again.
        let live: AppConfig =
            serde_json::from_str(&fs::read_to_string(&config_file).unwrap()).unwrap();
        assert_eq!(live.hotkeys.toggle, "F9");

        // 3. Snapshot itself is broken too -> honest factory fallback.
        unharden_last_good(&snap);
        fs::write(&snap, "not json at all{{{").unwrap();
        harden_last_good(&snap);
        fs::write(&config_file, "also broken...").unwrap();
        let (fallback, action3) = ensure_config_file(&config_file);
        match action3 {
            SelfHealingAction::CorruptedAndRecovered { restored_from_last_good, .. } => {
                assert!(!restored_from_last_good, "broken snapshot must not be trusted");
            }
            other => panic!("Expected factory fallback, got {:?}", other),
        }
        assert_eq!(fallback.engine.target_cps, 10.0);

        // 4. Snapshot refresh cycle never locks us out (unharden->write->harden).
        let (cfg4, _) = ensure_config_file(&config_file);
        assert_eq!(cfg4.engine.target_cps, 10.0);
        let snap2 = last_good_path(&config_file);
        assert!(snap2.exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn repair_replaces_corrupted_field_and_preserves_valid_settings() {
        let dir = std::env::temp_dir().join(format!("nanoclick_test_repair_{}", std::process::id()));
        let config_file = dir.join("config.json");
        let _ = fs::create_dir_all(&dir);

        // User configured toggle to "F11", theme to "matrix", but engine.target_cps is broken string
        let broken_config = r#"{
            "schema_version": 2,
            "engine": {
                "target_cps": "BROKEN_STRING_NOT_NUMBER",
                "click_type": "invalid_type"
            },
            "hotkeys": {
                "toggle": "F11"
            },
            "ui": {
                "theme": "matrix"
            }
        }"#;
        fs::write(&config_file, broken_config).unwrap();

        let (cfg, action) = ensure_config_file(&config_file);

        match action {
            SelfHealingAction::RepairedAndPatched { backup_path, details } => {
                assert!(backup_path.exists());
                assert!(!details.is_empty());
            }
            other => panic!("Expected RepairedAndPatched action, got {:?}", other),
        }

        // Corrupted fields replaced with safe defaults
        assert_eq!(cfg.engine.target_cps, 10.0);
        assert_eq!(cfg.engine.click_type, "single");

        // Valid user fields are PRESERVED!
        assert_eq!(cfg.hotkeys.toggle, "F11");
        assert_eq!(cfg.ui.theme, "matrix");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn repair_sanitizes_invalid_enums_and_out_of_range_numbers() {
        let raw = r#"{
            "engine": {
                "target_cps": 99999.0,
                "button": "laser_gun",
                "repeat_mode": "forever_and_ever"
            }
        }"#;
        let val: Value = serde_json::from_str(raw).unwrap();
        let (cfg, details) = repair_and_patch_value(&val).unwrap();

        assert_eq!(cfg.engine.target_cps, 10.0);
        assert_eq!(cfg.engine.button, "left");
        assert_eq!(cfg.engine.repeat_mode, "unlimited");
        assert!(details.iter().any(|d| d.contains("target_cps")));
        assert!(details.iter().any(|d| d.contains("button")));
    }

    #[test]
    fn repair_preserves_custom_presets_and_stats() {
        let raw = r#"{
            "presets": [
                {
                    "id": "custom_sniper",
                    "name": "Custom Sniper",
                    "description": "Custom preset",
                    "icon": "🎯",
                    "target_cps": 5.0,
                    "jitter_percent": 1.0,
                    "click_limit": 50,
                    "button": "right",
                    "click_type": "single",
                    "position_mode": "cursor",
                    "fixed_x": 0,
                    "fixed_y": 0,
                    "hold_duration_ms": 0,
                    "hold_interval_ms": 0,
                    "jitter_radius_px": 0,
                    "repeat_mode": "repeat",
                    "repeat_count": 5,
                    "repeat_interval_ms": 500,
                    "start_delay_ms": 100,
                    "stop_duration_min": 0,
                    "stop_time_str": "",
                    "is_default": false,
                    "points": []
                }
            ],
            "stats": {
                "total_clicks": 12345,
                "total_active_ms": 67890,
                "total_sessions": 42,
                "presets_applied": 10,
                "max_cps": 28.5,
                "history": []
            }
        }"#;

        let val: Value = serde_json::from_str(raw).unwrap();
        let (cfg, _details) = repair_and_patch_value(&val).unwrap();

        assert_eq!(cfg.presets.len(), 1);
        assert_eq!(cfg.presets[0].id, "custom_sniper");
        assert_eq!(cfg.stats.total_clicks, 12345);
        assert_eq!(cfg.stats.total_sessions, 42);
    }

    #[test]
    fn fast_repair_performance() {
        let start = std::time::Instant::now();
        let broken = r#"{ "engine": { "target_cps": -10.0 }, "hotkeys": { "toggle": "F9" } }"#;
        for _ in 0..100 {
            let val: Value = serde_json::from_str(broken).unwrap();
            let (cfg, _) = repair_and_patch_value(&val).unwrap();
            assert_eq!(cfg.engine.target_cps, 10.0);
            assert_eq!(cfg.hotkeys.toggle, "F9");
        }
        let elapsed = start.elapsed();
        // 100 in-memory repairs should take less than 150ms (< 1.5ms per repair).
        // Wall-clock guard only: repair is pure CPU work, machine load may spike
        // right after a from-scratch build (cold dylib pages, AV scan). The real
        // assertion is correctness above; this just catches 10x regressions.
        assert!(elapsed.as_millis() < 150, "100 in-memory repairs took {:?}", elapsed);
    }
}
