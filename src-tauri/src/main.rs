#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// RAM policy for the WebView2 layer (v1.1.0).
///
/// Why an env var instead of `additionalBrowserArgs` in `tauri.conf.json`:
/// the conf entry only covers the window declared there, while NanoClick
/// builds the HUD and the ripple overlay at runtime via
/// `WebviewWindowBuilder`. WebView2 reads
/// `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` for EVERY environment it creates,
/// so all three webviews get the same policy.
///
/// MEASURED RESULT (2026-09-21, `scripts/measure-ram.ps1`, whole process tree):
/// BEFORE 346.45 MB / 7 processes  →  AFTER 344.65 MB / 7 processes.
/// The footprint is dominated by Chromium's core helper processes (browser,
/// GPU, network, storage, crashpad), which these safe switches cannot remove.
///
/// Both of the "big" levers were tried and REMOVED on purpose:
/// * `--renderer-process-limit=1` — measured zero difference (a single-page
///   app already runs one renderer) while it would couple the HUD and the
///   overlay into the main window's renderer.
/// * `--js-flags=--max-old-space-size=…` — the value contains a space, and
///   WebView2 splits this env var on spaces, so it was never applied
///   correctly; capping the V8 heap also risks an OOM *inside* the renderer
///   during init, which is precisely the "interface does nothing" class of
///   bug. Removed rather than risked for no gain.
///
/// What remains is genuinely safe and slightly beneficial — it removes
/// subsystems we never use and stops background services we never need:
/// * `--disable-features=…` — no WebOOUI / PdfOOUI / SmartScreen clients.
/// * `--disable-background-networking` — no background network services.
/// * `--disk-cache-size=…` — bounds the `EBWebView` cache on disk.
///
/// FORBIDDEN here on purpose (see `docs/ZERO_JITTER_ISOLATION_PLAN.md` §2):
/// `--single-process` and `--in-process-gpu` collapse the GPU/renderer into
/// the host process, so a driver reset or a fullscreen DirectX switch would
/// freeze the Win32 thread that also runs the clicker. `--no-sandbox` is
/// likewise never acceptable. A regression test locks this in.
///
/// An explicit user-provided value wins: power users keep full control.
fn apply_webview_memory_policy() {
    const POLICY: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
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
