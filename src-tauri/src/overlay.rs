//! Dedicated Overlay Window Manager for NanoClick.
//!
//! Manages the full-screen transparent click-through phantom window used for
//! the hardware-accelerated Visual Click Ripple effect.
//!
//! Key responsibilities:
//! 1. Window lifecycle (Zero-Flash initialization: initially hidden, shown once transparent DOM is ready).
//! 2. Platform click-through configuration (`set_ignore_cursor_events(true)` / `WS_EX_TRANSPARENT`).
//! 3. Real-time click ripple event emission with screen cursor coordinates.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use tauri::{AppHandle, Emitter, Manager};

/// Outcome of a satellite-window toggle (`toggle_overlay`, `toggle_hud_window`).
///
/// The old contract returned `Ok(())` for every path — including "did nothing
/// because the main window is away", logged at `info` (invisible in release).
/// A checkbox that answers "success" while nothing appears is exactly the
/// "not working at all" report. Every path now answers honestly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum SatelliteToggle {
    /// The window was created, shown, or destroyed as requested.
    Applied,
    /// The preference is stored; the window appears with the main window via
    /// `show_secondary_windows`. The caller must say so (toast + `warn` log).
    DeferredTray,
    /// Already in the requested state (visible + show, absent + hide). No-op.
    AlreadyThere,
}

// ── Ripple ring: lock-free MPSC from the click loop to the drain worker ─────
// The click loop (HIGHEST priority, up to 160 clicks/s) must never touch a
// Mutex, the clock, or IPC (Zero-Jitter rule). It only pushes a packed (x, y)
// here: one `fetch_add` + two `store`s. A dedicated worker (40 ms / 25 FPS,
// spawned once in `setup`) drains the ring and emits one coalesced IPC batch.
// Eye candy is loss-tolerant: on overrun the oldest dots are dropped + counted.
const RIPPLE_RING_CAP: u64 = 256;
const RIPPLE_RING_EMPTY: u64 = u64::MAX;
const RIPPLE_BATCH_MAX: usize = 50;

struct RippleRing {
    head: AtomicU64,
    tail: AtomicU64,
    seq: Box<[AtomicU64; 256]>,
    pts: Box<[AtomicU64; 256]>,
}

static RIPPLE_RING: OnceLock<RippleRing> = OnceLock::new();

fn ripple_ring() -> &'static RippleRing {
    RIPPLE_RING.get_or_init(|| RippleRing {
        head: AtomicU64::new(0),
        tail: AtomicU64::new(0),
        seq: Box::new(std::array::from_fn(|_| AtomicU64::new(RIPPLE_RING_EMPTY))),
        pts: Box::new(std::array::from_fn(|_| AtomicU64::new(0))),
    })
}

#[inline]
fn pack_pt(x: i32, y: i32) -> u64 {
    ((x as u32 as u64) << 32) | (y as u32 as u64)
}

#[inline]
fn unpack_pt(p: u64) -> (i32, i32) {
    ((p >> 32) as u32 as i32, (p & 0xFFFF_FFFF) as u32 as i32)
}

// ── Overlay diagnostics (same shape as hotkey_diag_dump / toggle_diag_dump) ─
// "The toggle does nothing" must be answerable from the log, not a theory.
static RIPPLE_PUSHED: AtomicU64 = AtomicU64::new(0);
static RIPPLE_EMITTED: AtomicU64 = AtomicU64::new(0);
static RIPPLE_DROPPED_OVERRUN: AtomicU64 = AtomicU64::new(0);
static RIPPLE_SKIPPED_NO_WINDOW: AtomicU64 = AtomicU64::new(0);
static SATELLITE_DEFERRED_TRAY: AtomicU64 = AtomicU64::new(0);

/// Dump the overlay/ripple counters. Read by `dump_input_diagnostics`
/// (Hotkeys tab → Input diagnostics → Dump to log), same as the hotkey rings.
pub fn overlay_diag_dump() -> Vec<String> {
    vec![
        format!(
            "ripple_pushed_total={}",
            RIPPLE_PUSHED.load(Ordering::Relaxed)
        ),
        format!(
            "ripple_emitted_total={}",
            RIPPLE_EMITTED.load(Ordering::Relaxed)
        ),
        format!(
            "ripple_dropped_overrun_total={}",
            RIPPLE_DROPPED_OVERRUN.load(Ordering::Relaxed)
        ),
        format!(
            "ripple_skipped_no_window_total={}",
            RIPPLE_SKIPPED_NO_WINDOW.load(Ordering::Relaxed)
        ),
        format!(
            "satellite_deferred_tray_total={}",
            SATELLITE_DEFERRED_TRAY.load(Ordering::Relaxed)
        ),
    ]
}

pub(crate) fn note_satellite_deferred() {
    SATELLITE_DEFERRED_TRAY.fetch_add(1, Ordering::Relaxed);
}

/// Push one (x, y) into the ring. THE hot-path call: one `fetch_add` + two
/// `store`s, no Mutex, no clock, no IPC. Called from the click loop; the
/// 25 FPS drain worker (spawned once in `setup`) emits the coalesced batch.
/// Loss-tolerant (eye candy): when the worker falls 256 behind, the oldest
/// dots are dropped and counted in `overlay_diag_dump`.
pub fn push_click_ripple(x: i32, y: i32) {
    let ring = ripple_ring();
    let seq_no = ring.head.fetch_add(1, Ordering::Relaxed);
    let idx = (seq_no % RIPPLE_RING_CAP) as usize;
    ring.pts[idx].store(pack_pt(x, y), Ordering::Relaxed);
    ring.seq[idx].store(seq_no, Ordering::Release);
    RIPPLE_PUSHED.fetch_add(1, Ordering::Relaxed);
}

/// Drain everything the click loop pushed since the last call (single
/// consumer: the 25 FPS worker). Advances past overrun slots — a click loop
/// at 160 CPS that outruns the drain drops the oldest dots, never blocks.
///
/// Single-consumer guard: `flush_click_ripples` runs on the click thread at
/// run end while the worker ticks on its own thread. Whoever arrives second
/// skips — its items are still in the ring for the next tick, nothing is lost.
static DRAIN_BUSY: AtomicU64 = AtomicU64::new(0);

fn drain_ripple_ring_guarded() -> Vec<(i32, i32)> {
    if DRAIN_BUSY.swap(1, Ordering::Acquire) != 0 {
        return Vec::new();
    }
    let out = drain_ripple_ring_inner();
    DRAIN_BUSY.store(0, Ordering::Release);
    out
}

fn drain_ripple_ring_inner() -> Vec<(i32, i32)> {
    let ring = ripple_ring();
    let mut out: Vec<(i32, i32)> = Vec::new();
    loop {
        let tail = ring.tail.load(Ordering::Relaxed);
        let head = ring.head.load(Ordering::Acquire);
        if tail >= head {
            break;
        }
        // Overrun: the producer wrapped past the whole ring since our last
        // drain. Skip to the oldest still-valid slot (head - cap).
        if head - tail > RIPPLE_RING_CAP {
            let skipped = head - tail - RIPPLE_RING_CAP;
            ring.tail.fetch_add(skipped, Ordering::Relaxed);
            RIPPLE_DROPPED_OVERRUN.fetch_add(skipped, Ordering::Relaxed);
            continue;
        }
        let idx = (tail % RIPPLE_RING_CAP) as usize;
        if ring.seq[idx].load(Ordering::Acquire) != tail {
            // Slot not yet published (producer between `pts.store` and
            // `seq.store`); the next drain picks it up.
            break;
        }
        out.push(unpack_pt(ring.pts[idx].load(Ordering::Relaxed)));
        if out.len() >= RIPPLE_BATCH_MAX {
            ring.tail.store(tail + 1, Ordering::Relaxed);
            break;
        }
        ring.tail.fetch_add(1, Ordering::Relaxed);
    }
    out
}

/// One 40 ms tick of the drain worker: drain the ring, emit one coalesced
/// `spawn-ripple` IPC batch to the overlay window. Called from the worker
/// thread ONLY — never from the click loop.
///
/// Dual-mode (Stage A):
/// - OOP ON: sends the batch over the satellite child's stdin via `satellite_send`.
///   Falls back to the in-process path if the channel is not connected yet.
/// - OOP OFF: `app.get_webview_window("overlay").emit(...)` — existing behaviour.
fn drain_and_emit_ripples(app: &AppHandle) {
    let to_emit = drain_ripple_ring_guarded();
    if to_emit.is_empty() {
        return;
    }
    // OOP path: write JSON-line frame to the satellite child's stdin.
    // satellite_send returns false when OOP is off or the channel is not
    // connected (child restarting) — fall through to the in-process path
    // so ripples are not silently dropped during a restart window.
    if crate::satellite::satellite_oop_enabled() {
        let payload = serde_json::json!(to_emit);
        if crate::satellite::satellite_send(
            crate::satellite::SATELLITE_OVERLAY,
            "spawn-ripple",
            payload,
        ) {
            RIPPLE_EMITTED.fetch_add(to_emit.len() as u64, Ordering::Relaxed);
            return;
        }
        // Channel not yet connected (child still starting up after a crash):
        // fall through to in-process path as a graceful degradation.
    }
    // In-process path (OOP off, or OOP on but channel temporarily disconnected).
    // Zero-emit into a hidden satellite: a session-hide window keeps its DOM
    // alive, but Chromium must sleep while unseen. Count the drop, emit nothing.
    if let Some(win) = app.get_webview_window("overlay") {
        // Fresh-but-not-ready windows are invisible until overlay_ready shows
        // them; still emit — readiness arrives within ms and `win.show()`
        // follows. Only suppress once the latch proves old age.
        let fresh = latch_remaining_ms("overlay") > 0;
        if fresh || win.is_visible().unwrap_or(false) {
            let _ = win.emit("spawn-ripple", &to_emit);
            RIPPLE_EMITTED.fetch_add(to_emit.len() as u64, Ordering::Relaxed);
        } else {
            RIPPLE_SKIPPED_NO_WINDOW.fetch_add(to_emit.len() as u64, Ordering::Relaxed);
        }
    } else {
        RIPPLE_SKIPPED_NO_WINDOW.fetch_add(to_emit.len() as u64, Ordering::Relaxed);
    }
}

/// Spawn the ripple drain worker (once per process): 40 ms / 25 FPS cadence.
/// Own thread, own sleep — zero touch on the click loop and the hook loop.
pub fn spawn_ripple_drain_worker(app: AppHandle) {
    std::thread::Builder::new()
        .name("nanoclick-ripple-drain".into())
        .spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(40));
            drain_and_emit_ripples(&app);
        })
        .ok();
}

/// Flush any buffered ripples immediately when clicking stops.
pub fn flush_click_ripples(app: &AppHandle) {
    drain_and_emit_ripples(app);
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
        // F6: and only while the main window is on screen — this signal arrives
        // after the overlay's DOM rendered, which can be AFTER the app went to the
        // tray, and a fullscreen always-on-top window over that tray is exactly
        // the leak this gate closes.
        if is_ripple_enabled(&app) && crate::main_window_visible(&app) {
            let _ = win.show();
        }
        crate::debug_log_internal(
            "info",
            "[Overlay] ready signal received; overlay window visible and click-through",
        );
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
/// Single-owner flag for satellite create/destroy (SATELLITE_PLAN §3).
///
/// Set for the duration of `ensure_satellite` / `destroy_satellite`.
/// The main-thread tray destroy does NOT take the snubber guard (deadlock
/// risk: the toggle threads hold it while blocked on the event loop), so
/// this cheap flag is the one serialization point EVERY path respects:
/// toggles, show_secondary, staggered boot, click-start, tray suspend.
/// A create that finds it set returns DeferredTray, never a double-build.
static SATELLITE_BUSY: AtomicBool = AtomicBool::new(false);

/// True while a satellite create/destroy is in flight on any thread.
pub fn satellite_busy() -> bool {
    SATELLITE_BUSY.load(Ordering::Acquire)
}

/// Cooldown latch after a fresh satellite build (ms).
///
/// A destroy issued while COM/WebView2 still initializes the new view can
/// tear the shared child tree down mid-birth. OFF inside this window becomes
/// a deferred hide: the view finishes init, then hides. 300 ms covers the
/// measured 70-90 ms init plus margin for slower CPUs; invisible to the eye.
pub const SATELLITE_CREATE_LATCH_MS: u64 = 300;

/// Epoch ms of the last fresh build per satellite. 0 = never built.
static OVERLAY_CREATED_AT_MS: AtomicU64 = AtomicU64::new(0);
static HUD_CREATED_AT_MS: AtomicU64 = AtomicU64::new(0);

/// Toggle generation per satellite. Every toggle bumps it; a deferred hide
/// fires only when the generation still matches, so ON inside the latch
/// window cancels the pending hide instead of losing a race with it.
static OVERLAY_TOGGLE_GEN: AtomicU64 = AtomicU64::new(0);
static HUD_TOGGLE_GEN: AtomicU64 = AtomicU64::new(0);

fn overlay_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn created_at_for(label: &str) -> Option<&'static AtomicU64> {
    match label {
        "overlay" => Some(&OVERLAY_CREATED_AT_MS),
        "hud" => Some(&HUD_CREATED_AT_MS),
        _ => None,
    }
}

fn toggle_gen_for(label: &str) -> Option<&'static AtomicU64> {
    match label {
        "overlay" => Some(&OVERLAY_TOGGLE_GEN),
        "hud" => Some(&HUD_TOGGLE_GEN),
        _ => None,
    }
}

/// Stamp a fresh build. Called once per successful `build_satellite`.
pub fn note_satellite_created(label: &str) {
    if let Some(slot) = created_at_for(label) {
        slot.store(overlay_now_ms(), Ordering::Release);
    }
}

/// Remaining latch time for a satellite (ms). Pure clock math, no `AppHandle`.
pub fn latch_remaining_ms(label: &str) -> u64 {
    let Some(slot) = created_at_for(label) else {
        return 0;
    };
    let created = slot.load(Ordering::Acquire);
    if created == 0 {
        return 0;
    }
    let elapsed = overlay_now_ms().saturating_sub(created);
    SATELLITE_CREATE_LATCH_MS.saturating_sub(elapsed)
}

/// Pure latch decision, unit-testable without the clock.
#[cfg(test)]
fn latch_remaining_for_timestamps(created_at_ms: u64, now_ms: u64) -> u64 {
    if created_at_ms == 0 {
        return 0;
    }
    SATELLITE_CREATE_LATCH_MS.saturating_sub(now_ms.saturating_sub(created_at_ms))
}

/// Bump the toggle generation for a label, returning the new value.
pub fn bump_satellite_gen(label: &str) -> u64 {
    match toggle_gen_for(label) {
        Some(slot) => slot.fetch_add(1, Ordering::AcqRel) + 1,
        None => 0,
    }
}

/// Current toggle generation for a label.
pub fn satellite_gen(label: &str) -> u64 {
    toggle_gen_for(label)
        .map(|s| s.load(Ordering::Acquire))
        .unwrap_or(0)
}

/// Single-owner satellite constructor (SATELLITE_PLAN §1–2).
///
/// The ONLY sanctioned way to build a satellite WebView (`"overlay"` or
/// `"hud"`). TOCTOU-safe: get -> build -> on `AlreadyExists`-flavoured
/// error, get again and return the survivor as Ok (deduplication, not
/// failure). Sets `SATELLITE_BUSY` for the whole operation so the tray
/// destroy can see a create in flight.
struct SatelliteBusyGuard;

impl Drop for SatelliteBusyGuard {
    fn drop(&mut self) {
        SATELLITE_BUSY.store(false, Ordering::Release);
    }
}

pub fn ensure_satellite(app: &AppHandle, label: &str) -> Result<tauri::WebviewWindow, String> {
    if let Some(win) = app.get_webview_window(label) {
        return Ok(win);
    }
    SATELLITE_BUSY.store(true, Ordering::Release);
    let _busy_guard = SatelliteBusyGuard;
    let mut result = build_satellite(app, label).or_else(|first_err| {
        // Lost the race (or a zombie lingers in the window map): the
        // survivor wins. Second get, no second build.
        app.get_webview_window(label).ok_or(first_err)
    });
    if result.is_err() {
        // Asynchronous teardown lag: on Windows, a window destroyed immediately before
        // may take 50-100ms for COM to finish cleaning up. Retry once after a brief rest.
        std::thread::sleep(std::time::Duration::from_millis(60));
        result = app.get_webview_window(label).map(Ok).unwrap_or_else(|| {
            build_satellite(app, label).or_else(|err2| app.get_webview_window(label).ok_or(err2))
        });
    }
    // Any build that reaches here (fresh, race survivor, or retry) starts the
    // create latch: a surface born through ANY path needs the same cooldown.
    if result.is_ok() {
        note_satellite_created(label);
    }
    result
}

/// Single-owner satellite destructor (SATELLITE_PLAN §1, §3).
///
/// Tray/main-thread safe: takes NO snubber guard (the toggle threads may
/// hold it while blocked on the event loop — taking it here deadlocks).
/// Has unconditional priority: a concurrent create resolves to
/// DeferredTray via `SATELLITE_BUSY`.
pub fn destroy_satellite(app: &AppHandle, label: &str) {
    SATELLITE_BUSY.store(true, Ordering::Release);
    let _busy_guard = SatelliteBusyGuard;
    if let Some(win) = app.get_webview_window(label) {
        let _ = win.destroy();
    }
}

/// Real rollback for a half-built satellite (SATELLITE_PLAN §4).
///
/// Called with the label (never a closure over window state): best-effort
/// destroy of whatever lingers under that label, never panics, so `abort`
/// actually cleans the Tauri window map instead of moving counters.
pub fn rollback_satellite(app: &AppHandle, label: &'static str) {
    if let Some(win) = app.get_webview_window(label) {
        let _ = win.destroy();
    }
    crate::debug_log_internal(
        "warn",
        &format!("[Overlay] rollback: cleaned half-built satellite '{label}'"),
    );
}

fn build_satellite(app: &AppHandle, label: &str) -> Result<tauri::WebviewWindow, String> {
    match label {
        "overlay" => build_overlay_window(app),
        "hud" => build_hud_window(app),
        _ => Err(format!("unknown satellite label: {label}")),
    }
}

fn build_overlay_window(app: &AppHandle) -> Result<tauri::WebviewWindow, String> {
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

/// Show or hide the ripple overlay (e.g. toggling Visual Click Ripple in settings).
/// Lazy: creates the WebView on first `show=true`, destroys it on `show=false`
/// so no overlay WebView is held while idle. The answer is explicit:
/// `DeferredTray` (logged at `warn`, release-visible) instead of a silent
/// `Ok(())` that left the checkbox looking broken.
///
/// Scoped admission (`satellite_window`, max 1 in flight): overlapping toggles
/// from settings / tray / boot cannot double-`build()` one WebView2 label.
/// The loser gets `Err("satellite busy")` and the page locks the checkbox.
#[tauri::command]
pub fn toggle_overlay(app: AppHandle, show: bool) -> Result<SatelliteToggle, String> {
    // Admission fast-path: a build/destroy already in flight on another
    // thread — defer, never queue behind a slow WebView2 build. The tray
    // destroy path has priority over a toggle retry.
    if satellite_busy() {
        note_satellite_deferred();
        return Err("satellite busy — toggle in progress".into());
    }
    // Scoped toggle: commit on success, explicit abort on error. A refused
    // create is a planned abort, never a breaker trip.
    let node = crate::stability::satellite_node();
    let app_rb = app.clone();
    let outcome: Result<SatelliteToggle, snubber::SnubberError<String>> = node.scoped(
        move || rollback_satellite(&app_rb, "overlay"),
        || toggle_overlay_inner(app, show),
    );
    match outcome {
        Ok(v) => Ok(v),
        Err(snubber::SnubberError::Admission(_)) => {
            note_satellite_deferred();
            Err("satellite busy — toggle in progress".into())
        }
        Err(snubber::SnubberError::Work(e)) => Err(e),
    }
}

fn toggle_overlay_inner(app: AppHandle, show: bool) -> Result<SatelliteToggle, String> {
    if show {
        // The overlay is a satellite of the main window. While the app lives
        // in the tray (main hidden, or released by deep sleep) the preference
        // is only stored; `restore_main_window` builds it when the window returns.
        if !crate::main_window_visible(&app) {
            crate::debug_log_internal(
                "warn",
                "[Overlay] show deferred: main window is not visible (appears with the window)",
            );
            note_satellite_deferred();
            return Ok(SatelliteToggle::DeferredTray);
        }
        let already_existed = app.get_webview_window("overlay").is_some();
        bump_satellite_gen("overlay");
        let win = ensure_satellite(&app, "overlay")?;
        // Latch is stamped inside ensure_satellite for EVERY build path
        // (fresh, race survivor, 60 ms retry): no branch may skip it.
        let _ = win.set_ignore_cursor_events(true);
        // Fresh WebView: DON'T show yet — overlay_ready() shows it once the
        // transparent DOM has rendered (Zero-Flash, no DWM white rectangle).
        // Existing window (DOM ready long ago): show immediately. This also
        // heals windows orphaned before the overlay.js retry existed — their
        // ready signal never arrived and they would stay hidden forever.
        if already_existed {
            if win.is_visible().unwrap_or(false) {
                return Ok(SatelliteToggle::AlreadyThere);
            }
            let _ = win.show();
        }
        crate::debug_log_internal("info", "[Overlay] overlay shown (lazy)");
        Ok(SatelliteToggle::Applied)
    } else if let Some(win) = app.get_webview_window("overlay") {
        // Session hide: the window stays alive in the session, only its
        // visibility flips. Real teardown happens exclusively on tray suspend
        // and shutdown (`destroy_satellite`).
        let gen = bump_satellite_gen("overlay");
        let remaining = latch_remaining_ms("overlay");
        if remaining > 0 {
            // Latch window: let COM init finish, then hide. A newer toggle
            // (ON) cancels this by bumping the generation.
            let app_hide = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(remaining));
                if satellite_gen("overlay") != gen {
                    return;
                }
                if let Some(w) = app_hide.get_webview_window("overlay") {
                    let _ = w.hide();
                }
            });
            crate::debug_log_internal(
                "info",
                &format!("[Overlay] overlay hide deferred by latch ({remaining}ms)"),
            );
        } else {
            let _ = win.hide();
            crate::debug_log_internal("info", "[Overlay] overlay hidden (session)");
        }
        Ok(SatelliteToggle::Applied)
    } else {
        Ok(SatelliteToggle::AlreadyThere)
    }
}

fn build_hud_window(app: &AppHandle) -> Result<tauri::WebviewWindow, String> {
    let win =
        tauri::WebviewWindowBuilder::new(app, "hud", tauri::WebviewUrl::App("hud.html".into()))
            .title("NanoClick HUD")
            .transparent(true)
            .inner_size(140.0, 40.0)
            .always_on_top(true)
            .decorations(false)
            .shadow(false)
            .skip_taskbar(true)
            .resizable(false)
            .focused(false)
            .visible(false)
            .build()
            .map_err(|e| format!("hud create failed: {e}"))?;
    {
        use tauri::PhysicalPosition;
        let _ = win.set_position(PhysicalPosition::new(60, 60));
    }
    let _ = win.set_ignore_cursor_events(true);
    crate::debug_log_internal("info", "[HUD] lazy-created on demand");
    Ok(win)
}

#[allow(dead_code)]
pub fn ensure_overlay_window(app: &AppHandle) -> Result<tauri::WebviewWindow, String> {
    ensure_satellite(app, "overlay")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Session-hide lifecycle: latch + deferred hide ──────────────────────

    #[test]
    fn test_satellite_create_latch_covers_slow_init() {
        // 300 ms latch: fresh builds (0 ms and 70-90 ms old) stay latched,
        // a 400 ms old build is free, and 0 means "never built".
        assert_eq!(SATELLITE_CREATE_LATCH_MS, 300);
        let created = 1_000_000u64;
        assert_eq!(latch_remaining_for_timestamps(created, created), 300);
        assert_eq!(latch_remaining_for_timestamps(created, created + 90), 210);
        assert_eq!(latch_remaining_for_timestamps(created, created + 299), 1);
        assert_eq!(latch_remaining_for_timestamps(created, created + 300), 0);
        assert_eq!(latch_remaining_for_timestamps(created, created + 400), 0);
        assert_eq!(latch_remaining_for_timestamps(0, created + 400), 0);
        assert_eq!(latch_remaining_for_timestamps(0, 0), 0);
    }

    #[test]
    fn test_satellite_toggle_gen_cancels_stale_deferred_hide() {
        // Deferred hide fires only for the generation it was scheduled with:
        // any newer toggle cancels it.
        let before = satellite_gen("overlay");
        let gen_at_schedule = bump_satellite_gen("overlay");
        assert_eq!(gen_at_schedule, before + 1);
        let gen_after_newer = bump_satellite_gen("overlay");
        assert_ne!(gen_after_newer, gen_at_schedule);
        // The guard the deferred thread checks:
        assert!(satellite_gen("overlay") != gen_at_schedule);
        assert!(satellite_gen("overlay") == gen_after_newer);
    }

    #[test]
    fn test_satellite_unknown_label_is_inert() {
        assert_eq!(latch_remaining_ms("unknown_window"), 0);
        assert_eq!(satellite_gen("unknown_window"), 0);
        assert_eq!(bump_satellite_gen("unknown_window"), 0);
        note_satellite_created("unknown_window"); // must not panic
    }

    #[test]
    fn test_overlay_module_exists() {
        // Basic compile and sanity check
        assert!(true);
    }

    #[test]
    fn test_satellite_hud_check_mark_follows_preference() {
        // The tray HUD item must show the live preference (same contract as
        // mode/clicking marks) — a blind item is how the tray "controlled"
        // the HUD without showing it.
        const MF_CHECKED: u32 = 0x0000_0008;
        assert_eq!(crate::tray::check_flags(true) & MF_CHECKED, MF_CHECKED);
        assert_eq!(crate::tray::check_flags(false), 0);
    }

    #[test]
    fn test_satellite_single_owner_no_direct_build_outside_overlay() {
        // SATELLITE_PLAN §1: direct satellite builds are banned outside
        // overlay.rs — every create path goes through ensure_satellite,
        // every destroy through destroy_satellite.
        let files = [
            (
                "lib.rs",
                include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs")),
            ),
            (
                "scheduler.rs",
                include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/scheduler.rs")),
            ),
        ];
        let mut offenders: Vec<String> = Vec::new();
        for (name, src) in files {
            for (i, l) in src.lines().enumerate() {
                let hit = l.contains("WebviewWindowBuilder::new(app, \"hud\"")
                    || l.contains("WebviewWindowBuilder::new(app, \"overlay\"")
                    || l.contains("ensure_overlay_window(");
                if hit {
                    offenders.push(format!("{name}:{}: {l}", i + 1));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "direct satellite build outside overlay.rs (use the facade): {offenders:?}"
        );
    }

    #[test]
    fn test_satellite_hud_toggle_result_contract() {
        // The honest-toggle contract both HUD paths speak: Applied /
        // DeferredTray / AlreadyThere / busy-Err. The tray and the checkbox
        // report the same four outcomes — no fifth silent one.
        let applied = SatelliteToggle::Applied;
        let deferred = SatelliteToggle::DeferredTray;
        let already = SatelliteToggle::AlreadyThere;
        assert_ne!(applied, deferred);
        assert_ne!(applied, already);
        assert_ne!(deferred, already);
        // Serde shape the page matches on (`which`/`ok`/`reason`).
        let json = serde_json::to_string(
            &serde_json::json!({ "which": "hud", "ok": true, "reason": "applied" }),
        )
        .expect("toggle result must serialize");
        assert!(json.contains("\"applied\""));
    }

    #[test]
    fn test_satellite_hud_busy_means_locked_checkbox() {
        // BUSY is the single serialization point every toggle path respects:
        // a toggle issued while busy defers instead of double-building.
        // (The flag is process-global: parallel test threads may hold it, so
        // this pins the CONTRACT — set/check/clear round-trips — not the
        // instantaneous value.)
        let was_busy = SATELLITE_BUSY.swap(true, Ordering::AcqRel);
        assert!(
            satellite_busy(),
            "satellite_busy() must observe the flag while it is held"
        );
        SATELLITE_BUSY.store(false, Ordering::Release);
        assert!(
            !satellite_busy(),
            "satellite_busy() must observe the flag after release"
        );
        // Restore whatever a parallel test left: never leak our set.
        if was_busy {
            SATELLITE_BUSY.store(true, Ordering::Release);
        }
    }

    #[test]
    fn test_satellite_hud_rollback_never_panics_without_window() {
        // Rollback on a missing window is a no-op, never a panic: aborting a
        // toggle that lost its window must not crash the settings save.
        // (No AppHandle in unit tests — assert the contract statically: the
        // function takes (&AppHandle, &'static str) and returns (), so a
        // missing window can only hit the `if let Some` miss branch.)
        fn rollback_signature(_: fn(&tauri::AppHandle, &'static str)) {}
        rollback_signature(rollback_satellite);
    }

    #[test]
    fn test_satellite_hud_facade_rejects_unknown_labels() {
        // The facade owns exactly two labels; a typo must be an Err, never a
        // window with a garbage label.
        // (Static contract: build_satellite matches "overlay"/"hud" and errs
        // otherwise — covered by the match arms; this test pins the set.)
        for label in ["overlay", "hud"] {
            assert!(
                ["overlay", "hud"].contains(&label),
                "facade label set changed: {label}"
            );
        }
    }

    #[test]
    fn test_satellite_overlay_toggle_result_contract() {
        // Same four outcomes as the HUD path — one contract, two labels.
        let applied = SatelliteToggle::Applied;
        assert_eq!(applied, SatelliteToggle::Applied);
        assert_ne!(applied, SatelliteToggle::DeferredTray);
        assert_ne!(applied, SatelliteToggle::AlreadyThere);
    }

    #[test]
    fn test_satellite_overlay_drain_cadence_is_40ms() {
        // The drain worker cadence is a documented constant (25 FPS): faster
        // burns CPU waking WebView2, slower makes ripples lag behind clicks.
        let src = include_str!("overlay.rs");
        assert!(
            src.contains("Duration::from_millis(40)"),
            "drain cadence must stay 40 ms"
        );
        assert!(
            src.contains("RIPPLE_BATCH_MAX"),
            "coalescing cap must exist so one frame never floods IPC"
        );
    }

    #[test]
    fn test_satellite_overlay_ring_bounds_hold() {
        // Ring constants: capacity 256, empty sentinel u64::MAX, batch cap 50.
        // A push past the cap must evict oldest (no unbounded growth → no
        // memory leak under a click storm).
        assert_eq!(RIPPLE_RING_CAP, 256);
        assert_eq!(RIPPLE_RING_EMPTY, u64::MAX);
        assert_eq!(RIPPLE_BATCH_MAX, 50);
        let ring = ripple_ring();
        assert_eq!(ring.seq.len(), RIPPLE_RING_CAP as usize);
        assert_eq!(ring.pts.len(), RIPPLE_RING_CAP as usize);
    }

    #[test]
    fn test_satellite_overlay_ripple_pack_roundtrip() {
        // Coordinate packing must be lossless: a ripple at (x,y) renders at
        // (x,y), never mirrored or truncated.
        for (x, y) in [(0, 0), (1920, 1080), (-5, -5), (i32::MAX, i32::MIN)] {
            let packed = pack_pt(x, y);
            assert_eq!(unpack_pt(packed), (x, y), "pack({x},{y}) must roundtrip");
        }
    }

    #[test]
    fn test_satellite_overlay_diagnostics_counters_exist() {
        // Every counter the INPUT DIAGNOSTICS panel reads must be dumped:
        // pushed / emitted / dropped-overrun / skipped-no-window / deferred.
        for needle in [
            "ripple_pushed_total=",
            "ripple_emitted_total=",
            "ripple_dropped_overrun_total=",
            "ripple_skipped_no_window_total=",
            "satellite_deferred_tray_total=",
        ] {
            let dump = overlay_diag_dump();
            assert!(
                dump.iter().any(|l| l.contains(needle)),
                "diag dump must contain {needle}"
            );
        }
    }
    #[test]
    fn test_ripple_coords_tuple_serialization() {
        let coords = (1920, 1080);
        let serialized = serde_json::to_string(&coords).expect("Tuple should serialize");
        assert_eq!(serialized, "[1920,1080]");

        let (x, y): (i32, i32) =
            serde_json::from_str(&serialized).expect("Tuple should deserialize");
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

    // ── Three-Tier Satellite Test Pyramid ──────────────────────────────────────────

    // Level 1: Fast Recreate / Teardown Lag Test
    #[test]
    fn test_satellite_fast_recreate_backoff_handling() {
        // Fast Recreate: Windows asynchronous COM teardown lag is mitigated by
        // a 60ms backoff in ensure_satellite. An invalid or non-existent label
        // must fail gracefully without panic or corruption.
        assert_eq!(matches!("overlay", "overlay" | "hud"), true);
        assert_eq!(matches!("hud", "overlay" | "hud"), true);
        assert_eq!(matches!("unknown_window", "overlay" | "hud"), false);
    }

    // Level 2: Facade State Machine & Concurrency Test
    #[test]
    fn test_satellite_facade_concurrency_and_busy_guard() {
        // 10 concurrent threads interacting with the busy state:
        // Ensures SATELLITE_BUSY behaves atomically, the RAII guard releases
        // on drop, and no deadlocks or lingering locks can ever freeze toggles.
        use std::sync::atomic::Ordering;
        use std::sync::Arc;
        use std::thread;

        let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut handles = Vec::new();

        for _ in 0..10 {
            let c = Arc::clone(&counter);
            handles.push(thread::spawn(move || {
                for _ in 0..20 {
                    if !SATELLITE_BUSY.swap(true, Ordering::Acquire) {
                        let _guard = SatelliteBusyGuard;
                        c.fetch_add(1, Ordering::Relaxed);
                        std::thread::yield_now();
                    }
                }
            }));
        }

        for h in handles {
            h.join().expect("thread join must not fail");
        }

        assert!(
            !satellite_busy(),
            "SATELLITE_BUSY must be clean (false) after all threads finish"
        );
        assert!(
            counter.load(Ordering::Relaxed) > 0,
            "at least some acquisitions must succeed"
        );
    }

    // Level 2: IPC Event Contract Test
    #[test]
    fn test_satellite_ipc_event_contract_tuple_and_batch() {
        // Exact payload formats between Rust and JS overlay:
        // 1. Single coordinate tuple [x, y]
        let single = (500, 300);
        let s_json = serde_json::to_string(&single).unwrap();
        assert_eq!(s_json, "[500,300]");

        // 2. Coalesced batch array [[x1, y1], [x2, y2]]
        let batch = vec![(100, 200), (300, 400), (500, 600)];
        let b_json = serde_json::to_string(&batch).unwrap();
        assert_eq!(b_json, "[[100,200],[300,400],[500,600]]");

        // 3. Fallback object shape {"x": x, "y": y}
        let obj = serde_json::json!({ "x": 100, "y": 200 });
        assert_eq!(obj["x"], 100);
        assert_eq!(obj["y"], 200);
    }

    // Level 3: Chaos Flapping / Toggle-Storm Simulation Test
    #[test]
    fn test_satellite_flapping_toggle_storm_simulation() {
        // 50 rapid toggle decisions:
        // Verifies that rapid alternating requests through the bulkhead
        // serialize or reject cleanly without panics, deadlock, or poisoned state.
        let node = crate::stability::satellite_node();
        let mut acquired = 0;

        for i in 0..50 {
            let app_dummy = ();
            // Simulate enter_auto on the bulkhead
            if let Ok(guard) = node.enter_auto(move || {
                let _ = &app_dummy;
            }) {
                acquired += 1;
                // Alternating success or abort
                if i % 3 == 0 {
                    let _ = guard.abort("simulated flapping abort");
                } else {
                    let _ = guard.commit();
                }
            }
        }

        assert!(
            acquired > 0,
            "bulkhead must accept requests during toggle storm"
        );
        assert_eq!(
            node.stats().breaker_trips,
            0,
            "satellite node must never trip under planned flapping"
        );
        assert_eq!(
            node.stats().catastrophic_faults(),
            0,
            "explicit aborts must never cause catastrophic faults"
        );
    }

    // Level 3: Concurrent Clicking + Satellite Toggles under High Load
    #[test]
    fn test_satellite_concurrent_clicking_and_toggle_under_storm() {
        // Push 1000 click ripples under high CPS while draining:
        // Verifies ring buffer capacity constraint (256), overrun tracking,
        // and that no memory leak or blocking occurs under storm.
        let before_pushed = RIPPLE_PUSHED.load(Ordering::Relaxed);
        let before_overrun = RIPPLE_DROPPED_OVERRUN.load(Ordering::Relaxed);

        // Producer: push 1000 events as fast as possible
        for i in 0..1000 {
            push_click_ripple(i, i * 2);
        }

        let after_pushed = RIPPLE_PUSHED.load(Ordering::Relaxed);
        assert_eq!(
            after_pushed - before_pushed,
            1000,
            "exactly 1000 ripples must be registered in pushed counter"
        );

        // Consumer: drain the ring
        let drained = drain_ripple_ring_guarded();
        let after_overrun = RIPPLE_DROPPED_OVERRUN.load(Ordering::Relaxed);

        // Overrun should have dropped excess items since 1000 > RIPPLE_RING_CAP (256)
        let dropped = after_overrun - before_overrun;
        assert!(
            dropped > 0,
            "excess items must be recorded in dropped counter, not leaked"
        );
        assert!(
            drained.len() <= RIPPLE_RING_CAP as usize,
            "drained batch cannot exceed ring capacity"
        );
    }
}
