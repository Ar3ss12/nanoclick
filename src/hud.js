// nanoclick floating HUD — displays session click count.
// Renders immediately on DOM ready (no opacity gate).
// Updates in real-time via targeted hud-clicks IPC events from scheduler.rs.
function initHud() {
  const el = document.getElementById("hud");

  function attachListener(attemptsLeft) {
    const tauri = window.__TAURI__;
    const listen = tauri?.event?.listen;
    const invoke = tauri?.core?.invoke;

    if (typeof listen === "function") {
      listen("hud-clicks", (event) => {
        if (!el) return;
        const n = Number(event.payload) || 0;
        el.textContent = n > 999
          ? n.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",")
          : String(n);
      });

      // Signal Rust that HUD DOM is rendered and ready.
      // Rust automatically restores HUD visibility if show_hud is true in config.
      if (typeof invoke === "function") {
        invoke("hud_ready").catch((err) => {
          console.warn("[HUD] hud_ready invoke failed:", err);
        });
      }
      return;
    }

    if (attemptsLeft > 0) {
      setTimeout(() => attachListener(attemptsLeft - 1), 50);
    } else {
      console.warn("[HUD] Tauri event API not available after retries.");
      if (el) el.textContent = "—";
    }
  }

  attachListener(30);
}

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", initHud);
} else {
  initHud();
}
// Boot guard handshake (see boot_guard.js): raised only when this script was
// parsed AND executed. A 404 or a SyntaxError leaves it unset, and the guard
// then reports the dead script to %TEMP%\nanoclick_web.log + shows a banner.
window.__nanoclick_hud_boot_ok__ = true;

