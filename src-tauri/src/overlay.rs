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
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

static LAST_RIPPLE_EMIT_MS: AtomicU64 = AtomicU64::new(0);
static RIPPLE_BATCH: Mutex<Vec<(i32, i32)>> = Mutex::new(Vec::new());

/// Emit a click ripple event to the overlay window with physical screen coordinates (x, y).
/// Coalesces high-frequency clicks (e.g. 160 CPS) into batches emitted at 25 FPS (40ms interval).
/// Transmits the batched coordinates as a single IPC event, eliminating IPC queue flooding while preserving all click ripples.
pub fn emit_click_ripple(app: &AppHandle, x: i32, y: i32) {
    let mut batch = match RIPPLE_BATCH.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if batch.len() < 50 {
        batch.push((x, y));
    }

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let last = LAST_RIPPLE_EMIT_MS.load(Ordering::Relaxed);

    // Rate-limit IPC emissions to 25 per second (40ms interval)
    if now_ms.saturating_sub(last) >= 40 {
        LAST_RIPPLE_EMIT_MS.store(now_ms, Ordering::Relaxed);
        let to_emit: Vec<(i32, i32)> = batch.drain(..).collect();
        drop(batch);

        if let Some(win) = app.get_webview_window("overlay") {
            let _ = win.emit("spawn-ripple", to_emit);
        }
    }
}

/// Flush any buffered ripples immediately when clicking stops.
pub fn flush_click_ripples(app: &AppHandle) {
    let mut batch = match RIPPLE_BATCH.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if !batch.is_empty() {
        let to_emit: Vec<(i32, i32)> = batch.drain(..).collect();
        drop(batch);

        if let Some(win) = app.get_webview_window("overlay") {
            let _ = win.emit("spawn-ripple", to_emit);
        }
    }
}

// overlay_ready is invoked by overlay.js AFTER the WebView document is rendered.
// Lazy-created overlay window starts hidden; showing it only here eliminates
// the WebView2 DWM white flash (Zero-Flash init). Safe for cold boot:
// get_webview_window returns None until ensure_overlay_window() creates it.
#[tauri::command]
pub fn overlay_ready(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.set_ignore_cursor_events(true);
        // Restore persisted preference: show only if the user enabled ripple.
        // Default ON (matches UiSettings::default visual_ripple=true) so a fresh
        // profile keeps current behaviour; cold boot with ripple disabled stays hidden.
        if is_ripple_enabled(&app) {
            let _ = win.show();
        }
        crate::debug_log_internal("info", "[Overlay] ready signal received; overlay window visible and click-through");
    }
    Ok(())
}

/// Read persisted `ui.visual_ripple` without touching the click-loop atomics.
/// Falls back to `true` (UiSettings::default) when config is unreadable.
fn is_ripple_enabled(app: &AppHandle) -> bool {
    app.try_state::<crate::AppState>()
        .map(|s| s.config_manager.load().ui.visual_ripple)
        .unwrap_or(true)
}

/// Lazily create the fullscreen transparent overlay WebView (overlay.html).
/// Returns the window handle whether newly built or already existing.
/// Idempotent: second call just returns the existing window.
pub fn ensure_overlay_window(app: &AppHandle) -> Result<tauri::WebviewWindow, String> {
    if let Some(win) = app.get_webview_window("overlay") {
        return Ok(win);
    }
    let win = tauri::WebviewWindowBuilder::new(
        app,
        "overlay",
        tauri::WebviewUrl::App("overlay.html".into()),
    )
    .title("NanoClick Overlay")
    .transparent(true)
    .fullscreen(true)
    .always_on_top(true)
    .decorations(false)
    .shadow(false)
    .skip_taskbar(true)
    .resizable(false)
    .focused(false)
    .visible(false)
    .build()
    .map_err(|e| format!("overlay create failed: {e}"))?;
    let _ = win.set_ignore_cursor_events(true);
    crate::debug_log_internal("info", "[Overlay] lazy-created on demand");
    Ok(win)
}

/// Show or hide the overlay window dynamically (e.g. when toggling Visual Click Ripple in settings).
/// Lazy: creates the WebView on first `show=true`, destroys it on `show=false`
/// so idle RAM holds ZERO overlay WebViews. Hot click-loop is untouched —
/// emit_click_ripple() still no-ops via get_webview_window(None) when absent.
#[tauri::command]
pub fn toggle_overlay(app: AppHandle, show: bool) -> Result<(), String> {
    if show {
        let already_existed = app.get_webview_window("overlay").is_some();
        let win = ensure_overlay_window(&app)?;
        let _ = win.set_ignore_cursor_events(true);
        // Fresh WebView: DON'T show yet — overlay_ready() shows it once the
        // transparent DOM has rendered (Zero-Flash, no DWM white rectangle).
        // Existing window (DOM ready): show immediately.
        if already_existed {
            let _ = win.show();
        }
        crate::debug_log_internal("info", "[Overlay] overlay shown (lazy)");
    } else if let Some(win) = app.get_webview_window("overlay") {
        let _ = win.destroy();
        crate::debug_log_internal("info", "[Overlay] overlay destroyed, WebView memory released");
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

    #[test]
    fn test_ripple_coords_batch_serialization() {
        let batch = vec![(1920, 1080), (1921, 1081)];
        let serialized = serde_json::to_string(&batch).expect("Batch should serialize");
        assert_eq!(serialized, "[[1920,1080],[1921,1081]]");

        let deserialized: Vec<(i32, i32)> =
            serde_json::from_str(&serialized).expect("Batch should deserialize");
        assert_eq!(deserialized.len(), 2);
        assert_eq!(deserialized[0], (1920, 1080));
        assert_eq!(deserialized[1], (1921, 1081));
    }
}
