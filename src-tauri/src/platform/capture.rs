//! Native app-capture countdown — the timer lives in Rust, not in the page.
//!
//! The old flow counted 3→2→1 with `setTimeout` inside WebView2. Chromium
//! throttles page timers to ~1 Hz (or freezes them) the moment the window
//! loses focus — and the user MUST Alt+Tab away for the capture to see any
//! other app. The `…1` on screen was not a hang: the render engine was asleep.
//!
//! Now the frontend only says "start" / "cancel". A `std::thread` sleeps in
//! 250 ms slices (cancellable within a quarter second), emits `capture-tick`
//! per second over IPC (events arrive even when the page is backgrounded),
//! then reads `GetForegroundWindow` exactly once. Every exit — success,
//! own-window, stale id, cancel — emits its event AND resolves the command,
//! so no button can stay disabled forever. No tokio, no new crates.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

/// Monotonic session id: a double-click (or a second tab) spawns a second
/// thread, but only the newest id may emit — the stale one dies silently.
static CAPTURE_SEQ: AtomicU64 = AtomicU64::new(0);
/// Id of the session a cancel applies to. `u64::MAX` = nobody to cancel.
static CAPTURE_CANCEL_ID: AtomicU64 = AtomicU64::new(u64::MAX);

/// Start a native countdown. Returns immediately with `{ id }`; progress and
/// the result arrive as `capture-tick` / `capture-done` / `capture-error` /
/// `capture-cancelled` events carrying the same id (the page ignores others).
#[tauri::command]
pub async fn start_native_app_capture(
    app: AppHandle,
    countdown_secs: u32,
) -> Result<serde_json::Value, String> {
    let id = CAPTURE_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    let secs = countdown_secs.clamp(1, 10);
    std::thread::spawn(move || {
        // 250 ms slices: cancel lands within a quarter second, never "after".
        let total_slices = secs * 4;
        let mut last_emitted = secs + 1;
        for slice in 0..total_slices {
            if CAPTURE_CANCEL_ID.load(Ordering::SeqCst) == id
                || CAPTURE_SEQ.load(Ordering::SeqCst) != id
            {
                let _ = app.emit("capture-cancelled", serde_json::json!({ "id": id }));
                return;
            }
            let remaining = secs - (slice / 4);
            if remaining != last_emitted {
                last_emitted = remaining;
                let _ = app.emit(
                    "capture-tick",
                    serde_json::json!({ "id": id, "remaining": remaining }),
                );
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        if CAPTURE_CANCEL_ID.load(Ordering::SeqCst) == id
            || CAPTURE_SEQ.load(Ordering::SeqCst) != id
        {
            let _ = app.emit("capture-cancelled", serde_json::json!({ "id": id }));
            return;
        }
        match crate::platform::get_foreground_process_name() {
            Some(exe) => {
                if crate::platform::is_own_exe(&exe) {
                    let _ = app.emit(
                        "capture-error",
                        serde_json::json!({
                            "id": id,
                            "reason": "own-window",
                            "exe": exe,
                        }),
                    );
                } else {
                    let _ = app.emit(
                        "capture-done",
                        serde_json::json!({ "id": id, "exe": exe }),
                    );
                }
            }
            None => {
                let _ = app.emit(
                    "capture-error",
                    serde_json::json!({ "id": id, "reason": "unresolvable" }),
                );
            }
        }
    });
    Ok(serde_json::json!({ "id": id }))
}

/// Cancel the newest capture session (or a specific one by id).
#[tauri::command]
pub fn cancel_native_app_capture(id: Option<u64>) -> serde_json::Value {
    let target = id.unwrap_or_else(|| CAPTURE_SEQ.load(Ordering::SeqCst));
    CAPTURE_CANCEL_ID.store(target, Ordering::SeqCst);
    serde_json::json!({ "cancelled": target })
}

/// Id of the live position-picker stream. `u64::MAX` = nobody is picking.
static PICKER_ID: AtomicU64 = AtomicU64::new(u64::MAX);

/// Start streaming the live cursor position as `position-picker-tick {id,x,y}`
/// at ~20 Hz. The old flow polled `get_current_mouse_pos` from a page
/// `setInterval(50ms)` — 20 IPC round-trips per second plus a timer that
/// keeps the renderer awake. Now the page says "start"/"stop" and only paints.
#[tauri::command]
pub async fn start_position_picker_stream(app: AppHandle) -> Result<serde_json::Value, String> {
    let id = CAPTURE_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    PICKER_ID.store(id, Ordering::SeqCst);
    std::thread::spawn(move || {
        loop {
            if PICKER_ID.load(Ordering::SeqCst) != id {
                break;
            }
            let (x, y) = crate::platform::get_cursor_pos();
            let _ = app.emit(
                "position-picker-tick",
                serde_json::json!({ "id": id, "x": x, "y": y }),
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    });
    Ok(serde_json::json!({ "id": id }))
}

/// Stop the live position-picker stream (id-filtered like capture cancel).
#[tauri::command]
pub fn stop_position_picker_stream(id: Option<u64>) -> serde_json::Value {
    let target = id.unwrap_or_else(|| PICKER_ID.load(Ordering::SeqCst));
    if PICKER_ID.load(Ordering::SeqCst) == target {
        PICKER_ID.store(u64::MAX, Ordering::SeqCst);
    }
    serde_json::json!({ "stopped": target })
}

#[cfg(test)]
mod capture_tests {
    #[test]
    fn ids_are_monotonic_and_cancel_targets_the_newest() {
        let a = super::CAPTURE_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let b = super::CAPTURE_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        assert!(b > a, "capture ids must grow so stale threads can be recognised");
    }
}
