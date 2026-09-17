//! Dedicated Overlay Window Manager for NanoClick.
//!
//! Manages the full-screen transparent click-through phantom window used for
//! the hardware-accelerated Visual Click Ripple effect.
//!
//! Key responsibilities:
//! 1. Window lifecycle (Zero-Flash initialization: initially hidden, shown once transparent DOM is ready).
//! 2. Platform click-through configuration (`set_ignore_cursor_events(true)` / `WS_EX_TRANSPARENT`).
//! 3. Real-time click ripple event emission with screen cursor coordinates.

use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{AppHandle, Emitter, Manager};

static LAST_RIPPLE_EMIT_MS: AtomicU64 = AtomicU64::new(0);

/// Emit a click ripple event to the overlay window with physical screen coordinates (x, y).
/// The frontend overlay corrects for Per-Monitor DPI scaling via `window.devicePixelRatio`.
/// Throttled to ~30 FPS (33ms) so high CPS click spam never saturates the IPC bridge or WebView2 memory.
pub fn emit_click_ripple(app: &AppHandle, x: i32, y: i32) {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let last = LAST_RIPPLE_EMIT_MS.load(Ordering::Relaxed);
    if now_ms.saturating_sub(last) < 33 {
        return;
    }
    LAST_RIPPLE_EMIT_MS.store(now_ms, Ordering::Relaxed);

    if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.emit("spawn-ripple", (x, y));
    }
}

/// Initialize the overlay window during application setup.
/// Ensures the window starts click-through so it never traps mouse events.
pub fn setup_overlay(app: &AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("overlay") {
        // Enforce click-through from the start
        let _ = win.set_ignore_cursor_events(true);
        crate::debug_log_internal("info", "[Overlay] setup completed with click-through enabled");
    }
    Ok(())
}

/// Invoked by `overlay.js` once the transparent DOM has finished loading.
/// Showing the window only after this signal prevents the standard DWM/WebView2
/// white background buffer flash.
#[tauri::command]
pub fn overlay_ready(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.set_ignore_cursor_events(true);
        let _ = win.show();
        crate::debug_log_internal("info", "[Overlay] ready signal received; overlay window visible and click-through");
    }
    Ok(())
}

/// Show or hide the overlay window dynamically (e.g. when toggling Visual Click Ripple in settings).
#[tauri::command]
pub fn toggle_overlay(app: AppHandle, show: bool) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("overlay") {
        if show {
            let _ = win.set_ignore_cursor_events(true);
            let _ = win.show();
            crate::debug_log_internal("info", "[Overlay] overlay shown");
        } else {
            let _ = win.hide();
            crate::debug_log_internal("info", "[Overlay] overlay hidden");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_overlay_module_exists() {
        // Basic compile and sanity check
        assert!(true);
    }

    #[test]
    fn test_ripple_coords_tuple_serialization() {
        let coords = (1920, 1080);
        let serialized = serde_json::to_string(&coords).expect("Tuple should serialize");
        assert_eq!(serialized, "[1920,1080]");

        let (x, y): (i32, i32) = serde_json::from_str(&serialized).expect("Tuple should deserialize");
        assert_eq!(x, 1920);
        assert_eq!(y, 1080);
    }
}
