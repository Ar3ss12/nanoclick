use crate::config::Config;
use crate::platform::{self, backend::ClickSpec, NativeEventHandle, PlatformTimer};
use rand::Rng;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

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
        }
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
        }
    }

    pub fn set_config(&self, cfg: Config) {
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
        // ── v4.2 race hardening (debounce) ────────────────────────────
        // Ignore toggles that arrive faster than hotkey_debounce_ms.
        // Returns true when the toggle should be applied, false when it
        // was swallowed. Updates the timestamp atomically.
        if !Self::toggle_debounce_check(
            &self.last_toggle_instant,
            self.hotkey_debounce_ms.load(Ordering::Relaxed),
        ) {
            return if self.is_active() {
                "autoclicker"
            } else {
                "work"
            }
            .into();
        }

        // State-based decision (not blind inversion): resolve the action
        // from live atomics so a duplicated event can never flip twice.
        let prev_mode = self.mode_autoclicker.load(Ordering::Relaxed);
        let was_active = self.is_active();
        if !prev_mode {
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

        let new_mode = true;
        let new_active = !was_active;

        toggle_diag_push(format!(
            "[Hotkeys] toggle prev_mode={prev_mode} prev_active={was_active}              new_mode={new_mode} new_active={new_active}"
        ));

        self.mode_autoclicker.store(new_mode, Ordering::Relaxed);
        self.set_active(new_active, app_handle);

        let mode_str = if new_mode { "autoclicker" } else { "work" };
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
                    } else if new_mode {
                        "IDLE".into()
                    } else {
                        "WORK MODE (PAUSED)".into()
                    },
                },
            );
        }
        mode_str.to_string()
    }

    pub fn set_active(&self, active: bool, app_handle: Option<&AppHandle>) {
        if active && !self.is_autoclicker_mode() {
            return;
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
        let next = (cur + delta).clamp(1.0, 100.0);
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

    /// Emit the HUD and status updates at most every 40 ms (25 FPS) so high-frequency
    /// CPS clicking loops (e.g. 100 CPS) never saturate the Tauri IPC bridge or cause UI lag.
    fn status_and_hud_maybe_emit(
        app: &AppHandle,
        total: u32,
        mode: &str,
        cps: f64,
        status_text: &str,
        active: bool,
        force: bool,
    ) {
        use std::sync::atomic::AtomicU64;
        static LAST_EMIT: AtomicU64 = AtomicU64::new(0);
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let last = LAST_EMIT.load(Ordering::Relaxed);
        if !force && now_ms.saturating_sub(last) < 40 {
            return;
        }
        LAST_EMIT.store(now_ms, Ordering::Relaxed);
        let _ = app.emit("hud-clicks", total);
        let _ = app.emit(
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

        thread::spawn(move || {
            let mut rng = rand::thread_rng();
            let timer = PlatformTimer::new();
            let platform_backend = platform::default_input_backend();
            let mut next_click: Option<Instant> = None;

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

            // Immediate start notification
            if let Some(ref app) = app_handle {
                let _ = app.emit(
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
                        let total = clicks_done.fetch_add(1, Ordering::Relaxed) + 1;
                        batch_click_count += 1;
                        if let Some(ref app) = app_handle {
                            let mode_str = if mode_autoclicker.load(Ordering::Relaxed) {
                                "autoclicker"
                            } else {
                                "work"
                            };
                            Self::status_and_hud_maybe_emit(app, total, mode_str, 0.0, "HOLDING", true, false);
                        }
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
                let cps = f64::from_bits(cps_raw.load(Ordering::Relaxed)).max(0.1);
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
                        let total = clicks_done.fetch_add(1, Ordering::Relaxed) + 1;
                        batch_click_count += 1;
                        if let Some(ref app) = app_handle {
                            let mode_str = if mode_autoclicker.load(Ordering::Relaxed) {
                                "autoclicker"
                            } else {
                                "work"
                            };
                            Self::status_and_hud_maybe_emit(app, total, mode_str, cps, "RUNNING", true, false);
                        }
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
                    let total = clicks_done.fetch_add(1, Ordering::Relaxed) + 1;
                    batch_click_count += 1;
                    if let Some(ref app) = app_handle {
                        let mode_str = if mode_autoclicker.load(Ordering::Relaxed) {
                            "autoclicker"
                        } else {
                            "work"
                        };
                        Self::status_and_hud_maybe_emit(app, total, mode_str, cps, "RUNNING", true, false);
                    }
                }

                let interval_ns = if deviation_ns > 0 {
                    rng.gen_range((base_ns - deviation_ns)..=(base_ns + deviation_ns))
                } else {
                    base_ns
                };
                let interval = Duration::from_nanos(interval_ns as u64);

                next_click = Some(match next_click {
                    Some(prev) => {
                        let candidate = prev + interval;
                        if Instant::now() > candidate + Duration::from_millis(100) {
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
                Self::status_and_hud_maybe_emit(app, total, mode_str, final_cps, "IDLE", false, true);
            }
        });
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
}
