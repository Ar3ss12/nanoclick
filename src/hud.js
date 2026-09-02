// NanoClick floating HUD — listens for click-count updates from the engine.
// Guard: window.__TAURI__ may not be injected yet at script parse-time
// (race between WebView2 bridge init and script execution). We defer all
// Tauri API access until DOMContentLoaded to guarantee the bridge is ready.
document.addEventListener("DOMContentLoaded", () => {
  const listen = window.__TAURI__?.event?.listen;
  if (typeof listen !== "function") {
    // Tauri bridge unavailable (e.g. opened in a plain browser for debugging).
    console.warn("[HUD] Tauri event API not available — HUD will not update.");
    return;
  }

  const el = document.getElementById("hud");

  listen("hud-clicks", (event) => {
    const n = Number(event.payload) || 0;
    if (el) el.textContent = n > 999 ? n.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",") : String(n);
  });

  listen("hud-hide", () => {
    // Parent closes the window itself; this is just a safety net.
    const hudEl = document.getElementById("hud");
    if (hudEl) hudEl.style.opacity = "0";
  });
});
