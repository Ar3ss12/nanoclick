//! macOS platform — keyboard, mouse, hooks, cursor polling (stub).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

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

/// v4.2 — all input/hotkey/recorder functionality lives behind the
/// shared contracts (`backend::{InputBackend, HotkeyBackend,
/// RecorderBackend}`); the no-op implementations are `NoopInputBackend`,
/// `NoopHotkeyBackend` and `NoopRecorderBackend` in `platform/mod.rs`.
/// This stub module keeps only the timer + stop-event plumbing that the
/// scheduler uses on every platform.

/// Parse a config button label (stub platform).
pub fn parse_button_label(_label: &str) -> crate::core::action::MouseButton {
    crate::core::action::MouseButton::Left
}

/// Default input backend — capability-honest no-op on this platform.
pub fn default_input_backend() -> std::sync::Arc<dyn crate::platform::backend::InputBackend> {
    std::sync::Arc::new(crate::platform::NoopInputBackend)
}
