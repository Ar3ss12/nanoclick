// nanoclick Visual Click Ripple Overlay Engine
// Manages zero-overhead hardware-accelerated ripple animations across the screen.

document.addEventListener("DOMContentLoaded", async () => {
  const tauri = window.__TAURI__;
  const container = document.getElementById("rippleContainer");

  // Keep a small bound on simultaneous DOM nodes to eliminate memory growth at high CPS (e.g. 100 CPS)
  const MAX_CONCURRENT_RIPPLES = 30;

  function spawnRipple(visualX, visualY) {
    if (!container) return;

    if (container.children.length >= MAX_CONCURRENT_RIPPLES) {
      // Remove oldest ripple to keep rendering fast and lean
      container.removeChild(container.firstChild);
    }

    const dot = document.createElement("div");
    dot.className = "ripple-dot";
    dot.style.left = `${Math.round(visualX)}px`;
    dot.style.top = `${Math.round(visualY)}px`;

    container.appendChild(dot);

    // Auto-cleanup DOM node once CSS keyframe animation completes
    setTimeout(() => {
      if (dot.parentNode) {
        dot.parentNode.removeChild(dot);
      }
    }, 650);
  }

  // Subscribe to click event emitted by Rust click loop
  if (tauri?.event?.listen) {
    tauri.event.listen("spawn-ripple", (event) => {
      const payload = event.payload;
      if (!payload) return;

      const rawX = Array.isArray(payload) ? payload[0] : (payload?.x ?? 0);
      const rawY = Array.isArray(payload) ? payload[1] : (payload?.y ?? 0);

      // Windows Per-Monitor DPI Correction:
      // Rust GetCursorPos returns physical screen pixels, while CSS WebView2 uses logical CSS pixels.
      const dpr = window.devicePixelRatio || 1;
      const visualX = rawX / dpr;
      const visualY = rawY / dpr;

      spawnRipple(visualX, visualY);
    });
  }

  // Signal Rust that the transparent document is rendered and ready to show.
  // This completely eliminates the WebView2 DWM white flash on window creation.
  if (tauri?.core?.invoke) {
    try {
      await tauri.core.invoke("overlay_ready");
    } catch (err) {
      console.warn("[Overlay] overlay_ready signal error:", err);
    }
  }
});
