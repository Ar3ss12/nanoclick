use crate::config::Config;
use crate::core::action::MouseButton;
use crate::guard::{AppFilter, FocusWatchStop, ForegroundCache, TypingGuard, GUARD_POLL_MS};
use crate::platform::{self, backend::ClickSpec, NativeEventHandle, PlatformTimer};
use rand::Rng;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

#[derive(Debug, Clone, serde::Serialize)]
pub struct StatusUpdate {
    pub active: bool,
    pub mode: String, // "autoclicker" or "work"
    pub clicks_done: u32,
    pub cps: f64,
    pub status_text: String,
}

// ── Toggle diagnostics ring buffer (v4.2 hardening) ──────────
// Replaces file I/O in `hotkey_toggle()`. The toggle path is called from the
// global hotkey listener thread every R-press; file I/O + eprint!() added a
// measurable Mutex+Write cost (5-50 ms). All diagnostics land here as zero
// syscall cost; tests / debug command can dump the buffer.
static TOGGLE_DIAG: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

fn toggle_diag_push(line: impl Into<String>) {
    if let Ok(mut q) = TOGGLE_DIAG.lock() {
        if q.len() >= 128 {
            q.pop_front();
        }
        q.push_back(line.into());
    }
}

/// Dump and clear the toggle diagnostic buffer. Read by the
/// `dump_input_diagnostics` command (Settings → Dump) and by the tests.
pub fn toggle_diag_dump() -> Vec<String> {
    if let Ok(mut q) = TOGGLE_DIAG.lock() {
        return q.drain(..).collect();
    }
    Vec::new()
}

/// Focus Guard state machine — owned by the click-loop thread, toggled ON
/// by the UI (`pause_on_focus_loss`). Contract:
/// * `arm()` at run start: remember the foreground exe AS THE ALLOWED ONE.
///   Our own window is a REAL baseline (not `None`): leaving NanoClick via
///   Alt+Tab / click-away must stop; returning TO NanoClick never pauses.
/// * `poll()` on each loop pass: foreground CHANGED to a different process
///   → auto-pause. `None` (lock screen, elevated) fails OPEN; our own exe
///   fails open too (it is the control panel, not a click target).
/// * `disarm()` on run end / when the flag flips OFF. A plain config save
///   (e.g. the stats 5 s flush) must NOT disarm a live session — otherwise
///   the first save after START silently disables the guard.
/// * No baseline yet (`None` in the slot) → fail-open, we never pause on
///   what we cannot measure. Zero syscalls while disabled.
pub struct FocusGuard {
    enabled: AtomicBool,
    session_exe: StdMutex<Option<String>>,
}

impl FocusGuard {
    pub fn new(enabled: bool) -> Self {
        FocusGuard {
            enabled: AtomicBool::new(enabled),
            session_exe: StdMutex::new(None),
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
        if !enabled {
            if let Ok(mut g) = self.session_exe.lock() {
                *g = None;
            }
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Run start: snapshot the current foreground exe as the allowed one.
    /// `None` (unresolvable at arm time) does NOT overwrite an existing
    /// baseline — poll() fails open until a real exe is seen (we never pause
    /// on what we cannot measure). Arming the same exe twice is a no-op.
    pub fn arm(&self, exe: Option<String>) {
        if !self.is_enabled() {
            return;
        }
        let Some(exe) = exe else {
            return;
        };
        if let Ok(mut g) = self.session_exe.lock() {
            *g = Some(exe);
        }
    }

    /// Disarm at run end / flag-off (no stale session across runs).
    pub fn disarm(&self) {
        if let Ok(mut g) = self.session_exe.lock() {
            *g = None;
        }
    }

    /// `true` when the foreground app changed away from the session exe.
    /// Fail-open: `None` (unresolvable foreground: lock screen, elevated)
    /// and our own exe (control panel) never pause. `explorer.exe` (the
    /// Alt+Tab switcher itself) DOES pause — user decision, no grace window.
    pub fn should_pause(&self, current: Option<&str>) -> bool {
        if !self.is_enabled() {
            return false;
        }
        let Some(cur) = current else {
            return false; // fail-open
        };
        if crate::platform::is_own_exe(cur) {
            return false; // back in NanoClick — control panel, never a pause
        }
        match self.session_exe.lock() {
            Ok(guard) => match guard.as_deref() {
                // Session exe unknown (resolution failed at arm): fail-open
                // until we HAVE a baseline — otherwise first poll pauses.
                // Note: the baseline may itself be our own exe (start from
                // the UI button) — `cur` above is already known non-own, so
                // any mismatch, including explorer.exe, pauses.
                None => false,
                Some(allowed) => !cur.eq_ignore_ascii_case(allowed),
            },
            Err(_) => false,
        }
    }
}

pub struct ClickScheduler {
    active: Arc<AtomicBool>,
    mode_autoclicker: Arc<AtomicBool>,
    clicks_done: Arc<AtomicU32>,
    cps_raw: Arc<AtomicU64>,
    random_pct_raw: Arc<AtomicU64>,
    outlier_prob_raw: Arc<AtomicU64>,
    technique: Arc<Mutex<String>>,
    limit: Arc<AtomicU32>,
    button: Arc<Mutex<String>>,
    click_type: Arc<Mutex<String>>,
    position_mode: Arc<Mutex<String>>,
    fixed_x: Arc<AtomicU32>,
    fixed_y: Arc<AtomicU32>,
    repeat_mode: Arc<Mutex<String>>,
    repeat_count: Arc<AtomicU32>,
    hold_duration_ms: Arc<AtomicU64>,
    hold_interval_ms: Arc<AtomicU64>,
    repeat_interval_ms: Arc<AtomicU64>,
    jitter_radius_px: Arc<AtomicU32>,
    start_delay_ms: Arc<AtomicU64>,
    stop_duration_ms: Arc<AtomicU64>,
    stop_time_epoch_sec: Arc<AtomicI64>,
    /// WHICH auto-stop trigger the click loop may act on: `"none" | "duration" |
    /// "wallclock"`. A `Mutex<String>` like `repeat_mode`, snapshotted ONCE per
    /// run, because two non-zero deadlines are ambiguous: without it the loop
    /// would stop on whichever came first and the "only one runs at a time"
    /// contract the UI shows would be a lie.
    stop_mode: Arc<Mutex<String>>,
    stop_event: Arc<Mutex<Option<NativeEventHandle>>>,
    hotkey_toggle: Arc<Mutex<String>>,
    hotkey_mode_switch: Arc<Mutex<String>>,
    hotkey_emergency_stop: Arc<Mutex<String>>,
    hotkey_speed_up: Arc<Mutex<String>>,
    hotkey_slow_down: Arc<Mutex<String>>,
    hotkey_capture_pos: Arc<Mutex<String>>,
    hotkey_record_toggle: Arc<AtomicBool>,
    hotkeys_version: Arc<AtomicU64>,
    hotkey_record: Arc<Mutex<String>>,
    preset_hotkeys: Arc<Mutex<Vec<String>>>,
    hotkey_debounce_ms: Arc<AtomicU32>,
    /// GUI Lock Duration (ms) — the Start button's anti-double-click window.
    /// Stored (not hardcoded) so `get_config()` never lies about the value the
    /// user set in Settings (it used to answer a constant 1500).
    gui_lock_ms: Arc<AtomicU64>,
    /// Optional multi-point sequence. When non-empty the click loop
    /// visits each point in order with the per-point delay.
    sequence_points: Arc<Mutex<Vec<crate::config_manager::SequencePoint>>>,
    last_toggle_instant: Arc<Mutex<Option<std::time::Instant>>>,
    /// Optional image trigger copied from config (used by the click loop).
    image_trigger: Arc<Mutex<Option<crate::config_manager::ImageTrigger>>>,
    /// Set to true by the image-trigger poller to ask the click loop
    /// to stop on the next iteration.
    image_trigger_should_stop: Arc<AtomicBool>,
    visual_ripple: Arc<AtomicBool>,
    /// Smart Guard: freezes clicks briefly while the user types text.
    typing_guard: Arc<TypingGuard>,
    /// Smart Guard: restricts clicking to (or away from) selected apps.
    app_filter: Arc<AppFilter>,
    /// Smart Guard: auto-pause when the foreground app CHANGES mid-run
    /// (Alt+Tab, mouse click on another window, Win key, system toast).
    /// State machine owned by the click loop; UI only writes the flag.
    focus_guard: Arc<FocusGuard>,
    /// ID of the currently active preset running on this scheduler, if any.
    current_running_preset: Arc<Mutex<Option<String>>>,
    /// Presets snapshot used for instant hotkey preemption.
    presets: Arc<Mutex<Vec<crate::config_manager::PresetItem>>>,
}

/// What a single toggle press must do, resolved **without side effects**.
///
/// This enum is the contract the low-level keyboard hook relies on: the hook
/// only decides *what* to do (it has no access to worker state beyond the
/// scheduler), and `ClickScheduler` applies it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleOutcome {
    /// Idle → start clicking.
    Start,
    /// Running → stop clicking. Always honoured (see `decide_toggle`).
    Stop,
    /// Work Mode is engaged: the toggle is deliberately inert.
    IgnoredWorkMode,
    /// Start refused: the user is still typing (`go rush B` and the `r` key).
    BlockedByTyping,
    /// Start refused: an accepted toggle happened moments ago (fast double tap).
    BlockedByDebounce,
}

/// Gate order for one toggle press. This is the *only* place where the rules
/// live, so the hook thread, the UI button and the tests cannot drift apart.
///
/// 1. **STOP absolute:** when the clicker runs, the press stops it. Not
///    debounced, not typing-gated, not mode-gated. A user who wants the
///    clicking to stop must never be refused — this is exactly the rule that
///    was broken in the "still clicking after the second tap" reports.
/// 2. **Work Mode:** refuses to start (safety lock).
/// 3. **Typing lockout:** refuses to start (letter inside a word).
/// 4. **Debounce:** refuses to start (the second tap of a double tap).
/// 5. Otherwise: start.
///
/// Pure by construction — no atomics, no locks, no emits, no worker spawn,
/// so the whole decision matrix is unit-testable.
pub fn decide_toggle(
    active: bool,
    work_mode: bool,
    typing_locked: bool,
    debounce_ok: bool,
) -> ToggleOutcome {
    if active {
        return ToggleOutcome::Stop;
    }
    if work_mode {
        return ToggleOutcome::IgnoredWorkMode;
    }
    if typing_locked {
        return ToggleOutcome::BlockedByTyping;
    }
    if !debounce_ok {
        return ToggleOutcome::BlockedByDebounce;
    }
    ToggleOutcome::Start
}

impl ClickScheduler {
    pub fn new() -> Self {
        let initial_cfg = Config::default();
        ClickScheduler {
            active: Arc::new(AtomicBool::new(false)),
            mode_autoclicker: Arc::new(AtomicBool::new(initial_cfg.active_mode == "autoclicker")),
            clicks_done: Arc::new(AtomicU32::new(0)),
            cps_raw: Arc::new(AtomicU64::new(initial_cfg.cps.to_bits())),
            random_pct_raw: Arc::new(AtomicU64::new(initial_cfg.random_percent.to_bits())),
            outlier_prob_raw: Arc::new(AtomicU64::new(
                crate::config::normalize_outlier_prob(initial_cfg.outlier_prob).to_bits(),
            )),
            technique: Arc::new(Mutex::new(crate::config::normalize_technique(
                &initial_cfg.technique,
            ))),
            limit: Arc::new(AtomicU32::new(initial_cfg.click_limit)),
            button: Arc::new(Mutex::new(initial_cfg.button)),
            click_type: Arc::new(Mutex::new(initial_cfg.click_type)),
            position_mode: Arc::new(Mutex::new(initial_cfg.position_mode)),
            fixed_x: Arc::new(AtomicU32::new(initial_cfg.fixed_x as u32)),
            fixed_y: Arc::new(AtomicU32::new(initial_cfg.fixed_y as u32)),
            repeat_mode: Arc::new(Mutex::new(initial_cfg.repeat_mode)),
            repeat_count: Arc::new(AtomicU32::new(initial_cfg.repeat_count)),
            hold_duration_ms: Arc::new(AtomicU64::new(initial_cfg.hold_duration_ms)),
            hold_interval_ms: Arc::new(AtomicU64::new(initial_cfg.hold_interval_ms)),
            repeat_interval_ms: Arc::new(AtomicU64::new(initial_cfg.repeat_interval_ms)),
            jitter_radius_px: Arc::new(AtomicU32::new(initial_cfg.jitter_radius_px)),
            start_delay_ms: Arc::new(AtomicU64::new(initial_cfg.start_delay_ms)),
            stop_duration_ms: Arc::new(AtomicU64::new(initial_cfg.stop_duration_ms)),
            stop_time_epoch_sec: Arc::new(AtomicI64::new(initial_cfg.stop_time_epoch_sec)),
            stop_mode: Arc::new(Mutex::new(initial_cfg.stop_mode)),
            stop_event: Arc::new(Mutex::new(platform::create_stop_event())),
            hotkey_toggle: Arc::new(Mutex::new(initial_cfg.hotkey_toggle)),
            hotkey_mode_switch: Arc::new(Mutex::new(initial_cfg.hotkey_mode_switch)),
            hotkey_emergency_stop: Arc::new(Mutex::new(initial_cfg.hotkey_emergency_stop)),
            hotkey_speed_up: Arc::new(Mutex::new(initial_cfg.hotkey_speed_up)),
            hotkey_slow_down: Arc::new(Mutex::new(initial_cfg.hotkey_slow_down)),
            hotkey_capture_pos: Arc::new(Mutex::new(initial_cfg.hotkey_capture_pos)),
            hotkey_record_toggle: Arc::new(AtomicBool::new(initial_cfg.hotkey_record_toggle)),
            hotkey_record: Arc::new(Mutex::new(initial_cfg.hotkey_record)),
            preset_hotkeys: Arc::new(Mutex::new(initial_cfg.hotkey_preset_slots.clone())),
            hotkey_debounce_ms: Arc::new(AtomicU32::new(initial_cfg.hotkey_debounce_ms)),
            gui_lock_ms: Arc::new(AtomicU64::new(initial_cfg.gui_lock_ms)),
            sequence_points: Arc::new(Mutex::new(initial_cfg.sequence_points.clone())),
            last_toggle_instant: Arc::new(Mutex::new(None)),
            hotkeys_version: Arc::new(AtomicU64::new(1)),
            image_trigger: Arc::new(Mutex::new(None)),
            image_trigger_should_stop: Arc::new(AtomicBool::new(false)),
            visual_ripple: Arc::new(AtomicBool::new(initial_cfg.visual_ripple)),
            typing_guard: Arc::new(TypingGuard::new(initial_cfg.typing_pause_ms)),
            app_filter: Arc::new(AppFilter::new(
                &initial_cfg.app_filter_mode,
                &initial_cfg.app_filter_list,
            )),
            focus_guard: Arc::new(FocusGuard::new(initial_cfg.pause_on_focus_loss)),
            current_running_preset: Arc::new(Mutex::new(None)),
            presets: Arc::new(Mutex::new(initial_cfg.presets.clone())),
        }
    }

    /// Typing Guard handle. The low-level keyboard hook calls `note()` for
    /// every real text key-down and immediately stops a running clicker;
    /// `set_active(true)` is also gated on the lockout so the clicker can
    /// never be (re)started while the user is typing.
    pub fn typing_guard(&self) -> &TypingGuard {
        &self.typing_guard
    }

    /// Focus Guard handle (written by config saves, read by the click loop).
    pub fn focus_guard(&self) -> &FocusGuard {
        &self.focus_guard
    }

    /// App-filter handle (read by the click loop, written by config saves).
    pub fn app_filter(&self) -> &AppFilter {
        &self.app_filter
    }

    pub fn get_config(&self) -> Config {
        let is_auto = self.mode_autoclicker.load(Ordering::Relaxed);
        Config {
            cps: f64::from_bits(self.cps_raw.load(Ordering::Relaxed)),
            random_percent: f64::from_bits(self.random_pct_raw.load(Ordering::Relaxed)),
            outlier_prob: f64::from_bits(self.outlier_prob_raw.load(Ordering::Relaxed)),
            technique: self.technique.lock().unwrap().clone(),
            click_limit: self.limit.load(Ordering::Relaxed),
            button: self.button.lock().unwrap().clone(),
            click_type: self.click_type.lock().unwrap().clone(),
            position_mode: self.position_mode.lock().unwrap().clone(),
            fixed_x: self.fixed_x.load(Ordering::Relaxed) as i32,
            fixed_y: self.fixed_y.load(Ordering::Relaxed) as i32,
            repeat_mode: self.repeat_mode.lock().unwrap().clone(),
            repeat_count: self.repeat_count.load(Ordering::Relaxed),
            hold_duration_ms: self.hold_duration_ms.load(Ordering::Relaxed),
            hold_interval_ms: self.hold_interval_ms.load(Ordering::Relaxed),
            repeat_interval_ms: self.repeat_interval_ms.load(Ordering::Relaxed),
            jitter_radius_px: self.jitter_radius_px.load(Ordering::Relaxed),
            hotkey_toggle: self.hotkey_toggle.lock().unwrap().clone(),
            hotkey_mode_switch: self.hotkey_mode_switch.lock().unwrap().clone(),
            hotkey_emergency_stop: self.hotkey_emergency_stop.lock().unwrap().clone(),
            hotkey_speed_up: self.hotkey_speed_up.lock().unwrap().clone(),
            hotkey_slow_down: self.hotkey_slow_down.lock().unwrap().clone(),
            hotkey_capture_pos: self.hotkey_capture_pos.lock().unwrap().clone(),
            hotkey_record_toggle: self.hotkey_record_toggle.load(Ordering::Relaxed),
            hotkey_record: self.hotkey_record.lock().unwrap().clone(),
            hotkey_preset_slots: self.preset_hotkeys.lock().unwrap().clone(),
            start_delay_ms: 0,
            stop_duration_ms: 0,
            stop_time_epoch_sec: 0,
            stop_mode: self.stop_mode.lock().unwrap().clone(),
            gui_lock_ms: self.gui_lock_ms.load(Ordering::Relaxed),
            hotkey_debounce_ms: self.hotkey_debounce_ms.load(Ordering::Relaxed),
            active_mode: if is_auto {
                "autoclicker".into()
            } else {
                "work".into()
            },
            sequence_points: self.sequence_points.lock().unwrap().clone(),
            visual_ripple: self.visual_ripple.load(Ordering::Relaxed),
            typing_pause_ms: self.typing_guard.pause_ms(),
            app_filter_mode: self.app_filter.mode().as_config_str().to_string(),
            app_filter_list: self.app_filter.list(),
            pause_on_focus_loss: self.focus_guard.is_enabled(),
            presets: self.presets.lock().unwrap().clone(),
        }
    }

    pub fn set_config(&self, cfg: Config) {
        self.visual_ripple.store(cfg.visual_ripple, Ordering::Relaxed);
        self.cps_raw.store(cfg.cps.to_bits(), Ordering::Relaxed);
        self.random_pct_raw
            .store(cfg.random_percent.to_bits(), Ordering::Relaxed);
        self.outlier_prob_raw.store(
            crate::config::normalize_outlier_prob(cfg.outlier_prob).to_bits(),
            Ordering::Relaxed,
        );
        *self.technique.lock().unwrap() = crate::config::normalize_technique(&cfg.technique);
        self.limit.store(cfg.click_limit, Ordering::Relaxed);
        *self.button.lock().unwrap() = cfg.button;
        *self.click_type.lock().unwrap() = cfg.click_type;
        *self.position_mode.lock().unwrap() = cfg.position_mode;
        self.fixed_x.store(cfg.fixed_x as u32, Ordering::Relaxed);
        self.fixed_y.store(cfg.fixed_y as u32, Ordering::Relaxed);
        *self.repeat_mode.lock().unwrap() = cfg.repeat_mode;
        self.repeat_count.store(cfg.repeat_count, Ordering::Relaxed);
        self.hold_duration_ms
            .store(cfg.hold_duration_ms, Ordering::Relaxed);
        self.hold_interval_ms
            .store(cfg.hold_interval_ms, Ordering::Relaxed);
        self.repeat_interval_ms
            .store(cfg.repeat_interval_ms, Ordering::Relaxed);
        self.jitter_radius_px
            .store(cfg.jitter_radius_px, Ordering::Relaxed);
        self.start_delay_ms
            .store(cfg.start_delay_ms, Ordering::Relaxed);
        self.stop_duration_ms.store(
            cfg.stop_duration_ms.min(crate::config::MAX_STOP_DURATION_MS),
            Ordering::Relaxed,
        );
        self.stop_time_epoch_sec
            .store(cfg.stop_time_epoch_sec, Ordering::Relaxed);
        // Normalized HERE as well: `set_config` is not only fed by `Config::from`
        // — `Config::default()` and the boot path also reach it, and an
        // unrecognized string must never arm a timer the UI cannot show.
        *self.stop_mode.lock().unwrap() = crate::config::normalize_stop_mode(&cfg.stop_mode);
        self.mode_autoclicker
            .store(cfg.active_mode == "autoclicker", Ordering::Relaxed);
        *self.hotkey_toggle.lock().unwrap() = cfg.hotkey_toggle;
        *self.hotkey_mode_switch.lock().unwrap() = cfg.hotkey_mode_switch;
        *self.hotkey_emergency_stop.lock().unwrap() = cfg.hotkey_emergency_stop;
        *self.hotkey_speed_up.lock().unwrap() = cfg.hotkey_speed_up;
        *self.hotkey_slow_down.lock().unwrap() = cfg.hotkey_slow_down;
        *self.hotkey_capture_pos.lock().unwrap() = cfg.hotkey_capture_pos;
        self.hotkey_record_toggle
            .store(cfg.hotkey_record_toggle, Ordering::Relaxed);
        *self.hotkey_record.lock().unwrap() = cfg.hotkey_record;
        *self.preset_hotkeys.lock().unwrap() = cfg.hotkey_preset_slots;
        *self.presets.lock().unwrap() = cfg.presets;
        *self.sequence_points.lock().unwrap() = cfg.sequence_points.clone();
        // Engine timings that the hotkey layer reads through `get_config()`.
        // `hotkey_debounce_ms` was NEVER stored here — the "Toggle Response
        // Time" slider only took effect after a restart, because the snapshot
        // the hook re-parses kept the stale value.
        self.hotkey_debounce_ms
            .store(cfg.hotkey_debounce_ms, Ordering::Relaxed);
        self.gui_lock_ms.store(cfg.gui_lock_ms, Ordering::Relaxed);
        // Smart Guard state (typing freeze window + app/window scope).
        self.typing_guard.set_pause_ms(cfg.typing_pause_ms);
        self.app_filter
            .set(&cfg.app_filter_mode, &cfg.app_filter_list);
        // Focus Guard: sync the enabled flag. Disarm ONLY on a real flag flip:
        // a plain config save (stats flush every ~5 s while clicking) must
        // keep the live session baseline — otherwise the first save after
        // START silently disables the guard. set_enabled(false) already
        // clears the baseline; the explicit disarm covers OFF->ON (stale).
        let focus_was = self.focus_guard.is_enabled();
        self.focus_guard.set_enabled(cfg.pause_on_focus_loss);
        if focus_was != cfg.pause_on_focus_loss {
            self.focus_guard.disarm();
        }
        // Signal the hotkey listener that bindings changed so it re-parses
        // them once instead of diffing string snapshots on every poll.
        self.hotkeys_version.fetch_add(1, Ordering::Release);
    }

    /// Replace the active image trigger. Pass None to clear.
    pub fn set_image_trigger(&self, trigger: Option<crate::config_manager::ImageTrigger>) {
        *self.image_trigger.lock().unwrap() = trigger;
        self.image_trigger_should_stop
            .store(false, Ordering::Relaxed);
    }

    /// Update stored presets in the scheduler and bump the hotkey version so
    /// the hotkey listener re-parses preset bindings.
    pub fn update_presets(&self, presets: Vec<crate::config_manager::PresetItem>) {
        *self.presets.lock().unwrap() = presets;
        self.hotkeys_version.fetch_add(1, Ordering::Release);
    }

    /// Monotonic counter bumped every time hotkey bindings are updated.
    /// The listener compares this against its cached version to decide
    /// whether re-parsing is needed (parse-once-per-change contract).
    pub fn hotkeys_version(&self) -> u64 {
        self.hotkeys_version.load(Ordering::Acquire)
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    pub fn is_autoclicker_mode(&self) -> bool {
        self.mode_autoclicker.load(Ordering::Relaxed)
    }

    pub fn toggle_mode(&self, app_handle: Option<&AppHandle>) -> String {
        let prev = self.mode_autoclicker.load(Ordering::Relaxed);
        let new_mode = !prev;
        self.mode_autoclicker.store(new_mode, Ordering::Relaxed);
        if !new_mode && self.is_active() {
            self.set_active(false, app_handle);
        }

        let mode_str = if new_mode { "autoclicker" } else { "work" };
        if let Some(ref app) = app_handle {
            let _ = app.emit(
                "status-update",
                StatusUpdate {
                    active: self.is_active(),
                    mode: mode_str.to_string(),
                    clicks_done: self.get_clicks_done(),
                    cps: f64::from_bits(self.cps_raw.load(Ordering::Relaxed)),
                    status_text: if new_mode {
                        "AUTOCLICKER MODE".into()
                    } else {
                        "WORK MODE (PAUSED)".into()
                    },
                },
            );
        }
        mode_str.to_string()
    }

    /// Hotkey handler — used by the global keyboard event listener.
    ///
    /// Behaviour (mirrors the user spec: "R key starts/stops autoclicker"):
    ///   - From `work` mode → ignored; Work Mode is a safety lock.
    ///   - From `autoclicker` mode, currently active → stop clicking.
    ///   - From `autoclicker` mode, currently idle → start clicking.

    /// Returns true if a toggle should be applied; updates the timestamp.
    /// Returns false (and leaves the old timestamp) if the call is still
    /// within the debounce window of the previous accepted toggle. The
    /// helper is split out so it can be unit-tested without touching the
    /// full platform init.
    fn toggle_debounce_check(
        last: &Arc<Mutex<Option<std::time::Instant>>>,
        debounce_ms: u32,
    ) -> bool {
        let mut guard = last.lock().unwrap();
        if let Some(t) = *guard {
            if t.elapsed().as_millis() < debounce_ms as u128 {
                return false;
            }
        }
        *guard = Some(std::time::Instant::now());
        true
    }

    /// Free helper: stale-held cleanup decision. A key press is considered
    /// "fresh" when the physical key is up even if the listener missed the
    /// key-up event (channel/buffer overflow during fast tapping).
    #[cfg(test)]
    fn stale_held_check(physically_down: bool, is_in_held: bool) -> bool {
        is_in_held && !physically_down
    }
    pub fn hotkey_toggle(&self, app_handle: Option<&AppHandle>) -> String {
        // ── One pure decision, then apply it ──────────────────────────────
        // The whole gate order lives in `decide_toggle` and is unit-tested:
        // STOP absolute → Work Mode → typing lockout → debounce → start.
        let was_active = self.is_active();
        let work_mode = !self.is_autoclicker_mode();
        let typing_locked = !self.typing_allows_start();
        // The debounce window is consulted ONLY on the start path: stopping
        // must never touch it ("stop always wins").
        let debounce_ok = was_active
            || Self::toggle_debounce_check(
                &self.last_toggle_instant,
                self.hotkey_debounce_ms.load(Ordering::Relaxed),
            );
        let outcome = decide_toggle(was_active, work_mode, typing_locked, debounce_ok);
        let mode_str = if work_mode { "work" } else { "autoclicker" };

        match outcome {
            ToggleOutcome::BlockedByDebounce => {
                toggle_diag_push("[Hotkeys] toggle ignored: debounce window open");
                return mode_str.into();
            }
            ToggleOutcome::BlockedByTyping => {
                toggle_diag_push("[Hotkeys] toggle ignored: user is typing");
                return mode_str.into();
            }
            ToggleOutcome::IgnoredWorkMode => {
                toggle_diag_push("[Hotkeys] toggle ignored: work mode active");
                self.set_active(false, app_handle);
                if let Some(ref app) = app_handle {
                    let _ = app.emit(
                        "status-update",
                        StatusUpdate {
                            active: false,
                            mode: "work".into(),
                            clicks_done: self.get_clicks_done(),
                            cps: f64::from_bits(self.cps_raw.load(Ordering::Relaxed)),
                            status_text: "WORK MODE (PAUSED)".into(),
                        },
                    );
                }
                return "work".into();
            }
            // START and STOP fall through to the single apply point below.
            ToggleOutcome::Start | ToggleOutcome::Stop => {}
        }

        // State-based application (not blind inversion): the action was
        // resolved from live atomics, so a duplicated event can never flip
        // the clicker twice.
        let new_active = outcome == ToggleOutcome::Start;

        toggle_diag_push(format!(
            "[Hotkeys] toggle prev_active={was_active} new_active={new_active}"
        ));

        self.set_active(new_active, app_handle);

        if let Some(ref app) = app_handle {
            let _ = app.emit(
                "status-update",
                StatusUpdate {
                    active: self.is_active(),
                    mode: mode_str.to_string(),
                    clicks_done: self.get_clicks_done(),
                    cps: f64::from_bits(self.cps_raw.load(Ordering::Relaxed)),
                    status_text: if new_active {
                        "RUNNING".into()
                    } else {
                        "IDLE".into()
                    },
                },
            );
        }
        mode_str.to_string()
    }

    /// TYPING KILL-SWITCH decision: may the clicker be (re)started right now?
    /// Pure helper so the rule is unit-testable without spawning workers.
    fn typing_allows_start(&self) -> bool {
        !self.typing_guard.hotkeys_locked()
    }

    /// Apply a requested state. **Returns what really happened**, because
    /// callers outside the click worker (the tray menu, the UI toggle command)
    /// have to report that value to the page: `Start` versus
    /// `BlockedByTyping`/`IgnoredWorkMode` is the difference between a button
    /// that works and a button that lies about it.
    pub fn set_active(&self, active: bool, app_handle: Option<&AppHandle>) -> ToggleOutcome {
        // TYPING KILL-SWITCH + WORK MODE: starting the clicker while the user
        // is typing (the `r` in `go rush B`) or while Work Mode is engaged is
        // forbidden — central choke point so every activation path (hotkey,
        // UI button, preset restore, mode switch) inherits the block.
        // Stopping ALWAYS succeeds.
        if active {
            let outcome = decide_toggle(
                false,
                !self.is_autoclicker_mode(),
                !self.typing_allows_start(),
                true,
            );
            if outcome != ToggleOutcome::Start {
                toggle_diag_push(match outcome {
                    ToggleOutcome::BlockedByTyping => "[Hotkeys] start blocked: user is typing",
                    ToggleOutcome::IgnoredWorkMode => "[Hotkeys] start blocked: work mode active",
                    _ => "[Hotkeys] start blocked",
                });
                return outcome;
            }
        }

        let was_active = self.active.swap(active, Ordering::Relaxed);
        if active && !was_active {
            // ── BUG FIX ─────────────────────────────────────────────
            // The `stop_event` handle is shared across every worker run. If we
            // just stopped, it still carries `true` and the next worker will
            // see the stale signal on its very first `wait_until` poll and
            // exit immediately after 1 click. Reset it before spawning.
            if let Some(ref h) = *self.stop_event.lock().unwrap() {
                h.store(false, Ordering::Release);
            }
            self.clicks_done.store(0, Ordering::Relaxed);
            self.spawn_worker(app_handle.cloned());
        } else if !active && was_active {
            *self.current_running_preset.lock().unwrap() = None;
            let handle = self.stop_event.lock().unwrap().clone();
            platform::signal_stop_event(handle);
        }

        if active {
            ToggleOutcome::Start
        } else {
            ToggleOutcome::Stop
        }
    }

    /// Update internal scheduler parameters from a preset specification.
    pub fn apply_preset_fields_to_scheduler(&self, preset: &crate::config_manager::PresetItem) {
        self.cps_raw.store(preset.target_cps.to_bits(), Ordering::Relaxed);
        self.random_pct_raw
            .store(preset.jitter_percent.to_bits(), Ordering::Relaxed);
        self.outlier_prob_raw.store(
            crate::config::normalize_outlier_prob(preset.outlier_prob).to_bits(),
            Ordering::Relaxed,
        );
        *self.technique.lock().unwrap() =
            crate::config::normalize_technique(&preset.technique);
        self.limit.store(preset.click_limit, Ordering::Relaxed);
        *self.button.lock().unwrap() = preset.button.clone();
        *self.click_type.lock().unwrap() = preset.click_type.clone();
        *self.position_mode.lock().unwrap() = preset.position_mode.clone();
        self.fixed_x.store(preset.fixed_x as u32, Ordering::Relaxed);
        self.fixed_y.store(preset.fixed_y as u32, Ordering::Relaxed);
        *self.repeat_mode.lock().unwrap() = preset.repeat_mode.clone();
        self.repeat_count.store(preset.repeat_count, Ordering::Relaxed);
        self.hold_duration_ms
            .store(preset.hold_duration_ms, Ordering::Relaxed);
        self.hold_interval_ms
            .store(preset.hold_interval_ms, Ordering::Relaxed);
        self.repeat_interval_ms
            .store(preset.repeat_interval_ms, Ordering::Relaxed);
        self.jitter_radius_px
            .store(preset.jitter_radius_px, Ordering::Relaxed);
        self.start_delay_ms
            .store(preset.start_delay_ms, Ordering::Relaxed);
        let stop_dur_ms = if preset.stop_duration_ms > 0 {
            preset.stop_duration_ms
        } else {
            (preset.stop_duration_min as u64) * 60_000
        };
        self.stop_duration_ms.store(
            stop_dur_ms.min(crate::config::MAX_STOP_DURATION_MS),
            Ordering::Relaxed,
        );
        let stop_time_sec = crate::config::parse_stop_time_str_next(&preset.stop_time_str);
        self.stop_time_epoch_sec.store(stop_time_sec, Ordering::Relaxed);
        *self.stop_mode.lock().unwrap() =
            crate::config::resolve_stop_mode(&preset.stop_mode, stop_dur_ms, stop_time_sec);
        *self.sequence_points.lock().unwrap() = preset.points.clone();
    }

    /// Hotkey handler for a specific preset ID.
    ///
    /// Preemption behavior:
    /// - If currently idle: loads preset parameters, starts clicking.
    /// - If currently running THIS SAME preset: toggles off (stops).
    /// - If currently running ANOTHER preset: preempts immediately!
    ///   Signals old worker to stop without blocking (.join is forbidden),
    ///   replaces parameters with new preset spec, creates a fresh stop event,
    ///   and spawns the new worker instantly (< 1 ms latency).
    pub fn activate_preset_hotkey(
        &self,
        preset_id: &str,
        app_handle: Option<&AppHandle>,
    ) -> ToggleOutcome {
        let is_running = self.is_active();
        let cur_preset = self.current_running_preset.lock().unwrap().clone();

        if is_running {
            if cur_preset.as_deref() == Some(preset_id) {
                // Same preset pressed again -> toggle stop
                toggle_diag_push(format!("[Hotkeys] preset {preset_id} toggle stop"));
                self.set_active(false, app_handle);
                *self.current_running_preset.lock().unwrap() = None;
                if let Some(app) = app_handle {
                    self.emit_status_now(app, "IDLE");
                }
                return ToggleOutcome::Stop;
            }

            // Preemption: another preset was running!
            toggle_diag_push(format!(
                "[Hotkeys] preset preemption from {:?} to {preset_id}",
                cur_preset
            ));
            let target_preset = self
                .presets
                .lock()
                .unwrap()
                .iter()
                .find(|p| p.id == preset_id)
                .cloned();

            if let Some(preset) = target_preset {
                // Signal stop event to old worker (lock-free flag, zero blocking)
                let old_stop_handle = self.stop_event.lock().unwrap().clone();
                platform::signal_stop_event(old_stop_handle);

                // Create fresh stop event for the new worker
                let new_stop_handle = platform::create_stop_event();
                *self.stop_event.lock().unwrap() = new_stop_handle;

                // Swap parameters on the fly
                self.apply_preset_fields_to_scheduler(&preset);
                *self.current_running_preset.lock().unwrap() = Some(preset_id.to_string());
                self.clicks_done.store(0, Ordering::Relaxed);

                // Spawn new worker immediately without any thread join
                self.spawn_worker(app_handle.cloned());

                if let Some(app) = app_handle {
                    self.emit_status_now(app, "RUNNING");
                    let _ = app.emit("preset-activated", preset_id);
                }
                return ToggleOutcome::Start;
            }
        } else {
            // Idle -> check typing guard / work mode
            let outcome = decide_toggle(
                false,
                !self.is_autoclicker_mode(),
                !self.typing_allows_start(),
                true,
            );
            if outcome != ToggleOutcome::Start {
                toggle_diag_push(format!("[Hotkeys] preset {preset_id} start blocked"));
                return outcome;
            }

            let target_preset = self
                .presets
                .lock()
                .unwrap()
                .iter()
                .find(|p| p.id == preset_id)
                .cloned();

            if let Some(preset) = target_preset {
                self.apply_preset_fields_to_scheduler(&preset);
                *self.current_running_preset.lock().unwrap() = Some(preset_id.to_string());
                let res = self.set_active(true, app_handle);
                if res == ToggleOutcome::Start {
                    if let Some(app) = app_handle {
                        let _ = app.emit("preset-activated", preset_id);
                    }
                }
                return res;
            }
        }
        ToggleOutcome::Stop
    }

    /// Returns the ID of the currently executing preset, if started from a preset hotkey.
    pub fn current_running_preset_id(&self) -> Option<String> {
        self.current_running_preset.lock().unwrap().clone()
    }

    pub fn get_clicks_done(&self) -> u32 {
        self.clicks_done.load(Ordering::Relaxed)
    }

    pub fn adjust_cps(&self, delta: f64, app_handle: Option<&AppHandle>) -> f64 {
        let cur = f64::from_bits(self.cps_raw.load(Ordering::Relaxed));
        let next = (cur + delta).clamp(1.0, 160.0);
        self.cps_raw.store(next.to_bits(), Ordering::Relaxed);
        if let Some(app) = app_handle {
            let _ = app.emit("global-cps-change", next);
            let _ = app.emit(
                "status-update",
                StatusUpdate {
                    active: self.is_active(),
                    mode: if self.is_autoclicker_mode() {
                        "autoclicker".into()
                    } else {
                        "work".into()
                    },
                    clicks_done: self.get_clicks_done(),
                    cps: next,
                    status_text: if self.is_active() {
                        "RUNNING".into()
                    } else {
                        "IDLE".into()
                    },
                },
            );
        }
        next
    }

    /// Targeted RUNNING/IDLE telemetry emitter — SINGLE OWNER: the decoupled 66 ms
    /// telemetry worker (+ one final IDLE emit from the click thread after the
    /// loop exits). The hot click loop NEVER calls this (zero-jitter rule).
    /// No throttle inside: the worker's 66 ms sleep IS the cadence (~15 FPS),
    /// so there is no shared `LAST_EMIT` atomic and no `SystemTime` syscall —
    /// nothing to false-share with the click thread.
    fn status_and_hud_emit(
        app: &AppHandle,
        total: u32,
        mode: &str,
        cps: f64,
        status_text: &str,
        active: bool,
    ) {
        // Targeted emit: hud-clicks only to "hud" window.
        // app.emit() = broadcast to ALL windows → floods HUD IPC queue at high CPS.
        if let Some(hud) = app.get_webview_window("hud") {
            let _ = hud.emit("hud-clicks", total);
        }
        // Targeted emit: status-update only to "main" window.
        if let Some(main_win) = app.get_webview_window("main") {
            let _ = main_win.emit(
                "status-update",
                StatusUpdate {
                    active,
                    mode: mode.into(),
                    clicks_done: total,
                    cps,
                    status_text: status_text.into(),
                },
            );
        }
    }
    /// Immediate `status-update` (+ `hud-clicks`) emit carrying the LIVE state,
    /// for callers that are **not** the click worker.
    ///
    /// Why this exists: the 66 ms telemetry worker only runs while a run is
    /// alive, so it can never report a *refused* action. A tray menu
    /// "Start / Stop clicking" under Work Mode or the typing guard used to
    /// produce zero events anywhere — indistinguishable from a dead menu item —
    /// and a rebuild after deep sleep had no way to learn the current state
    /// either. Same targets and same fire-and-forget contract as the worker
    /// (`let _ =`, never a broadcast): the Zero-Jitter rule still holds, because
    /// this is called from the UI/tray path, never from the click loop.
    pub(crate) fn emit_status_now(&self, app: &AppHandle, status_text: &str) {
        let mode = if self.is_autoclicker_mode() {
            "autoclicker"
        } else {
            "work"
        };
        Self::status_and_hud_emit(
            app,
            self.get_clicks_done(),
            mode,
            f64::from_bits(self.cps_raw.load(Ordering::Relaxed)),
            status_text,
            self.is_active(),
        );
    }

    fn spawn_worker(&self, app_handle: Option<AppHandle>) {
        let active = Arc::clone(&self.active);
        let mode_autoclicker = Arc::clone(&self.mode_autoclicker);
        let clicks_done = Arc::clone(&self.clicks_done);
        let cps_raw = Arc::clone(&self.cps_raw);
        let random_pct_raw = Arc::clone(&self.random_pct_raw);
        let outlier_prob_raw = Arc::clone(&self.outlier_prob_raw);
        let limit = Arc::clone(&self.limit);
        let button_arc = Arc::clone(&self.button);
        let click_type_arc = Arc::clone(&self.click_type);
        let position_mode_arc = Arc::clone(&self.position_mode);
        let fixed_x_arc = Arc::clone(&self.fixed_x);
        let fixed_y_arc = Arc::clone(&self.fixed_y);
        let repeat_mode_arc = Arc::clone(&self.repeat_mode);
        let repeat_count_arc = Arc::clone(&self.repeat_count);
        let hold_duration_arc = Arc::clone(&self.hold_duration_ms);
        let hold_interval_arc = Arc::clone(&self.hold_interval_ms);
        let repeat_interval_arc = Arc::clone(&self.repeat_interval_ms);
        let jitter_radius_arc = Arc::clone(&self.jitter_radius_px);
        let sequence_points_arc = Arc::clone(&self.sequence_points);
        let start_delay_arc = Arc::clone(&self.start_delay_ms);
        let stop_duration_arc = Arc::clone(&self.stop_duration_ms);
        let stop_time_arc = Arc::clone(&self.stop_time_epoch_sec);
        let stop_mode_arc = Arc::clone(&self.stop_mode);
        let stop_event_lock = Arc::clone(&self.stop_event);
        let image_trigger_arc = Arc::clone(&self.image_trigger);
        let image_trigger_should_stop = Arc::clone(&self.image_trigger_should_stop);
        let visual_ripple = Arc::clone(&self.visual_ripple);
        let app_filter_arc = Arc::clone(&self.app_filter);
        let focus_guard_arc = Arc::clone(&self.focus_guard);
        let technique_arc = Arc::clone(&self.technique);

        thread::spawn(move || {
            #[cfg(target_os = "windows")]
            unsafe {
                use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST};
                let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
            }

            let mut rng = rand::thread_rng();
            let timer = PlatformTimer::new();
            let platform_backend = platform::default_input_backend();
            // Smart Guard: TTL-memoized foreground lookup for the app filter
            // (2 syscalls, so it must never run per click).
            let fg_cache = ForegroundCache::new();
            // Focus Guard: arm the session baseline + shared watcher state.
            // The watcher thread is spawned ONLY while the guard is enabled
            // (guard OFF = zero threads, zero syscalls). It polls the
            // foreground directly (~50 ms, no TTL cache) and wakes this loop
            // via the shared stop event, so reaction no longer depends on the
            // click interval / hold / start-delay sleeps.
            let focus_watch_stop = Arc::new(FocusWatchStop::new());
            let focus_watch_exit = Arc::new(AtomicBool::new(false));
            if focus_guard_arc.is_enabled() {
                focus_guard_arc.arm(fg_cache.exe());
                focus_watch_stop.reset();
                let wake_handle: Option<crate::platform::NativeEventHandle> =
                    stop_event_lock.lock().unwrap().clone();
                if let Ok(mut g) = focus_watch_stop.wake.lock() {
                    *g = wake_handle;
                }
            }
            // The watcher owns its clones; the loop keeps the originals for
            // the in-loop choke + run-end join.
            let focus_watch_handle = if focus_guard_arc.is_enabled() {
                Some(crate::guard::spawn_focus_watcher(
                    Arc::clone(&focus_guard_arc),
                    Arc::clone(&active),
                    Arc::clone(&focus_watch_stop),
                    app_handle.clone(),
                    Arc::clone(&focus_watch_exit),
                ))
            } else {
                None
            };

            let cur_button = button_arc.lock().unwrap().clone();
            let cur_technique = technique_arc.lock().unwrap().clone();
            // v4.2 — parse the config strings ONCE here at the boundary
            // and carry a typed ClickSpec through the whole click loop.
            // The platform layer no longer sees raw strings.
            let cur_click_spec = ClickSpec {
                button: platform::parse_button_label(&cur_button),
                click_type: crate::platform::backend::ClickType::from_config_str(
                    &click_type_arc.lock().unwrap().clone(),
                ),
                position_mode: crate::platform::backend::PositionMode::from_config_str(
                    &position_mode_arc.lock().unwrap().clone(),
                ),
                fixed_x: fixed_x_arc.load(Ordering::Relaxed) as i32,
                fixed_y: fixed_y_arc.load(Ordering::Relaxed) as i32,
                jitter_radius: jitter_radius_arc.load(Ordering::Relaxed),
                points: sequence_points_arc.lock().unwrap().clone(),
                point_index: 0,
            };
            // When a sequence is active the click loop positions the cursor
            // at each point in order. Snapshot the points once and reuse.
            let active_sequence: Vec<crate::config_manager::SequencePoint> =
                cur_click_spec.points.clone();
            let cur_repeat_mode = repeat_mode_arc.lock().unwrap().clone();
            let cur_repeat_count = repeat_count_arc.load(Ordering::Relaxed);
            let cur_hold_duration = hold_duration_arc.load(Ordering::Relaxed).max(10);
            let cur_hold_interval = hold_interval_arc.load(Ordering::Relaxed);
            let cur_repeat_interval = repeat_interval_arc.load(Ordering::Relaxed);

            let mut batch_click_count: u32 = 0;
            let mut batches_done: u32 = 0;

            // LAZY overlay: pre-create the ripple WebView on a background thread
            // at click-START (not in the hot loop) when ripple is enabled, so the
            // first spawn-ripple emit already has a target. Zero cadence impact:
            // this runs once before the loop, never per click.
            if visual_ripple.load(Ordering::Relaxed) {
                if let Some(ref app) = app_handle {
                    let app_clone = app.clone();
                    std::thread::spawn(move || {
                        let _ = crate::overlay::ensure_overlay_window(&app_clone);
                    });
                }
            }

            let emit_ripple_if_enabled = |spec: &ClickSpec| {
                if visual_ripple.load(Ordering::Relaxed) {
                    if let Some(ref app) = app_handle {
                        let (rx, ry) = match spec.position_mode {
                            crate::platform::backend::PositionMode::Fixed => (spec.fixed_x, spec.fixed_y),
                            crate::platform::backend::PositionMode::Cursor => platform_backend.cursor_position(),
                        };
                        crate::overlay::emit_click_ripple(app, rx, ry);
                    }
                }
            };

            // Immediate start notification — targeted to "main" window only
            if let Some(ref app) = app_handle {
                if let Some(main_win) = app.get_webview_window("main") {
                    let _ = main_win.emit(
                        "status-update",
                        StatusUpdate {
                            active: true,
                            mode: if mode_autoclicker.load(Ordering::Relaxed) {
                                "autoclicker".into()
                            } else {
                                "work".into()
                            },
                            clicks_done: 0,
                            cps: f64::from_bits(cps_raw.load(Ordering::Relaxed)),
                            status_text: "RUNNING".into(),
                        },
                    );
                }
            }

            // ── DECOUPLED ASYNCHRONOUS TELEMETRY WORKER (Fire-and-Forget) ─
            // Runs on a separate low-overhead thread so the high-CPS click loop
            // never touches Tauri IPC, awaits WebViews, or experiences UI lockup.
            if let Some(ref app) = app_handle {
                let active_for_telemetry = Arc::clone(&active);
                let clicks_for_telemetry = Arc::clone(&clicks_done);
                let mode_for_telemetry = Arc::clone(&mode_autoclicker);
                let cps_for_telemetry = Arc::clone(&cps_raw);
                let app_for_telemetry = app.clone();

                std::thread::spawn(move || {
                    while active_for_telemetry.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(66)); // ~15 FPS UI telemetry cadence
                        if !active_for_telemetry.load(Ordering::Relaxed) {
                            break;
                        }
                        let total = clicks_for_telemetry.load(Ordering::Relaxed);
                        let mode_str = if mode_for_telemetry.load(Ordering::Relaxed) {
                            "autoclicker"
                        } else {
                            "work"
                        };
                        let cur_cps = f64::from_bits(cps_for_telemetry.load(Ordering::Relaxed));
                        Self::status_and_hud_emit(&app_for_telemetry, total, mode_str, cur_cps, "RUNNING", true);
                    }
                });
            }

            // ── IMAGE TRIGGER POLLER ──────────────────────────────────────
            // Spawned ONLY when an image trigger is set; exits as soon as
            // the click loop is no longer active.
            let poller_should_stop = Arc::new(AtomicBool::new(false));
            {
                let image_trigger_for_poller = Arc::clone(&image_trigger_arc);
                let should_stop_flag = Arc::clone(&image_trigger_should_stop);
                let active_flag = Arc::clone(&active);
                let poller_done = Arc::clone(&poller_should_stop);
                let app_for_poller = app_handle.clone();
                std::thread::spawn(move || loop {
                    if poller_done.load(Ordering::Relaxed) || !active_flag.load(Ordering::Relaxed) {
                        break;
                    }
                    let snapshot = image_trigger_for_poller.lock().unwrap().clone();
                    if let Some(trig) = snapshot {
                        let poll_ms = trig.poll_ms.clamp(50, 2000) as u64;
                        std::thread::sleep(Duration::from_millis(poll_ms));
                        if let Some(rgba) = platform::get_pixel_rgba(trig.x, trig.y) {
                            let tol = trig.tolerance.min(255);
                            let tr = (trig.color_rgba >> 24) & 0xFF;
                            let tg = (trig.color_rgba >> 16) & 0xFF;
                            let tb = (trig.color_rgba >> 8) & 0xFF;
                            let pr = (rgba >> 24) & 0xFF;
                            let pg = (rgba >> 16) & 0xFF;
                            let pb = (rgba >> 8) & 0xFF;
                            if tr.abs_diff(pr) <= tol
                                && tg.abs_diff(pg) <= tol
                                && tb.abs_diff(pb) <= tol
                            {
                                should_stop_flag.store(true, Ordering::Relaxed);
                                if let Some(ref app) = app_for_poller {
                                    let _ = app.emit("image-trigger-match", trig.label.clone());
                                }
                                break;
                            }
                        }
                    } else {
                        std::thread::sleep(Duration::from_millis(200));
                    }
                });
            }

            // ── START DELAY (configurable) ─────────────────────────────
            let cur_start_delay = start_delay_arc.load(Ordering::Relaxed);
            if cur_start_delay > 0 {
                let event_handle = stop_event_lock.lock().unwrap().clone().expect("stop_event");
                let target_start = Instant::now() + Duration::from_millis(cur_start_delay);
                let wait_ok = timer.wait_until(target_start, event_handle);
                if !wait_ok || !active.load(Ordering::Relaxed) {
                    active.store(false, Ordering::Relaxed);
                }
            }

            // Snapshot stop timers for the lifetime of this run
            // (clamped again here: `set_config` is not the only way a Config is
            // built — `Config::default()` and the boot-time load also reach the
            // atomics, and the click loop must never be handed a duration the
            // Windows timer arithmetic cannot represent).
            let cur_stop_duration_ms = stop_duration_arc
                .load(Ordering::Relaxed)
                .min(crate::config::MAX_STOP_DURATION_MS);
            let cur_stop_time_sec = stop_time_arc.load(Ordering::Relaxed);
            // ONE TRIGGER PER RUN. Both values stay in the config (so switching
            // back restores what the user typed), but only the armed one may end
            // this run — the page shows exactly that, and a loop that stopped on
            // "whichever deadline is nearer" would contradict its own UI.
            let cur_stop_mode = stop_mode_arc.lock().unwrap().clone();
            let cur_stop_duration_ms = if cur_stop_mode == "duration" {
                cur_stop_duration_ms
            } else {
                0
            };
            let cur_stop_time_sec = if cur_stop_mode == "wallclock" {
                cur_stop_time_sec
            } else {
                0
            };
            let run_started_at = Instant::now();
            // WHY the loop ended, if a stop timer did it. Set inside the loop and
            // EMITTED after it, so the click path itself stays IPC-free
            // (Zero-Jitter: the loop never waits for the UI).
            let mut auto_stop_reason: Option<&'static str> = None;
            // Captured at the INSTANT the loop decides to stop, not at the emit
            // site: the emit happens after the focus-watcher join and the IDLE
            // status, so `run_started_at.elapsed()` there is inflated by the
            // teardown (a 5 s limit reported 5.05 s).
            let mut auto_stop_elapsed_ms: u64 = 0;
            let mut mouse_guard = MouseHoldGuard::new(platform_backend.as_ref(), cur_click_spec.button);
            let mut butterfly_engine = ButterflyEngine::new();
            let mut drag_engine = DragClickEngine::new();

            // ── CLICKING LOOP ──────────────────────────────────────────
            while active.load(Ordering::Relaxed) && mode_autoclicker.load(Ordering::Relaxed) {
                // Stop by elapsed duration
                if cur_stop_duration_ms > 0
                    && run_started_at.elapsed() >= Duration::from_millis(cur_stop_duration_ms)
                {
                    auto_stop_reason = Some("duration");
                    auto_stop_elapsed_ms = run_started_at.elapsed().as_millis() as u64;
                    active.store(false, Ordering::Relaxed);
                    break;
                }
                // Image-trigger stop request (raised by the poller below).
                if image_trigger_should_stop.load(Ordering::Relaxed) {
                    active.store(false, Ordering::Relaxed);
                    image_trigger_should_stop.store(false, Ordering::Relaxed);
                    if let Some(ref app) = app_handle {
                        let _ = app.emit("image-trigger-stopped", "matched target pixel");
                    }
                    break;
                }
                // Stop by absolute wall-clock time
                if cur_stop_time_sec > 0 {
                    let now_unix = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    if now_unix >= cur_stop_time_sec {
                        auto_stop_reason = Some("wallclock");
                        auto_stop_elapsed_ms = run_started_at.elapsed().as_millis() as u64;
                        active.store(false, Ordering::Relaxed);
                        break;
                    }
                }
                let current_limit = limit.load(Ordering::Relaxed);

                // Check limit per batch or overall
                if current_limit > 0 && batch_click_count >= current_limit {
                    batches_done += 1;
                    if cur_repeat_mode == "repeat"
                        && cur_repeat_count > 0
                        && batches_done >= cur_repeat_count
                    {
                        active.store(false, Ordering::Relaxed);
                        break;
                    }

                    // Sleep for repeat_interval before next batch
                    if cur_repeat_interval > 0 {
                        let target = Instant::now() + Duration::from_millis(cur_repeat_interval);
                        let event_handle =
                            stop_event_lock.lock().unwrap().clone().expect("stop_event");
                        let success = timer.wait_until(target, event_handle);
                        if !success || !active.load(Ordering::Relaxed) {
                            break;
                        }
                    }
                    batch_click_count = 0; // Reset batch count for next repeat cycle
                }

                // If not in repeat batch mode, check total limit
                if cur_repeat_mode != "repeat"
                    && current_limit > 0
                    && clicks_done.load(Ordering::Relaxed) >= current_limit
                {
                    active.store(false, Ordering::Relaxed);
                    break;
                }

                // ── SMART GUARD: APP FILTER ──────────────────────────────
                // Single choke point in front of every dispatch branch
                // (hold / double / single / sequence), so no click can ever
                // slip past the filter. Costs one atomic load while the
                // filter is disabled.
                //
                // The Typing Guard deliberately does NOT live here: while
                // typing the clicker is already idle, so freezing the loop
                // would only cost performance. What actually hurts the user
                // is the TOGGLE HOTKEY firing inside a word — that gate is
                // in the keyboard hook.
                if app_filter_arc.is_active() {
                    let fg_exe = fg_cache.exe();
                    if !app_filter_arc.allows(fg_exe.as_deref()) {
                        thread::sleep(Duration::from_millis(GUARD_POLL_MS));
                        continue;
                    }
                }

                // ── SMART GUARD: FOCUS LOSS AUTO-PAUSE ────────────────────
                // Two layers (either may fire first — exactly-once claim):
                // 1. watcher thread (~50 ms, direct lookup): flips `active`
                //    off, wakes this sleeper via the stop event, and may
                //    already have emitted `focus-loss-paused`.
                // 2. this in-loop choke (TTL cache): catches whatever the
                //    watcher hasn't seen yet; owns the emit when IT fires.
                // `None` (lock screen, elevated) and our own exe fail OPEN.
                if focus_guard_arc.is_enabled() {
                    if focus_watch_stop.is_requested() {
                        active.store(false, Ordering::Relaxed);
                        if focus_watch_stop.request_stop(focus_watch_stop.take_thief()) {
                            if let Some(ref app) = app_handle {
                                let _ = app.emit("focus-loss-paused", focus_watch_stop.take_thief());
                            }
                        }
                        break;
                    }
                    let fg_exe = fg_cache.exe();
                    if focus_guard_arc.should_pause(fg_exe.as_deref()) {
                        active.store(false, Ordering::Relaxed);
                        if focus_watch_stop.request_stop(fg_exe.clone()) {
                            if let Some(ref app) = app_handle {
                                let _ = app.emit("focus-loss-paused", fg_exe);
                            }
                        }
                        break;
                    }
                }

                // ── HOLD CLICK LOGIC ─────────────────────────────────────
                if cur_click_spec.click_type == crate::platform::backend::ClickType::Hold {
                    // Press Down
                    // v4.2 instant stop: re-check active immediately before
                    // dispatching so a stop signal that arrived during the wait
                    // never produces an extra click.
                    if !active.load(Ordering::Relaxed) {
                        break;
                    }
                    // v4.2 instant stop: Double is decomposed here so the gap
                    // between the two clicks is cancel-aware (the backend has no
                    // cancel handle; the scheduler owns the active flag).
                    if cur_click_spec.click_type == crate::platform::backend::ClickType::Double {
                        let mut dispatched = 0usize;
                        while dispatched < 2 && active.load(Ordering::Relaxed) {
                            let single = crate::platform::backend::ClickSpec {
                                click_type: crate::platform::backend::ClickType::Single,
                                ..cur_click_spec.clone()
                            };
                            if platform_backend.click_mouse(&single) {
                                clicks_done.fetch_add(1, Ordering::Relaxed);
                                batch_click_count += 1;
                                emit_ripple_if_enabled(&single);
                            }
                            dispatched += 1;
                            if dispatched < 2 {
                                let target = Instant::now() + Duration::from_millis(50);
                                let event_handle =
                                    stop_event_lock.lock().unwrap().clone().expect("stop_event");
                                if !timer.wait_until(target, event_handle)
                                    || !active.load(Ordering::Relaxed)
                                {
                                    break;
                                }
                            }
                        }
                    } else if platform_backend.click_mouse(&cur_click_spec) {
                        clicks_done.fetch_add(1, Ordering::Relaxed);
                        batch_click_count += 1;
                        emit_ripple_if_enabled(&cur_click_spec);
                    }

                    // Hold for hold_duration_ms
                    let event_handle = stop_event_lock.lock().unwrap().clone().expect("stop_event");
                    let target_down = Instant::now() + Duration::from_millis(cur_hold_duration);
                    if !timer.wait_until(target_down, event_handle)
                        || !active.load(Ordering::Relaxed)
                    {
                        platform_backend.release_mouse_hold(cur_click_spec.button);
                        break;
                    }

                    // Release Up
                    platform_backend.release_mouse_hold(cur_click_spec.button);

                    // Pause for hold_interval_ms if > 0
                    if cur_hold_interval > 0 {
                        let event_handle2 =
                            stop_event_lock.lock().unwrap().clone().expect("stop_event");
                        let target_up = Instant::now() + Duration::from_millis(cur_hold_interval);
                        if !timer.wait_until(target_up, event_handle2)
                            || !active.load(Ordering::Relaxed)
                        {
                            break;
                        }
                    }

                    continue;
                }

                // ── REGULAR / DOUBLE CLICK LOGIC ─────────────────────────
                // clamp(1, 100): the UI enforces this range, but if an
                // out-of-range value was persisted (e.g. 1000.0 from a
                // previous bug), the loop would spin at OS-sleep granularity
                // (~150 CPS effective) instead of the requested rate.
                let cps = f64::from_bits(cps_raw.load(Ordering::Relaxed)).clamp(1.0, 160.0);
                let random_pct = f64::from_bits(random_pct_raw.load(Ordering::Relaxed)).max(0.0);
                let outlier_bits = outlier_prob_raw.load(Ordering::Relaxed);
                let outlier_prob = f64::from_bits(outlier_bits);
                let outlier_prob = if outlier_prob.is_finite() {
                    outlier_prob.clamp(0.0, 0.10)
                } else {
                    crate::config::DEFAULT_OUTLIER_PROB
                };

                let active_technique = crate::config::resolve_technique(&cur_technique, cps);
                let target_interval_ms = 1000.0 / cps;

                let (hold_ms, release_ms) = match active_technique.as_str() {
                    "butterfly" => butterfly_engine.next_timing(&mut rng, target_interval_ms),
                    "drag" => drag_engine.next_timing(&mut rng, target_interval_ms),
                    _ => calc_single_timing(&mut rng, target_interval_ms, random_pct, outlier_prob),
                };

                // v4.2 instant stop: re-check active immediately before dispatch
                if !active.load(Ordering::Relaxed) {
                    break;
                }

                // ── Multi-point sequence handling ──────────────────────
                if !active_sequence.is_empty() {
                    let idx = (batch_click_count as usize) % active_sequence.len();
                    let p = &active_sequence[idx];
                    platform_backend.set_cursor_pos(p.x, p.y);
                    let mut seq_spec = cur_click_spec.clone();
                    seq_spec.fixed_x = p.x;
                    seq_spec.fixed_y = p.y;
                    seq_spec.points.clear();
                    seq_spec.point_index = 0;

                    mouse_guard.press_down();
                    clicks_done.fetch_add(1, Ordering::Relaxed);
                    batch_click_count += 1;
                    emit_ripple_if_enabled(&seq_spec);

                    let event_handle = stop_event_lock.lock().unwrap().clone().expect("stop_event");
                    let target_down = Instant::now() + Duration::from_secs_f64(hold_ms / 1000.0);
                    if !timer.wait_until(target_down, event_handle.clone()) || !active.load(Ordering::Relaxed) {
                        mouse_guard.release_up();
                        break;
                    }
                    mouse_guard.release_up();

                    let seq_delay = if p.delay_ms > 0 { p.delay_ms as f64 } else { release_ms };
                    let target_up = Instant::now() + Duration::from_secs_f64(seq_delay / 1000.0);
                    if !timer.wait_until(target_up, event_handle) || !active.load(Ordering::Relaxed) {
                        break;
                    }
                    continue;
                }

                // Regular click positioning
                let (orig_x, orig_y) = platform_backend.cursor_position();
                if cur_click_spec.position_mode == crate::platform::backend::PositionMode::Fixed {
                    let mut tx = cur_click_spec.fixed_x;
                    let mut ty = cur_click_spec.fixed_y;
                    if cur_click_spec.jitter_radius > 0 {
                        let r = cur_click_spec.jitter_radius as i32;
                        tx += rng.gen_range(-r..=r);
                        ty += rng.gen_range(-r..=r);
                    }
                    platform_backend.set_cursor_pos(tx, ty);
                } else if cur_click_spec.jitter_radius > 0 {
                    let r = cur_click_spec.jitter_radius as i32;
                    let tx = orig_x + rng.gen_range(-r..=r);
                    let ty = orig_y + rng.gen_range(-r..=r);
                    platform_backend.set_cursor_pos(tx, ty);
                }

                // Press Down
                mouse_guard.press_down();
                clicks_done.fetch_add(1, Ordering::Relaxed);
                batch_click_count += 1;
                emit_ripple_if_enabled(&cur_click_spec);

                // Hold phase (safe against early break)
                let event_handle = stop_event_lock.lock().unwrap().clone().expect("stop_event");
                let target_down = Instant::now() + Duration::from_secs_f64(hold_ms / 1000.0);
                if !timer.wait_until(target_down, event_handle.clone()) || !active.load(Ordering::Relaxed) {
                    mouse_guard.release_up();
                    break;
                }

                // Release phase
                mouse_guard.release_up();
                if cur_click_spec.jitter_radius > 0
                    && cur_click_spec.position_mode == crate::platform::backend::PositionMode::Cursor
                {
                    platform_backend.set_cursor_pos(orig_x, orig_y);
                }

                // Double click support
                if cur_click_spec.click_type == crate::platform::backend::ClickType::Double {
                    let gap_target = Instant::now() + Duration::from_millis(50);
                    if !timer.wait_until(gap_target, event_handle.clone()) || !active.load(Ordering::Relaxed) {
                        break;
                    }
                    mouse_guard.press_down();
                    clicks_done.fetch_add(1, Ordering::Relaxed);
                    batch_click_count += 1;
                    emit_ripple_if_enabled(&cur_click_spec);
                    let target_down2 = Instant::now() + Duration::from_secs_f64(hold_ms / 1000.0);
                    if !timer.wait_until(target_down2, event_handle.clone()) || !active.load(Ordering::Relaxed) {
                        mouse_guard.release_up();
                        break;
                    }
                    mouse_guard.release_up();
                }

                // Release duration between clicks
                let target_up = Instant::now() + Duration::from_secs_f64(release_ms / 1000.0);
                if !timer.wait_until(target_up, event_handle) || !active.load(Ordering::Relaxed) {
                    break;
                }
            }

            if cur_click_spec.click_type == crate::platform::backend::ClickType::Hold {
                platform_backend.release_mouse_hold(cur_click_spec.button);
            }

            // Stop the focus watcher: signal exit and join it so no stray
            // thread survives the run (it also exits on inactive `active`).
            focus_watch_exit.store(true, Ordering::Relaxed);
            if let Some(h) = focus_watch_handle {
                let _ = h.join();
            }

            if let Some(ref app) = app_handle {
                let total = clicks_done.load(Ordering::Relaxed);
                let mode_str = if mode_autoclicker.load(Ordering::Relaxed) {
                    "autoclicker"
                } else {
                    "work"
                };
                let final_cps = f64::from_bits(cps_raw.load(Ordering::Relaxed));
                Self::status_and_hud_emit(app, total, mode_str, final_cps, "IDLE", false);
                crate::overlay::flush_click_ripples(app);
                // "The clicker stopped itself" must never be silent: without this
                // the run just ends and the only explanation is a status-update the
                // page folds into its idle state. Emitted AFTER the IDLE status and
                // OUTSIDE the loop — the click path stays IPC-free.
                if let Some(reason) = auto_stop_reason {
                    let _ = app.emit(
                        "auto-stop",
                        serde_json::json!({
                            "reason": reason,
                            "elapsed_ms": auto_stop_elapsed_ms,
                        }),
                    );
                }
            }

            // Focus Guard: the session is over — drop the baseline so the
            // next run re-arms with a fresh foreground. (set_config also
            // disarms on save; this is the authoritative run-end cleanup.)
            focus_guard_arc.disarm();
        });
    }
}

/// Rare biological hesitation: with probability `prob` the interval
/// stretches to 2-3x base. Pure helper so the rate/shape is unit-testable
/// without spinning the click loop.
pub(crate) fn outlier_interval_ns(
    rng: &mut impl rand::Rng,
    base_ns: i64,
    bates_ns: i64,
    prob: f64,
) -> i64 {
    // NaN/negative/infinite can never arm a hesitation: only (0, 0.10] may.
    // (NaN.clamp() PANICS in core, so the range check must come first.)
    if prob.is_finite() && prob > 0.0 && rng.gen_bool(prob.clamp(0.0, 0.10)) {
        let k: f64 = rng.gen_range(2.0..=3.0);
        return (base_ns as f64 * k) as i64;
    }
    bates_ns
}

/// Bates B3 jitter factor: mean of 3 independent uniform draws in [-pct, +pct].
///
/// Gaussian-like human tremor (concentrated near target CPS) with STRICT bounds:
/// the mean of three values in [-p, +p] can never leave [-p, +p] — no clamp,
/// no broken slider contract, no timer underflow at 160 CPS.
/// Moments: E = 0, Var = p²/9 (uniform would be p²/3). NO ×√3 compensation
/// on purpose: rescaling would push edges to ±p×1.73 and break the ±35% promise.
pub(crate) fn bates_jitter_factor(rng: &mut impl rand::Rng, pct_frac: f64) -> f64 {
    if !(pct_frac > 0.0) {
        return 0.0;
    }
    let r1 = rng.gen_range(-pct_frac..=pct_frac);
    let r2 = rng.gen_range(-pct_frac..=pct_frac);
    let r3 = rng.gen_range(-pct_frac..=pct_frac);
    (r1 + r2 + r3) / 3.0
}

/// RAII guard ensuring mouse button release on any exit, break or panic.
pub(crate) struct MouseHoldGuard<'a, B: crate::platform::backend::InputBackend + ?Sized> {
    backend: &'a B,
    button: MouseButton,
    is_down: bool,
}

impl<'a, B: crate::platform::backend::InputBackend + ?Sized> MouseHoldGuard<'a, B> {
    pub(crate) fn new(backend: &'a B, button: MouseButton) -> Self {
        Self {
            backend,
            button,
            is_down: false,
        }
    }

    pub(crate) fn press_down(&mut self) {
        if !self.is_down {
            self.backend.mouse_down(self.button);
            self.is_down = true;
        }
    }

    pub(crate) fn release_up(&mut self) {
        if self.is_down {
            self.backend.mouse_up(self.button);
            self.is_down = false;
        }
    }
}

impl<'a, B: crate::platform::backend::InputBackend + ?Sized> Drop for MouseHoldGuard<'a, B> {
    fn drop(&mut self) {
        if self.is_down {
            self.backend.mouse_up(self.button);
            self.is_down = false;
        }
    }
}

/// Bates B3 distribution: mean of 3 uniform draws in [mean - radius, mean + radius].
/// Returns a value strictly bounded within [mean - radius, mean + radius] with Gaussian-like clustering.
pub(crate) fn bates_b3(rng: &mut impl rand::Rng, mean: f64, radius: f64) -> f64 {
    if !(radius > 0.0) {
        return mean;
    }
    let r1 = rng.gen_range(-radius..=radius);
    let r2 = rng.gen_range(-radius..=radius);
    let r3 = rng.gen_range(-radius..=radius);
    mean + (r1 + r2 + r3) / 3.0
}

/// Calculate dynamic hold time strictly capped at 40% of target interval,
/// while respecting physical microswitch debounce/activation floors and
/// dynamically fading out human hold duration at high CPS (up to 160 CPS).
pub(crate) fn calc_dynamic_hold_time(rng: &mut impl rand::Rng, target_interval_ms: f64) -> f64 {
    let cps = (1000.0 / target_interval_ms.max(1.0)).clamp(1.0, 160.0);
    // Dynamic fadeout factor: 1.0 at <=20 CPS, smoothly drops to 0.0 at >=80 CPS
    let bio_factor = ((80.0 - cps) / 60.0).clamp(0.0, 1.0);

    let raw_hold = bates_b3(rng, 18.0 + 13.0 * bio_factor, 5.0 + 8.0 * bio_factor);
    let max_allowed_hold = target_interval_ms * 0.40;
    let floor = 2.0 + 6.0 * bio_factor;
    let min_hold = floor.min(max_allowed_hold).max(2.0);
    raw_hold.min(max_allowed_hold).max(min_hold)
}

/// Single click timing with human fatigue model (asymmetrical tail) and biological hesitation,
/// which dynamically fades out at high CPS (>30..80 CPS) allowing full 160 CPS overdrive.
pub(crate) fn calc_single_timing(
    mut rng: &mut impl rand::Rng,
    target_interval_ms: f64,
    jitter_pct: f64,
    outlier_prob: f64,
) -> (f64, f64) {
    let cps = (1000.0 / target_interval_ms.max(1.0)).clamp(1.0, 160.0);
    let bio_factor = ((70.0 - cps) / 50.0).clamp(0.0, 1.0);

    let base_ns = (target_interval_ms * 1_000_000.0) as i64;
    let deviation_ns = (base_ns as f64 * ((jitter_pct * bio_factor).max(0.0) / 100.0)) as i64;
    let mut interval_ns = if deviation_ns > 0 {
        let f = bates_jitter_factor(rng, deviation_ns as f64 / base_ns as f64);
        (base_ns as f64 * (1.0 + f)) as i64
    } else {
        base_ns
    };

    // Human fatigue/stutter: 15% probability of asymmetrical stretch +5..=18ms (scaled by bio_factor)
    if rng.gen_ratio(15, 100) && bio_factor > 0.05 {
        interval_ns += ((rng.gen_range(5.0..=18.0) * bio_factor) * 1_000_000.0) as i64;
    }

    let outlier_prob = outlier_prob * bio_factor;
    interval_ns = outlier_interval_ns(&mut rng, base_ns, interval_ns, outlier_prob);

    let interval_ms = (interval_ns as f64) / 1_000_000.0;
    let hold_ms = calc_dynamic_hold_time(rng, target_interval_ms);
    let release_ms = (interval_ms - hold_ms).max(2.0);

    (hold_ms, release_ms)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ButterflyFinger {
    Index,
    Middle,
}

/// Butterfly technique: alternating two-finger biomechanics with microswitch debounce bounce.
/// Features dynamic fadeout scaling to track target_cps up to 160 CPS.
#[derive(Debug, Clone)]
pub(crate) struct ButterflyEngine {
    current_finger: ButterflyFinger,
}

impl ButterflyEngine {
    pub(crate) fn new() -> Self {
        Self {
            current_finger: ButterflyFinger::Index,
        }
    }

    pub(crate) fn next_timing(
        &mut self,
        rng: &mut impl rand::Rng,
        target_interval_ms: f64,
    ) -> (f64, f64) {
        let cps = (1000.0 / target_interval_ms.max(1.0)).clamp(1.0, 160.0);
        let bio_factor = ((80.0 - cps) / 60.0).clamp(0.0, 1.0);

        // 35% probability of physical switch bounce (debounce bounce peak at 14..=26ms)
        let is_bounce = rng.gen_ratio(35, 100);
        let base_interval = if is_bounce {
            bates_b3(rng, 20.0, 6.0)
        } else {
            match self.current_finger {
                ButterflyFinger::Index => {
                    self.current_finger = ButterflyFinger::Middle;
                    bates_b3(rng, 72.0, 12.0) // 60..=84ms
                }
                ButterflyFinger::Middle => {
                    self.current_finger = ButterflyFinger::Index;
                    bates_b3(rng, 84.0, 15.0) // 69..=99ms
                }
            }
        };

        // Scale to user's target CPS (nominal unscaled butterfly average cycle is ~57ms, ~17.5 CPS)
        let nominal_cycle = 57.0;
        let scale = target_interval_ms / nominal_cycle;
        let raw_interval = (base_interval * scale).max(target_interval_ms * 0.70);

        // At high CPS (bio_factor -> 0), interval converges cleanly to target_interval_ms
        let interval_ms = (target_interval_ms + (raw_interval - target_interval_ms) * bio_factor).max(4.0);

        let max_allowed_hold = interval_ms * 0.40;
        let raw_hold = bates_b3(rng, 20.0, 6.0) * scale;
        let human_hold = raw_hold.clamp(2.0, max_allowed_hold);
        let machine_hold = (interval_ms * 0.35).clamp(2.0, max_allowed_hold);
        let hold_ms = machine_hold + (human_hold - machine_hold) * bio_factor;
        let release_ms = (interval_ms - hold_ms).max(2.0);

        (hold_ms, release_ms)
    }
}

/// Drag click technique: stick-slip friction bursts + hand repositioning deadzone.
/// Features Debounce=0ms micro-bounce simulation (instant burst fire 90–110 CPS),
/// and dynamic fadeout scaling allowing full 160 CPS control.
#[derive(Debug, Clone)]
pub(crate) struct DragClickEngine {
    burst_remaining: u32,
}

impl DragClickEngine {
    pub(crate) fn new() -> Self {
        Self { burst_remaining: 0 }
    }

    pub(crate) fn next_timing(
        &mut self,
        rng: &mut impl rand::Rng,
        target_interval_ms: f64,
    ) -> (f64, f64) {
        let target_cps = (1000.0 / target_interval_ms.max(1.0)).clamp(1.0, 160.0);
        let bio_factor = ((70.0 - target_cps) / 50.0).clamp(0.0, 1.0);

        // High-speed scale when target_cps exceeds 50 CPS
        let high_speed_scale = (target_interval_ms / 10.0).clamp(0.35, 1.0);

        if self.burst_remaining == 0 {
            // 1. Хаос довжини пачки: 25% зірваний драг (5..=8), 75% повноцінна протяжка (10..=18)
            let burst_len = if rng.gen_bool(0.25) {
                rng.gen_range(5..=8)
            } else {
                rng.gen_range(10..=18)
            };
            self.burst_remaining = burst_len;

            // 2. Балансування під повзунок CPS:
            // Крок кліку всередині пачки: ~9.5ms на низькому CPS (Debounce=0ms, 100+ CPS спалах),
            // стискається при високому target_cps
            let burst_click_ms = (9.5 * high_speed_scale).min(target_interval_ms);
            let desired_cycle_total_ms = (burst_len as f64 / target_cps) * 1000.0;
            let burst_duration_ms = burst_len as f64 * burst_click_ms;
            let raw_reset_ms = desired_cycle_total_ms - burst_duration_ms;

            // Адаптивна пауза повернення руки: при звичайному CPS зберігає людську дедзону,
            // при високому CPS (Fadeout) плавно стискається до інтервалу такту
            let min_reset = (60.0 * bio_factor + target_interval_ms * (1.0 - bio_factor)).max(2.0);
            let reset_pause_ms = raw_reset_ms.clamp(min_reset, 2500.0);
            let final_reset_ms = bates_b3(rng, reset_pause_ms, reset_pause_ms * 0.20 * bio_factor).max(min_reset);

            self.burst_remaining -= 1;
            let hold = (bates_b3(rng, 4.0, 1.0) * high_speed_scale).clamp(2.0, target_interval_ms * 0.40);
            return (hold, final_reset_ms);
        }

        // --- Фаза всередині протяжки (Debounce = 0ms мікро-відскок + тертя stick-slip) ---
        self.burst_remaining -= 1;

        // Тертя сповільнюється до кінця протяжки:
        let friction_drag = (18 - self.burst_remaining.min(18)) as f64 * 0.25 * bio_factor;
        let hold_ms = (bates_b3(rng, 4.0, 1.0) * high_speed_scale).clamp(2.0, target_interval_ms * 0.40);
        let release_ms = (bates_b3(rng, 5.0 + friction_drag, 1.5) * high_speed_scale).max(2.0);

        (hold_ms, release_ms)
    }
}

#[cfg(test)]
mod hotkey_debounce_tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::Mutex;

    #[test]
    fn first_toggle_always_passes() {
        let last = Arc::new(Mutex::new(None));
        assert!(ClickScheduler::toggle_debounce_check(&last, 80));
    }

    #[test]
    fn second_toggle_within_window_is_blocked() {
        let last = Arc::new(Mutex::new(None));
        assert!(ClickScheduler::toggle_debounce_check(&last, 80));
        assert!(!ClickScheduler::toggle_debounce_check(&last, 80));
    }

    #[test]
    fn second_toggle_after_window_passes() {
        let last = Arc::new(Mutex::new(None));
        assert!(ClickScheduler::toggle_debounce_check(&last, 5));
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(ClickScheduler::toggle_debounce_check(&last, 5));
    }

    #[test]
    fn stale_held_helpers_cover_fast_double_tap() {
        assert!(ClickScheduler::stale_held_check(false, true));
        assert!(!ClickScheduler::stale_held_check(true, false));
        assert!(!ClickScheduler::stale_held_check(true, true));
    }

    #[test]
    fn debounce_block_does_not_advance_timestamp() {
        let last = Arc::new(Mutex::new(None));
        assert!(ClickScheduler::toggle_debounce_check(&last, 80));
        let t0 = *last.lock().unwrap();
        for _ in 0..5 {
            assert!(!ClickScheduler::toggle_debounce_check(&last, 80));
        }
        let t1 = *last.lock().unwrap();
        assert_eq!(
            t0, t1,
            "blocked calls must not update the debounce timestamp"
        );
    }

    #[test]
    fn toggle_diag_push_does_not_panic() {
        // 150 pushes (above capacity 128) — should not panic, should keep
        // exactly 128 entries.
        for i in 0..150 {
            toggle_diag_push(format!("line {i}"));
        }
        let dump = toggle_diag_dump();
        assert_eq!(dump.len(), 128, "ring buffer should cap at 128");
        // oldest entries should be the first ones dropped, so first in dump
        // is line 22 (150 - 128)
        assert!(
            dump[0].contains("22"),
            "expected line 22 first; got {}",
            dump[0]
        );
        assert!(dump.last().unwrap().contains("149"));
    }

    #[test]
    fn toggle_diag_dump_returns_empty_second_call() {
        for i in 0..3 {
            toggle_diag_push(format!("x{i}"));
        }
        let d1 = toggle_diag_dump();
        assert_eq!(d1.len(), 3);
        let d2 = toggle_diag_dump();
        assert!(d2.is_empty(), "dump should drain");
    }
}

#[cfg(test)]
mod single_mode_active_precheck_tests {
    use super::*;

    /// Source-level structural test: the worker loop's single-mode branch
    /// must re-check `active` immediately before invoking `click_mouse`,
    /// otherwise `wait_until → true → click_mouse` allows a phantom click
    /// when `active` flips to false in the 1-2 ms gap. We assert this by
    /// scanning the source for the canonical pattern.
    #[test]
    fn single_mode_has_active_precheck_before_click_mouse() {
        let src = include_str!("scheduler.rs");
        // Find the single-mode click invocation (not the one inside the
        // while-loop for double decomposition).
        // The active pre-check must appear BEFORE the click_mouse call.
        let precheck_off = src
            .find("// v4.2 instant stop (single mode): wait_until returned success")
            .expect("precheck comment not present in scheduler.rs");
        // The single-mode click_mouse call is the SECOND occurrence in the
        // source: the first is inside the double-decomposition loop.
        let mut click_positions =
            src.match_indices("if platform_backend.click_mouse(&cur_click_spec)");
        click_positions.next(); // skip double-mode
        let (click_off, _) = click_positions
            .next()
            .expect("single-mode click_mouse call missing");
        assert!(
            precheck_off < click_off,
            "active pre-check must appear before single-mode click_mouse call"
        );
    }

    /// Source-level guard: the remaining 2 debug_log_internal("stage-ok", ...)
    /// calls in hotkey_toggle were replaced with toggle_diag_push.
    #[test]
    fn hotkey_toggle_no_longer_writes_to_log_file() {
        let src = include_str!("scheduler.rs");
        let start = src
            .find("pub fn hotkey_toggle")
            .expect("hotkey_toggle not found");
        let after = start;
        let end = src[after..]
            .find("mode_str.to_string()")
            .map(|o| start + o + "mode_str.to_string()".len())
            .expect("hotkey_toggle body end not found");
        let body = &src[start..end];
        assert!(
            !body.contains("debug_log_internal"),
            "hotkey_toggle() must not call debug_log_internal (write to log file)"
        );
        assert!(
            body.contains("toggle_diag_push"),
            "hotkey_toggle() should push diagnostics to the ring buffer instead"
        );
    }

    /// TYPING KILL-SWITCH: a text keypress while running must stop the clicker,
    /// and a start attempt while the user is typing must be swallowed.
    ///
    /// NOTE: this test NEVER calls `set_active(true)` — in autoclicker mode
    /// that would spawn a real click worker. All assertions go through the
    /// pure `typing_allows_start()` decision plus the guard's own lockout
    /// state; the `set_active` choke point is covered by inspection (the
    /// `if active && !self.typing_allows_start() { return; }` guard sits
    /// before the mode check and before `spawn_worker`).
    #[test]
    fn typing_kill_switch_blocks_start_but_never_stop() {
        let scheduler = ClickScheduler::new();
        // Disabled guard (default config): the kill-switch decision is
        // transparent.
        assert!(scheduler.typing_allows_start());

        // Arm the lockout with a real text keypress: starting must now be
        // forbidden.
        scheduler.typing_guard.set_pause_ms(600);
        scheduler.typing_guard.note();
        assert!(
            scheduler.typing_guard.hotkeys_locked(),
            "note() must arm the lockout"
        );
        assert!(
            !scheduler.typing_allows_start(),
            "start must be forbidden while typing"
        );

        // Lockout expired: the decision flips back. Re-arm from scratch with
        // a zero window so no wall-clock sleep is needed.
        scheduler.typing_guard.set_pause_ms(0);
        assert!(scheduler.typing_allows_start());
    }

    #[test]
    fn test_visual_ripple_config_toggle() {
        let scheduler = ClickScheduler::new();
        // Default must be true
        assert!(scheduler.get_config().visual_ripple);

        // Turn off
        let mut cfg = scheduler.get_config();
        cfg.visual_ripple = false;
        scheduler.set_config(cfg);
        assert!(!scheduler.get_config().visual_ripple);

        // Turn back on
        let mut cfg2 = scheduler.get_config();
        cfg2.visual_ripple = true;
        scheduler.set_config(cfg2);
        assert!(scheduler.get_config().visual_ripple);
    }

    #[test]
    fn bates_jitter_moments_and_strict_bounds() {
        use rand::SeedableRng;
        // Deterministic seed: stable in CI, no flake.
        let mut rng = rand::rngs::StdRng::seed_from_u64(0xC10C_0001);
        let p = 0.35f64;
        let n = 10_000usize;
        let mut sum = 0.0f64;
        let mut sum_sq = 0.0f64;
        let mut out_of_bounds = 0usize;
        for _ in 0..n {
            let f = bates_jitter_factor(&mut rng, p);
            if f < -p || f > p {
                out_of_bounds += 1;
            }
            sum += f;
            sum_sq += f * f;
        }
        let mean = sum / n as f64;
        let var = sum_sq / n as f64 - mean * mean;
        // Mean ≈ 0 within ±0.5% of the scale.
        assert!(mean.abs() < 0.005 * p, "Bates mean drifted: {mean}");
        // Variance ≈ p²/9 within ±15% (uniform would be p²/3 — 3x wider).
        let expected = p * p / 9.0;
        let rel = ((var - expected) / expected).abs();
        assert!(rel < 0.15, "Bates variance off: got {var}, want ~{expected}");
        // Hard bound: ZERO escapes in 10k draws — no clamp ever needed.
        assert_eq!(out_of_bounds, 0, "Bates escaped [-p, +p]");
    }

    #[test]
    fn bates_jitter_zero_pct_is_exact() {
        use rand::SeedableRng;
        let mut rng = rand::rngs::StdRng::seed_from_u64(1);
        assert_eq!(bates_jitter_factor(&mut rng, 0.0), 0.0);
        assert_eq!(bates_jitter_factor(&mut rng, -0.5), 0.0);
        // NaN guard: `!(NaN > 0.0)` is true → exact 0.0, never NaN out.
        assert_eq!(bates_jitter_factor(&mut rng, f64::NAN), 0.0);
    }

    #[test]
    fn outlier_prob_zero_is_passthrough() {
        use rand::SeedableRng;
        let mut rng = rand::rngs::StdRng::seed_from_u64(7);
        for _ in 0..1000 {
            assert_eq!(outlier_interval_ns(&mut rng, 80_000_000, 81_000_000, 0.0), 81_000_000);
        }
        // Negative / NaN / infinite also mean "no hesitation", never a stretch
        // (and never a panic — NaN.clamp() would panic in core).
        assert_eq!(outlier_interval_ns(&mut rng, 80_000_000, 81_000_000, -0.5), 81_000_000);
        assert_eq!(outlier_interval_ns(&mut rng, 80_000_000, 81_000_000, f64::NAN), 81_000_000);
        assert_eq!(outlier_interval_ns(&mut rng, 80_000_000, 81_000_000, f64::INFINITY), 81_000_000);
    }

    #[test]
    fn outlier_rate_and_shape() {
        use rand::SeedableRng;
        let base: i64 = 80_000_000; // 12.5 CPS — the human reference point
        // 100k draws: 2% binomial sigma is ~0.14%, so ±0.5% is ~3.5 sigma.
        // (20k proved too seed-sensitive in review: one honest seed read
        // 1.68%, inside 1.5-2.5% only by luck of the draw.)
        let n = 100_000usize;
        let mut rng = rand::rngs::StdRng::seed_from_u64(0x0E57_0001);
        let mut hits = 0usize;
        let mut min_k = f64::MAX;
        let mut max_k = f64::MIN;
        let mut sum = 0i64;
        for _ in 0..n {
            let out = outlier_interval_ns(&mut rng, base, base, 0.02);
            sum += out;
            if out != base {
                hits += 1;
                let k = out as f64 / base as f64;
                min_k = min_k.min(k);
                max_k = max_k.max(k);
            }
        }
        let rate = hits as f64 / n as f64;
        assert!(rate > 0.015 && rate < 0.025, "outlier rate drifted: {rate}");
        assert!(min_k >= 2.0 && max_k <= 3.0, "outlier shape escaped [2x,3x]: {min_k}..{max_k}");
        // Mean dips honestly ~3%: E[k] = 0.98*1 + 0.02*2.5 = 1.03.
        // Never compensated — the dip IS the humanity.
        let mean_k = sum as f64 / n as f64 / base as f64;
        assert!(mean_k > 1.015 && mean_k < 1.045, "mean shift off: {mean_k}");
    }

    #[test]
    fn outlier_prob_roundtrips_through_config() {
        let scheduler = ClickScheduler::new();
        assert!((scheduler.get_config().outlier_prob - 0.02).abs() < 1e-12);
        assert_eq!(scheduler.get_config().technique, "auto");
        let mut cfg = scheduler.get_config();
        cfg.outlier_prob = 0.5; // above ceiling
        cfg.technique = "DRAG ".into();
        scheduler.set_config(cfg);
        assert!((scheduler.get_config().outlier_prob - 0.10).abs() < 1e-12);
        assert_eq!(scheduler.get_config().technique, "drag");
        let mut cfg2 = scheduler.get_config();
        cfg2.outlier_prob = 0.0; // switch-off survives
        scheduler.set_config(cfg2);
        assert_eq!(scheduler.get_config().outlier_prob, 0.0);
    }
}

#[cfg(test)]
mod focus_guard_tests {
    use super::*;

    /// Disabled guard (default config) is transparent: a foreground change
    /// never pauses, and arming while disabled stores nothing.
    #[test]
    fn disabled_guard_never_pauses() {
        let g = FocusGuard::new(false);
        assert!(!g.is_enabled());
        g.arm(Some("game.exe".into()));
        assert!(!g.should_pause(Some("game.exe")));
        assert!(!g.should_pause(Some("browser.exe")));
        assert!(!g.should_pause(None));
    }

    /// Core contract: same exe as at run start → keep going; a DIFFERENT
    /// exe mid-run (Alt+Tab, click-away, Win key) → pause.
    #[test]
    fn same_exe_continues_changed_exe_pauses() {
        let g = FocusGuard::new(true);
        g.arm(Some("game.exe".into()));
        assert!(!g.should_pause(Some("game.exe")));
        assert!(
            !g.should_pause(Some("GAME.EXE")),
            "match must be case-insensitive"
        );
        assert!(g.should_pause(Some("browser.exe")));
        assert!(g.should_pause(Some("explorer.exe")));
    }

    /// Fail-open: an unresolvable foreground (lock screen, elevated process)
    /// never pauses, and an enabled guard without a baseline never pauses.
    #[test]
    fn fail_open_on_unknown_foreground_and_missing_baseline() {
        let g = FocusGuard::new(true);
        // No arm() at all: no baseline → fail-open.
        assert!(!g.should_pause(Some("browser.exe")));
        g.arm(Some("game.exe".into()));
        // Baseline present but current unresolvable → fail-open.
        assert!(!g.should_pause(None));
    }

    /// Arm while disabled must NOT seed the baseline: enabling later starts
    /// from a clean slate (first polls fail-open until a fresh arm).
    #[test]
    fn arm_while_disabled_is_ignored() {
        let g = FocusGuard::new(false);
        g.arm(Some("game.exe".into()));
        g.set_enabled(true);
        assert!(!g.should_pause(Some("browser.exe")));
        // A proper arm after enabling fixes the baseline.
        g.arm(Some("game.exe".into()));
        assert!(g.should_pause(Some("browser.exe")));
    }

    /// Disarm clears the baseline (run end / config save contract): the next
    /// decision fails open until a new arm.
    #[test]
    fn disarm_clears_session_baseline() {
        let g = FocusGuard::new(true);
        g.arm(Some("game.exe".into()));
        assert!(g.should_pause(Some("browser.exe")));
        g.disarm();
        assert!(!g.should_pause(Some("browser.exe")));
    }

    /// Toggling the switch OFF clears the baseline too (set_config contract):
    /// a stale session exe must never survive a disable/enable cycle.
    #[test]
    fn set_enabled_false_disarms() {
        let g = FocusGuard::new(true);
        g.arm(Some("game.exe".into()));
        g.set_enabled(false);
        assert!(!g.is_enabled());
        g.set_enabled(true);
        assert!(
            !g.should_pause(Some("browser.exe")),
            "stale baseline must not survive a disable cycle"
        );
    }

    /// Scheduler wiring: set_config() syncs the guard's enabled flag but a
    /// plain save (same flag value — e.g. the stats ~5 s flush while
    /// clicking) keeps the live session baseline. Only a real flag flip
    /// disarms (OFF clears via set_enabled, ON clears a stale baseline).
    #[test]
    fn set_config_syncs_focus_guard_flag_and_disarms() {
        let scheduler = ClickScheduler::new();
        assert!(!scheduler.focus_guard().is_enabled());

        let mut cfg = scheduler.get_config();
        cfg.pause_on_focus_loss = true;
        scheduler.set_config(cfg);
        assert!(scheduler.focus_guard().is_enabled());

        // Arm a session manually, then re-save config with the SAME flag:
        // the live baseline must survive (stats flush must not kill it).
        scheduler.focus_guard().arm(Some("game.exe".into()));
        let cfg2 = scheduler.get_config();
        assert!(cfg2.pause_on_focus_loss);
        scheduler.set_config(cfg2);
        assert!(
            scheduler.focus_guard().should_pause(Some("browser.exe")),
            "set_config with unchanged flag must keep the live session"
        );

        // A real flip OFF clears the baseline via set_enabled(false).
        let mut cfg3 = scheduler.get_config();
        cfg3.pause_on_focus_loss = false;
        scheduler.set_config(cfg3);
        assert!(!scheduler.focus_guard().is_enabled());
        assert!(!scheduler.focus_guard().should_pause(Some("browser.exe")));
    }

    /// Own-exe contract (user spec): our window is a real baseline AND a
    /// fail-open current. Leaving NanoClick stops; returning never pauses.
    #[test]
    fn own_exe_is_baseline_and_fail_open_current() {
        use crate::platform::own_exe_name;
        let Some(own) = own_exe_name() else {
            return; // off-Windows fallback path: nothing to assert
        };
        let g = FocusGuard::new(true);
        g.arm(Some(own.clone()));
        // Returning to ourselves → no pause.
        assert!(!g.should_pause(Some(own.as_str())));
        let upper = own.to_ascii_uppercase();
        assert!(!g.should_pause(Some(upper.as_str())));
        // `None` (lock screen / elevated) → no pause.
        assert!(!g.should_pause(None));
        // Any other app — including the Alt+Tab switcher — pauses.
        assert!(g.should_pause(Some("game.exe")));
        assert!(g.should_pause(Some("explorer.exe")));
        // And the reverse: game baseline + back to NanoClick → no pause.
        g.arm(Some("game.exe".into()));
        assert!(!g.should_pause(Some(own.as_str())));
        assert!(g.should_pause(Some("browser.exe")));
    }

    /// `arm(None)` never wipes a live baseline (unresolvable foreground at
    /// arm time is not a baseline).
    #[test]
    fn arm_none_keeps_existing_baseline() {
        let g = FocusGuard::new(true);
        g.arm(None);
        assert!(!g.should_pause(Some("browser.exe")));
        g.arm(Some("game.exe".into()));
        g.arm(None);
        assert!(g.should_pause(Some("browser.exe")));
        assert!(!g.should_pause(Some("game.exe")));
    }

    /// Source-level structural test (codebase convention): the focus check
    /// must sit AFTER the app filter choke and BEFORE any dispatch branch
    /// (hold / sequence / single), and arm() must happen before the loop
    /// starts — otherwise the first clicks escape the guard.
    #[test]
    fn focus_check_gates_every_dispatch_branch() {
        let src = include_str!("scheduler.rs");
        let filter_off = src
            .find("── SMART GUARD: APP FILTER")
            .expect("app filter choke not present");
        let focus_off = src
            .find("── SMART GUARD: FOCUS LOSS AUTO-PAUSE")
            .expect("focus loss choke not present");
        let hold_off = src
            .find("── HOLD CLICK LOGIC")
            .expect("hold dispatch branch not present");
        assert!(
            filter_off < focus_off,
            "focus check must come after the app filter choke"
        );
        assert!(
            focus_off < hold_off,
            "focus check must gate every dispatch branch"
        );

        let arm_off = src.find("focus_guard_arc.arm(").expect("arm() missing");
        let loop_off = src
            .find("// ── DECOUPLED ASYNCHRONOUS TELEMETRY WORKER")
            .expect("telemetry marker missing");
        assert!(
            arm_off < loop_off,
            "arm() must happen before the click loop starts"
        );
    }
}

/// ── Hotkey stop-path regression suite ────────────────────────────────
/// Covers the field reports where the toggle "kept clicking" or "switched
/// itself off": a fast double tap, a tap 300 ms apart, and a tap while the
/// typing lockout was armed.
///
/// The suite drives the REAL `decide_toggle` gates and the REAL debounce
/// bookkeeping through [`ToggleSim`], but never calls
/// `ClickScheduler::set_active(true)` — that would spawn a live click worker
/// and actually press the developer's mouse buttons.
#[cfg(test)]
mod hotkey_stop_path_tests {
    use super::*;
    use std::sync::{Arc, Mutex as StdMutex};
    use std::time::{Duration, Instant};

    /// Deterministic mirror of the hook → scheduler toggle path.
    struct ToggleSim {
        active: bool,
        work_mode: bool,
        typing_locked: bool,
        debounce_ms: u32,
        last_toggle: Arc<StdMutex<Option<Instant>>>,
    }

    impl ToggleSim {
        fn new(debounce_ms: u32) -> Self {
            ToggleSim {
                active: false,
                work_mode: false,
                typing_locked: false,
                debounce_ms,
                last_toggle: Arc::new(StdMutex::new(None)),
            }
        }

        /// One hook press: a key-down that matched the toggle combo.
        fn press(&mut self) -> ToggleOutcome {
            // Production rule: the debounce window is consulted ONLY on the
            // start path — a stop must never be refusable.
            let debounce_ok = self.active
                || ClickScheduler::toggle_debounce_check(&self.last_toggle, self.debounce_ms);
            let outcome = decide_toggle(
                self.active,
                self.work_mode,
                self.typing_locked,
                debounce_ok,
            );
            match outcome {
                ToggleOutcome::Start => self.active = true,
                ToggleOutcome::Stop => self.active = false,
                _ => {}
            }
            outcome
        }
    }

    #[test]
    fn press_from_idle_starts_the_clicker() {
        let mut sim = ToggleSim::new(80);
        assert_eq!(sim.press(), ToggleOutcome::Start);
        assert!(sim.active, "the first press must start clicking");
    }

    /// The exact user scenario: start, then press again 300 ms later to stop.
    #[test]
    fn press_again_300_ms_later_stops_the_clicker() {
        let mut sim = ToggleSim::new(80);
        assert_eq!(sim.press(), ToggleOutcome::Start);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(sim.press(), ToggleOutcome::Stop);
        assert!(!sim.active, "300 ms later the second press must stop clicking");
    }

    /// Debounce must never swallow a stop, even on an ultra-fast second tap.
    #[test]
    fn press_within_debounce_window_still_stops_the_clicker() {
        let mut sim = ToggleSim::new(5000); // absurdly long window, on purpose
        assert_eq!(sim.press(), ToggleOutcome::Start);
        std::thread::sleep(Duration::from_millis(10));
        assert_eq!(sim.press(), ToggleOutcome::Stop, "a stop is never debounced");
        assert!(!sim.active);
    }

    #[test]
    fn stop_is_never_gated_by_the_typing_lockout() {
        let mut sim = ToggleSim::new(80);
        assert_eq!(sim.press(), ToggleOutcome::Start);
        // A text key landed mid-run: the clicker is stopped by the kill-switch
        // and the lockout is armed.
        sim.typing_locked = true;
        assert_eq!(
            sim.press(),
            ToggleOutcome::Stop,
            "the user must always be able to stop the clicker"
        );
        assert!(!sim.active);
    }

    #[test]
    fn stop_wins_even_while_work_mode_is_engaged() {
        let mut sim = ToggleSim::new(80);
        assert_eq!(sim.press(), ToggleOutcome::Start);
        sim.work_mode = true; // switching to Work Mode must not block the stop
        assert_eq!(sim.press(), ToggleOutcome::Stop);
        assert!(!sim.active);
    }

    #[test]
    fn idle_press_in_work_mode_is_ignored() {
        let mut sim = ToggleSim::new(80);
        sim.work_mode = true;
        assert_eq!(sim.press(), ToggleOutcome::IgnoredWorkMode);
        assert!(!sim.active, "Work Mode is a safety lock: never start there");
    }

    #[test]
    fn typing_lockout_blocks_the_start() {
        let mut sim = ToggleSim::new(80);
        sim.typing_locked = true;
        assert_eq!(sim.press(), ToggleOutcome::BlockedByTyping);
        assert!(!sim.active, "`r` inside a word must not start clicking");
    }

    #[test]
    fn start_right_after_a_stop_is_debounced() {
        let mut sim = ToggleSim::new(200);
        assert_eq!(sim.press(), ToggleOutcome::Start);
        assert_eq!(sim.press(), ToggleOutcome::Stop);
        // The window was armed by the START, so a third tap cannot flip the
        // clicker back on right after the stop.
        assert_eq!(sim.press(), ToggleOutcome::BlockedByDebounce);
        assert!(!sim.active);
    }

    #[test]
    fn start_is_allowed_once_the_debounce_window_elapses() {
        let mut sim = ToggleSim::new(40);
        assert_eq!(sim.press(), ToggleOutcome::Start);
        assert_eq!(sim.press(), ToggleOutcome::Stop);
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(sim.press(), ToggleOutcome::Start);
        assert!(sim.active);
    }

    /// Regression for the field reports: a burst of fast taps must never
    /// leave the clicker running when the user's last intent was "stop".
    #[test]
    fn rapid_taps_never_leave_the_clicker_inverted() {
        let mut sim = ToggleSim::new(80);
        assert_eq!(sim.press(), ToggleOutcome::Start);
        assert_eq!(sim.press(), ToggleOutcome::Stop);
        for _ in 0..5 {
            assert_eq!(sim.press(), ToggleOutcome::BlockedByDebounce);
        }
        assert!(
            !sim.active,
            "the clicker must stay stopped — an inverted toggle is the bug we fix"
        );
    }

    #[test]
    fn blocked_start_does_not_consume_the_debounce_window() {
        let mut sim = ToggleSim::new(500);
        assert_eq!(sim.press(), ToggleOutcome::Start);
        assert_eq!(sim.press(), ToggleOutcome::Stop);

        let before = *sim.last_toggle.lock().unwrap();
        assert_eq!(sim.press(), ToggleOutcome::BlockedByDebounce);
        let after = *sim.last_toggle.lock().unwrap();
        assert_eq!(before, after, "a refused start must not extend the window");

        std::thread::sleep(Duration::from_millis(520));
        assert_eq!(sim.press(), ToggleOutcome::Start);
    }

    /// Locks the documented gate order so a future refactor cannot reorder it:
    /// STOP is absolute, then Work Mode, then typing, then debounce.
    #[test]
    fn every_gate_combination_follows_the_documented_order() {
        use ToggleOutcome::*;
        let cases: [(bool, bool, bool, bool, ToggleOutcome); 16] = [
            (true, false, false, true, Stop),
            (true, false, false, false, Stop),
            (true, false, true, true, Stop),
            (true, false, true, false, Stop),
            (true, true, false, true, Stop),
            (true, true, false, false, Stop),
            (true, true, true, true, Stop),
            (true, true, true, false, Stop),
            (false, true, false, true, IgnoredWorkMode),
            (false, true, true, true, IgnoredWorkMode),
            (false, true, false, false, IgnoredWorkMode),
            (false, true, true, false, IgnoredWorkMode),
            (false, false, true, true, BlockedByTyping),
            (false, false, true, false, BlockedByTyping),
            (false, false, false, false, BlockedByDebounce),
            (false, false, false, true, Start),
        ];
        for (active, work, typing, debounce, expected) in cases {
            assert_eq!(
                decide_toggle(active, work, typing, debounce),
                expected,
                "active={active} work={work} typing={typing} debounce={debounce}"
            );
        }
    }

    /// A refused start must leave no trace: once the user stops typing, the
    /// clicker starts normally on the next press.
    #[test]
    fn typing_blocked_press_leaves_the_clicker_idle_and_recoverable() {
        let mut sim = ToggleSim::new(80);
        sim.typing_locked = true;
        assert_eq!(sim.press(), ToggleOutcome::BlockedByTyping);
        assert!(!sim.active);

        sim.typing_locked = false;
        // The refused press consumed the window (production behaviour); wait
        // it out, then the clicker starts on the first press.
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(sim.press(), ToggleOutcome::Start);
        assert!(sim.active);
    }

    /// The real scheduler must delegate its start gate to the same pure
    /// decision the simulator uses — otherwise the tests could pass while
    /// production drifts. Never calls `set_active(true)` (no worker spawn).
    #[test]
    fn scheduler_start_gate_matches_the_pure_decision() {
        let scheduler = ClickScheduler::new();
        scheduler.typing_guard().note();
        assert!(scheduler.typing_guard().hotkeys_locked());

        // What `set_active(true)` consults while the lockout is armed:
        assert_eq!(
            decide_toggle(
                false,
                !scheduler.is_autoclicker_mode(),
                !scheduler.typing_allows_start(),
                true,
            ),
            ToggleOutcome::BlockedByTyping
        );

        // …and the stop gate stays wide open for a running clicker.
        assert_eq!(
            decide_toggle(true, !scheduler.is_autoclicker_mode(), true, true),
            ToggleOutcome::Stop
        );

        // Work Mode refuses to start even when nothing is typed.
        let idle = ClickScheduler::new();
        assert_eq!(
            decide_toggle(false, true, !idle.typing_allows_start(), true),
            ToggleOutcome::IgnoredWorkMode
        );
    }

    /// `get_config()` must report what `set_config()` was given — it used to
    /// answer a hardcoded `gui_lock_ms: 1500` and never stored the toggle
    /// response time at all, so the Settings sliders could not take effect on a
    /// running app (the hotkey layer re-parses bindings from this snapshot).
    #[test]
    fn get_config_reports_the_stored_engine_timings() {
        let scheduler = ClickScheduler::new();
        let mut cfg = scheduler.get_config();
        assert_eq!(cfg.gui_lock_ms, 1500, "default comes from the config");

        cfg.gui_lock_ms = 2400;
        cfg.hotkey_debounce_ms = 35;
        scheduler.set_config(cfg);

        let after = scheduler.get_config();
        assert_eq!(after.gui_lock_ms, 2400, "gui_lock_ms must survive set_config");
        assert_eq!(
            after.hotkey_debounce_ms, 35,
            "the response-time slider must take effect without a restart"
        );
    }

    /// `set_active` must REPORT a veto instead of swallowing it: the tray menu
    /// and the UI button both hand that value to the page, and a silent refusal
    /// is what made a tray item look like a dead binding (the clicker ignored the
    /// request and nothing anywhere changed).
    ///
    /// Both cases below are refusals, so no worker is ever spawned — this test
    /// presses nothing on the developer's mouse.
    #[test]
    fn set_active_reports_the_veto_reason_to_its_caller() {
        // Work Mode: the safety lock refuses every start request.
        let work = ClickScheduler::new();
        work.toggle_mode(None);
        assert!(!work.is_autoclicker_mode());
        assert_eq!(work.set_active(true, None), ToggleOutcome::IgnoredWorkMode);
        assert!(!work.is_active(), "a vetoed start must not leave a run behind");
        // …but STOP stays absolute, exactly as `decide_toggle` promises.
        assert_eq!(work.set_active(false, None), ToggleOutcome::Stop);

        // Typing guard: the `r` in "go rush B" must not start a run either.
        let typing = ClickScheduler::new();
        typing.typing_guard().note();
        assert!(typing.typing_guard().hotkeys_locked());
        assert_eq!(typing.set_active(true, None), ToggleOutcome::BlockedByTyping);
        assert!(!typing.is_active());
    }

    #[test]
    fn activate_preset_hotkey_preempts_running_preset() {
        let scheduler = ClickScheduler::new();
        let p1 = crate::config_manager::PresetItem {
            id: "preset_1".to_string(),
            name: "Preset 1".to_string(),
            target_cps: 50.0,
            button: "right".to_string(),
            start_delay_ms: 60_000,
            ..Default::default()
        };
        let p2 = crate::config_manager::PresetItem {
            id: "preset_2".to_string(),
            name: "Preset 2".to_string(),
            target_cps: 12.0,
            button: "middle".to_string(),
            start_delay_ms: 60_000,
            ..Default::default()
        };
        scheduler.update_presets(vec![p1, p2]);

        // 1. Idle -> activate preset_1: starts running preset_1
        assert_eq!(
            scheduler.activate_preset_hotkey("preset_1", None),
            ToggleOutcome::Start
        );
        assert!(scheduler.is_active());
        assert_eq!(
            scheduler.current_running_preset_id().as_deref(),
            Some("preset_1")
        );
        assert_eq!(scheduler.get_config().button, "right");
        assert!((scheduler.get_config().cps - 50.0).abs() < f64::EPSILON);

        // 2. Running preset_1 -> activate preset_2: preempts immediately to preset_2!
        assert_eq!(
            scheduler.activate_preset_hotkey("preset_2", None),
            ToggleOutcome::Start
        );
        assert!(scheduler.is_active());
        assert_eq!(
            scheduler.current_running_preset_id().as_deref(),
            Some("preset_2")
        );
        assert_eq!(scheduler.get_config().button, "middle");
        assert!((scheduler.get_config().cps - 12.0).abs() < f64::EPSILON);

        // 3. Running preset_2 -> activate preset_2 again: toggle stops!
        assert_eq!(
            scheduler.activate_preset_hotkey("preset_2", None),
            ToggleOutcome::Stop
        );
        assert!(!scheduler.is_active());
        assert_eq!(scheduler.current_running_preset_id(), None);
    }

    #[test]
    fn test_dynamic_hold_time_safety_cap_across_all_cps() {
        let mut rng = rand::thread_rng();
        // Check CPS from 1 to 100
        for cps_int in 1..=100 {
            let cps = cps_int as f64;
            let target_interval_ms = 1000.0 / cps;
            let max_allowed = target_interval_ms * 0.40;

            for _ in 0..50 {
                let hold_ms = calc_dynamic_hold_time(&mut rng, target_interval_ms);
                // Must never exceed 40% of the full period
                assert!(
                    hold_ms <= max_allowed + 1e-6,
                    "CPS {}: hold_ms {} exceeded 40% max_allowed {}",
                    cps,
                    hold_ms,
                    max_allowed
                );
                // Must be at least 2ms for OS recognition
                assert!(
                    hold_ms >= 2.0,
                    "CPS {}: hold_ms {} was less than hardware 2ms minimum",
                    cps,
                    hold_ms
                );
                // At normal/low CPS (e.g. 10 CPS, period 100ms, 40% = 40ms), minimum should respect 8ms
                if max_allowed >= 8.0 {
                    assert!(
                        hold_ms >= 8.0,
                        "CPS {}: hold_ms {} was less than 8ms floor",
                        cps,
                        hold_ms
                    );
                }
            }
        }
    }

    #[test]
    fn test_single_timing_generates_valid_intervals_and_holds() {
        let mut rng = rand::thread_rng();
        for cps in [5.0, 10.0, 20.0, 50.0, 100.0] {
            let target_interval_ms = 1000.0 / cps;
            for _ in 0..50 {
                let (hold_ms, release_ms) =
                    calc_single_timing(&mut rng, target_interval_ms, 20.0, 0.02);
                assert!(hold_ms >= 2.0);
                assert!(release_ms >= 2.0);
                assert!(hold_ms <= target_interval_ms * 0.40 + 1e-6);
            }
        }
    }

    #[test]
    fn test_butterfly_engine_bimodal_distribution() {
        let mut rng = rand::thread_rng();
        let mut engine = ButterflyEngine::new();
        let target_interval_ms = 50.0; // 20 CPS

        let mut bounce_count = 0;
        let mut finger_count = 0;

        for _ in 0..500 {
            let (hold_ms, release_ms) = engine.next_timing(&mut rng, target_interval_ms);
            let total_interval = hold_ms + release_ms;
            if total_interval <= 35.0 {
                bounce_count += 1;
            } else {
                finger_count += 1;
            }
            assert!(hold_ms >= 2.0);
            assert!(release_ms >= 2.0);
        }

        // Bimodal validation: both bounce (~35%) and alternating finger strikes must be present
        assert!(
            bounce_count >= 50,
            "Expected debounce bounces, got {}",
            bounce_count
        );
        assert!(
            finger_count >= 150,
            "Expected alternating finger strikes, got {}",
            finger_count
        );
    }

    #[test]
    fn test_drag_click_burst_and_reset_deadzone() {
        let mut rng = rand::thread_rng();
        let mut engine = DragClickEngine::new();
        let target_interval_ms = 66.6; // 15 CPS

        let mut has_reset_deadzone = false;
        let mut has_fast_friction_click = false;

        for _ in 0..100 {
            let (hold_ms, release_ms) = engine.next_timing(&mut rng, target_interval_ms);
            if release_ms >= 150.0 {
                has_reset_deadzone = true;
            } else if release_ms <= 30.0 {
                has_fast_friction_click = true;
            }
            assert!(hold_ms >= 2.0);
        }

        assert!(
            has_reset_deadzone,
            "Drag click engine must produce hand return deadzone (>150ms)"
        );
        assert!(
            has_fast_friction_click,
            "Drag click engine must produce fast friction burst clicks"
        );
    }

    #[test]
    fn test_drag_click_slider_scales_deadzone() {
        let mut rng = rand::thread_rng();
        let mut engine_slow = DragClickEngine::new();
        let mut engine_fast = DragClickEngine::new();

        // 10 CPS vs 25 CPS
        let slow_interval = 100.0;
        let fast_interval = 40.0;

        // Draw multiple cycles and collect reset deadzones (the largest release_ms in each cycle)
        let mut max_slow_release = 0.0f64;
        let mut max_fast_release = 0.0f64;

        for _ in 0..50 {
            let (_, release) = engine_slow.next_timing(&mut rng, slow_interval);
            if release > max_slow_release {
                max_slow_release = release;
            }
        }
        for _ in 0..50 {
            let (_, release) = engine_fast.next_timing(&mut rng, fast_interval);
            if release > max_fast_release {
                max_fast_release = release;
            }
        }

        assert!(
            max_slow_release > max_fast_release,
            "10 CPS should have larger hand reset deadzone than 25 CPS (slow: {}ms, fast: {}ms)",
            max_slow_release,
            max_fast_release
        );
    }

    #[test]
    fn test_all_techniques_reach_high_cps() {
        let mut rng = rand::thread_rng();
        let target_interval_ms = 1000.0 / 160.0; // 6.25ms for 160 CPS

        // 1. Single timing at 160 CPS
        for _ in 0..20 {
            let (hold, release) = calc_single_timing(&mut rng, target_interval_ms, 10.0, 0.02);
            assert!(hold >= 2.0, "Single hold at 160 CPS must be >= 2ms, got {}", hold);
            assert!(release >= 2.0, "Single release at 160 CPS must be >= 2ms, got {}", release);
            assert!(hold + release <= 8.5, "Single cycle at 160 CPS must be tight, got {}", hold + release);
        }

        // 2. Butterfly timing at 160 CPS
        let mut b_engine = ButterflyEngine::new();
        for _ in 0..20 {
            let (hold, release) = b_engine.next_timing(&mut rng, target_interval_ms);
            assert!(hold >= 2.0, "Butterfly hold at 160 CPS must be >= 2ms, got {}", hold);
            assert!(release >= 2.0, "Butterfly release at 160 CPS must be >= 2ms, got {}", release);
            assert!(hold + release <= 8.5, "Butterfly cycle at 160 CPS must be tight, got {}", hold + release);
        }

        // 3. Drag timing at 160 CPS
        let mut d_engine = DragClickEngine::new();
        for _ in 0..20 {
            let (hold, release) = d_engine.next_timing(&mut rng, target_interval_ms);
            assert!(hold >= 2.0, "Drag hold at 160 CPS must be >= 2ms, got {}", hold);
            assert!(release >= 2.0, "Drag release at 160 CPS must be >= 2ms, got {}", release);
        }
    }
}

