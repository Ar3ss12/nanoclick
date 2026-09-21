#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// RAM-reduction for the WebView2 layer (v1.1.0).
///
/// Why an env var instead of `additionalBrowserArgs` in `tauri.conf.json`:
/// the conf entry only covers the window declared there, while NanoClick
/// builds the HUD and the ripple overlay at runtime via
/// `WebviewWindowBuilder`. WebView2 reads
/// `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` for EVERY environment it creates,
/// so all three webviews get the same policy.
///
/// These flags are chosen to stay inside the Zero-Jitter Mandate
/// (`docs/ZERO_JITTER_ISOLATION_PLAN.md` §2) — the native click loop and the
/// `WH_KEYBOARD_LL` hook live in Rust threads and are never affected:
/// * `--renderer-process-limit=1` — one renderer for main + HUD + overlay
///   instead of one per webview: the biggest single win, and it removes
///   duplicated V8/Blink heaps.
/// * `--disable-features=…` — drops UI subsystems we never use (WebOOUI,
///   PdfOOUI, SmartScreen) instead of letting them allocate at boot.
/// * `--js-flags=--max-old-space-size=128 --max-semi-space-size=2` — caps the
///   V8 heap. The UI is a form + one canvas; the old default lets the heap
///   grow into the hundreds of MB before collecting.
/// * `--disable-background-networking` — no background network services.
/// * `--disk-cache-size=…` — keeps the `EBWebView` cache on disk bounded.
///
/// FORBIDDEN here on purpose (see the mandate): `--single-process` and
/// `--in-process-gpu` collapse the GPU/renderer into the host process, so a
/// driver reset or a fullscreen DirectX switch would freeze the Win32 thread
/// that also runs the clicker. `--no-sandbox` is likewise never acceptable.
///
/// An explicit user-provided value wins: power users keep full control.
fn apply_webview_memory_policy() {
    const POLICY: &str = "--renderer-process-limit=1 \
--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
--js-flags=--max-old-space-size=128 --max-semi-space-size=2 \
--disable-background-networking \
--disk-cache-size=33554432";
    if std::env::var_os("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").is_none() {
        std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", POLICY);
    }
}

fn main() {
    apply_webview_memory_policy();
    nanoclick::run();
}
