use crate::config::Config;
use crate::guard::{AppFilter, ForegroundCache, TypingGuard, GUARD_POLL_MS};
use crate::platform::{self, backend::ClickSpec, NativeEventHandle, PlatformTimer};
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

/// Dump and clear the toggle diagnostic buffer (test hooks / debug command).
#[allow(dead_code)]
pub fn toggle_diag_dump() -> Vec<String> {
    if let Ok(mut q) = TOGGLE_DIAG.lock() {
        return q.drain(..).collect();
    }
    Vec::new()
}

/// Focus Guard state machine — owned by the click-loop thread, toggled ON
/// by the UI (`pause_on_focus_loss`). Contract:
/// * `arm()` at run start: remember the foreground exe AS THE ALLOWED ONE
///   (usually our own window — the clicker clicks elsewhere by design).
/// * `poll()` on each loop pass: foreground CHANGED to a different process
///   → auto-pause. `None` (lock screen, elevated) fails OPEN.
/// * `disarm()` on run end / config save. Zero syscalls while disabled.
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
    /// Failure to resolve is NOT an error: poll() fails open until a real
    /// exe is seen (we never pause on what we cannot measure).
    pub fn arm(&self, exe: Option<String>) {
        if !self.is_enabled() {
            return;
        }
        if let Ok(mut g) = self.session_exe.lock() {
            *g = exe;
        }
    }

    /// Disarm at run end / config save (no stale session across runs).
    pub fn disarm(&self) {
        if let Ok(mut g) = self.session_exe.lock() {
            *g = None;
        }
    }

    /// `true` when the foreground app changed away from the session exe.
    /// Fail-open: `None` (unresolvable foreground) never pauses.
    pub fn should_pause(&self, current: Option<&str>) -> bool {
        if !self.is_enabled() {
            return false;
        }
        let Some(cur) = current else {
            return false; // fail-open
        };
        match self.session_exe.lock() {
            Ok(guard) => match guard.as_deref() {
                // Session exe unknown (resolution failed at arm): fail-open
                // until we HAVE a baseline — otherwise first poll pauses.
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
            gui_lock_ms: 1500,
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
        }
    }

    pub fn set_config(&self, cfg: Config) {
        self.visual_ripple.store(cfg.visual_ripple, Ordering::Relaxed);
        self.cps_raw.store(cfg.cps.to_bits(), Ordering::Relaxed);
        self.random_pct_raw
            .store(cfg.random_percent.to_bits(), Ordering::Relaxed);
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
        self.stop_duration_ms
            .store(cfg.stop_duration_ms, Ordering::Relaxed);
        self.stop_time_epoch_sec
            .store(cfg.stop_time_epoch_sec, Ordering::Relaxed);
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
        *self.sequence_points.lock().unwrap() = cfg.sequence_points.clone();
        // Smart Guard state (typing freeze window + app/window scope).
        self.typing_guard.set_pause_ms(cfg.typing_pause_ms);
        self.app_filter
            .set(&cfg.app_filter_mode, &cfg.app_filter_list);
        // Focus Guard: sync the enabled flag; disarm a stale session so a
        // config save never carries an old exe across runs.
        self.focus_guard.set_enabled(cfg.pause_on_focus_loss);
        self.focus_guard.disarm();
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

    pub fn set_active(&self, active: bool, app_handle: Option<&AppHandle>) {
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
                return;
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
            let handle = self.stop_event.lock().unwrap().clone();
            platform::signal_stop_event(handle);
        }
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
    fn spawn_worker(&self, app_handle: Option<AppHandle>) {
        let active = Arc::clone(&self.active);
        let mode_autoclicker = Arc::clone(&self.mode_autoclicker);
        let clicks_done = Arc::clone(&self.clicks_done);
        let cps_raw = Arc::clone(&self.cps_raw);
        let random_pct_raw = Arc::clone(&self.random_pct_raw);
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
        let stop_event_lock = Arc::clone(&self.stop_event);
        let image_trigger_arc = Arc::clone(&self.image_trigger);
        let image_trigger_should_stop = Arc::clone(&self.image_trigger_should_stop);
        let visual_ripple = Arc::clone(&self.visual_ripple);
        let app_filter_arc = Arc::clone(&self.app_filter);
        let focus_guard_arc = Arc::clone(&self.focus_guard);

        thread::spawn(move || {
            #[cfg(target_os = "windows")]
            unsafe {
                use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST};
                let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
            }

            let mut rng = rand::thread_rng();
            let timer = PlatformTimer::new();
            let platform_backend = platform::default_input_backend();
            let mut next_click: Option<Instant> = None;
            // Smart Guard: TTL-memoized foreground lookup for the app filter
            // (2 syscalls, so it must never run per click).
            let fg_cache = ForegroundCache::new();
            // Focus Guard: remember the foreground exe AS THE ALLOWED ONE at
            // run start (hotkey start = the app we are about to click into).
            // Zero syscalls while the guard is disabled; once per run else.
            if focus_guard_arc.is_enabled() {
                focus_guard_arc.arm(fg_cache.exe());
            }

            let cur_button = button_arc.lock().unwrap().clone();
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
            let cur_stop_duration_ms = stop_duration_arc.load(Ordering::Relaxed);
            let cur_stop_time_sec = stop_time_arc.load(Ordering::Relaxed);
            let run_started_at = Instant::now();

            // ── CLICKING LOOP ──────────────────────────────────────────
            while active.load(Ordering::Relaxed) && mode_autoclicker.load(Ordering::Relaxed) {
                // Stop by elapsed duration
                if cur_stop_duration_ms > 0
                    && run_started_at.elapsed() >= Duration::from_millis(cur_stop_duration_ms)
                {
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
                // Alt+Tab / click-away / Win key / system toast moved the
                // foreground to a DIFFERENT process mid-run → stop the
                // clicker so it never keeps hammering the newly focused
                // window. Same choke point as the app filter, so no dispatch
                // branch can slip past. Same TTL cache: one atomic load
                // while disabled, at most the shared 2 syscalls per 300 ms
                // while enabled. `None` (lock screen, elevated) fails OPEN —
                // we never pause on what we cannot measure.
                if focus_guard_arc.is_enabled() {
                    let fg_exe = fg_cache.exe();
                    if focus_guard_arc.should_pause(fg_exe.as_deref()) {
                        active.store(false, Ordering::Relaxed);
                        if let Some(ref app) = app_handle {
                            let _ = app.emit("focus-loss-paused", fg_exe);
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

                let base_ns = (1_000_000_000.0 / cps) as i64;
                let deviation_ns = (base_ns as f64 * (random_pct / 100.0)) as i64;

                if let Some(target) = next_click {
                    let event_handle = stop_event_lock.lock().unwrap().clone().expect("stop_event");
                    let success = timer.wait_until(target, event_handle);
                    if !success || !active.load(Ordering::Relaxed) {
                        break; // Interrupted by stop signal
                    }
                }

                // v4.2 instant stop (single mode): wait_until returned success,
                // but `active` may have flipped between the load() above and us
                // reaching this line — a 1-2 ms gap is enough for the listener
                // thread to call set_active(false). Re-check now so we do not
                // dispatch one phantom click after stop.
                if !active.load(Ordering::Relaxed) {
                    break;
                }
                // ── Multi-point sequence handling ──────────────────────
                // When active_sequence is non-empty we move the cursor to
                // the next point in order before clicking, then wait the
                // point-specific delay before the next iteration.
                if !active_sequence.is_empty() {
                    let idx = (batch_click_count as usize) % active_sequence.len();
                    let p = &active_sequence[idx];
                    platform_backend.set_cursor_pos(p.x, p.y);
                    let mut seq_spec = cur_click_spec.clone();
                    seq_spec.fixed_x = p.x;
                    seq_spec.fixed_y = p.y;
                    seq_spec.points.clear();
                    seq_spec.point_index = 0;
                    if platform_backend.click_mouse(&seq_spec) {
                        clicks_done.fetch_add(1, Ordering::Relaxed);
                        batch_click_count += 1;
                        emit_ripple_if_enabled(&seq_spec);
                        // ZERO-JITTER: no UI/IPC in the hot click loop.
                        // RUNNING telemetry is emitted solely by the decoupled
                        // 66 ms worker (see below) reading lock-free atomics.
                        if p.delay_ms > 0 {
                            let event_handle =
                                stop_event_lock.lock().unwrap().clone().expect("stop_event");
                            let target = Instant::now() + Duration::from_millis(p.delay_ms as u64);
                            let _ = timer.wait_until(target, event_handle);
                            if !active.load(Ordering::Relaxed) {
                                break;
                            }
                        }
                    }
                    // Continue to next iteration so cadence can advance.
                    continue;
                }
                if platform_backend.click_mouse(&cur_click_spec) {
                    clicks_done.fetch_add(1, Ordering::Relaxed);
                    batch_click_count += 1;
                    emit_ripple_if_enabled(&cur_click_spec);
                    // ZERO-JITTER: no UI/IPC in the hot click loop (see sequence branch above).
                }

                let interval_ns = if deviation_ns > 0 {
                    // Bates B3: Gaussian-like tremor, strict ±deviation bounds.
                    let f = bates_jitter_factor(&mut rng, deviation_ns as f64 / base_ns as f64);
                    (base_ns as f64 * (1.0 + f)) as i64
                } else {
                    base_ns
                };
                let interval = Duration::from_nanos(interval_ns as u64);

                next_click = Some(match next_click {
                    Some(prev) => {
                        let candidate = prev + interval;
                        if Instant::now() > candidate {
                            // Cadence drift guard: if a thread hitch or GC pause caused
                            // time to slip past the candidate, reset target from now to
                            // prevent rapid-fire click bursts (which causes web/game click drop).
                            Instant::now() + interval
                        } else {
                            candidate
                        }
                    }
                    None => Instant::now() + interval,
                });
            }

            if cur_click_spec.click_type == crate::platform::backend::ClickType::Hold {
                platform_backend.release_mouse_hold(cur_click_spec.button);
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
            }

            // Focus Guard: the session is over — drop the baseline so the
            // next run re-arms with a fresh foreground. (set_config also
            // disarms on save; this is the authoritative run-end cleanup.)
            focus_guard_arc.disarm();
        });
    }
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

    /// Scheduler wiring: set_config() must sync the guard's enabled flag and
    /// always disarm a stale session (a config save never carries an old exe
    /// across runs).
    #[test]
    fn set_config_syncs_focus_guard_flag_and_disarms() {
        let scheduler = ClickScheduler::new();
        assert!(!scheduler.focus_guard().is_enabled());

        let mut cfg = scheduler.get_config();
        cfg.pause_on_focus_loss = true;
        scheduler.set_config(cfg);
        assert!(scheduler.focus_guard().is_enabled());

        // Arm a session manually, then re-save config: the stale baseline
        // must go (fail-open after the disarm).
        scheduler.focus_guard().arm(Some("game.exe".into()));
        let cfg2 = scheduler.get_config();
        scheduler.set_config(cfg2);
        assert!(
            !scheduler.focus_guard().should_pause(Some("browser.exe")),
            "set_config must disarm a stale session"
        );

        let mut cfg3 = scheduler.get_config();
        cfg3.pause_on_focus_loss = false;
        scheduler.set_config(cfg3);
        assert!(!scheduler.focus_guard().is_enabled());
        assert!(!scheduler.focus_guard().should_pause(Some("browser.exe")));
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
}
