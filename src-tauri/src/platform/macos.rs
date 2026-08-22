//! macOS platform — keyboard, mouse, hooks, cursor polling (stub).

use crate::scheduler::ClickScheduler;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::AppHandle;

pub struct PlatformTimer;

impl PlatformTimer {
    pub fn new() -> Self {
        PlatformTimer
    }

    pub fn wait_until(&self, target: Instant, stop_handle: NativeEventHandle) -> bool {
        loop {
            let now = Instant::now();
            if now >= target {
                return true;
            }
            if stop_handle.load(Ordering::Acquire) {
                return false;
            }
            let remaining = target.duration_since(now);
            let chunk = remaining.min(Duration::from_millis(10));
            thread::sleep(chunk);
        }
    }
}

impl Default for PlatformTimer {
    fn default() -> Self {
        Self::new()
    }
}

pub type NativeEventHandle = Option<Arc<AtomicBool>>;

pub fn create_stop_event() -> Option<NativeEventHandle> {
    Some(Arc::new(AtomicBool::new(false)))
}

pub fn signal_stop_event(handle: Option<NativeEventHandle>) {
    if let Some(h) = handle {
        h.store(true, Ordering::Release);
    }
}

pub fn get_cursor_pos() -> (i32, i32) {
    (0, 0)
}

pub fn click_mouse() -> bool {
    false
}

pub fn click_mouse_ext(
    _button: &str,
    _click_type: &str,
    _position_mode: &str,
    _fixed_x: i32,
    _fixed_y: i32,
    _jitter_radius: u32,
) -> bool {
    false
}

pub fn release_mouse_hold(_button: &str) {}

pub fn spawn_global_hotkey_listener(_scheduler: Arc<ClickScheduler>, _app_handle: AppHandle) {
    // No-op on macOS.
}

pub fn shutdown_global_hotkey_listener() {}

pub fn stop_recorder_hooks() {}
