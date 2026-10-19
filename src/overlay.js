// nanoclick Visual Click Ripple Overlay Engine
// Zero-overhead hardware-accelerated Canvas 2D ripple renderer.
// Completely eliminates DOM allocation churn, orphaned nodes, and timer leaks.

// Suppress native context menu on all overlay pages (right-click click mode support).
window.addEventListener("contextmenu", (e) => e.preventDefault(), true);

function initOverlay() {
  const canvas = document.getElementById("rippleCanvas");
  if (!canvas) return;

  const ctx = canvas.getContext("2d", { alpha: true });
  if (!ctx) return;

  let dpr = window.devicePixelRatio || 1;

  const MAX_CONCURRENT_RIPPLES = 25;
  const RIPPLE_DURATION_MS = 450;
  let ripples = [];
  let isAnimating = false;

  function updateCanvasSize() {
    dpr = window.devicePixelRatio || 1;
    const w = window.innerWidth;
    const h = window.innerHeight;
    canvas.width = Math.floor(w * dpr);
    canvas.height = Math.floor(h * dpr);
    // A resize re-scales the plane: dots stored in the old scale would jump.
    // Drop them — a 450 ms eye-candy gap beats dots drawn in the wrong place.
    ripples.length = 0;
  }

  window.addEventListener("resize", updateCanvasSize);
  updateCanvasSize();

  function render(now) {
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, window.innerWidth, window.innerHeight);

    let activeCount = 0;
    for (let i = 0; i < ripples.length; i++) {
      const r = ripples[i];
      const elapsed = now - r.start;
      if (elapsed < RIPPLE_DURATION_MS) {
        ripples[activeCount++] = r;
        const progress = elapsed / RIPPLE_DURATION_MS;
        // Ease-out cubic: snappy initial expansion, smooth settling
        const ease = 1 - Math.pow(1 - progress, 3);
        const radius = 5 + ease * 35;
        const alpha = 1 - progress;

        ctx.beginPath();
        ctx.arc(r.x, r.y, radius, 0, Math.PI * 2);
        ctx.fillStyle = `rgba(110, 190, 255, ${0.45 * alpha})`;
        ctx.fill();
        ctx.lineWidth = 2;
        ctx.strokeStyle = `rgba(110, 190, 255, ${0.85 * alpha})`;
        ctx.stroke();
      }
    }
    ripples.length = activeCount;

    if (activeCount > 0) {
      requestAnimationFrame(render);
    } else {
      // The last frame already cleared + painted nothing: stop, no second
      // clear (it flashed a blank frame on some GPUs).
      isAnimating = false;
    }
  }

  function spawnRipple(rawX, rawY) {
    // NaN guard: a torn IPC payload (DPR=0 frame, destroyed WebView mid-emit)
    // used to plant NaN into ctx.arc — an uncaught exception that killed the
    // whole overlay renderer. Drop the dot, keep the loop alive.
    if (!Number.isFinite(rawX) || !Number.isFinite(rawY)) return;
    // DPR guard: a zero-DPR frame yields (0,0) — a dot in the corner instead
    // of a crash, but still noise. Clamp the divisor, never divide by zero.
    // SINGLE OWNER: callers pass RAW pixels, division lives ONLY here.
    // `dpr` is the outer closure binding (kept fresh by updateCanvasSize).
    const scale = Number.isFinite(dpr) && dpr > 0 ? dpr : 1;
    const visualX = rawX / scale;
    const visualY = rawY / scale;
    if (!Number.isFinite(visualX) || !Number.isFinite(visualY)) return;
    if (ripples.length >= MAX_CONCURRENT_RIPPLES) {
      ripples.shift();
    }
    ripples.push({
      x: visualX,
      y: visualY,
      start: performance.now(),
    });

    if (!isAnimating) {
      isAnimating = true;
      requestAnimationFrame(render);
    }
  }

  // Attach listener and signal readiness with retry loop (matching hud.js pattern).
  // On a cold WebView2 the IPC bridge (window.__TAURI__) may not exist yet at DOMContentLoaded.
  // Re-reading window.__TAURI__ on each tick ensures both listen() and invoke() succeed.
  function attachListener(attemptsLeft) {
    const tauri = window.__TAURI__;
    const listen = tauri?.event?.listen;
    const invoke = tauri?.core?.invoke;

    if (typeof listen === "function") {
      listen("spawn-ripple", (event) => {
        const payload = event.payload;
        if (!payload) return;

        // Handle coalesced batch of coordinates: [[x1, y1], [x2, y2], ...]
        // RAW pixels — spawnRipple owns the DPR division, never divide here.
        if (Array.isArray(payload)) {
          if (payload.length > 0 && Array.isArray(payload[0])) {
            for (let i = 0; i < payload.length; i++) {
              const pt = payload[i];
              spawnRipple(pt[0], pt[1]);
            }
            return;
          }
          if (payload.length >= 2 && typeof payload[0] === "number") {
            spawnRipple(payload[0], payload[1]);
            return;
          }
        }

        const rawX = payload?.x ?? 0;
        const rawY = payload?.y ?? 0;
        spawnRipple(rawX, rawY);
      });

      // Signal Rust that the transparent document is rendered and ready to show.
      // Eliminates the WebView2 DWM white flash on window creation (Zero-Flash init).
      if (typeof invoke === "function") {
        invoke("overlay_ready").catch((err) => {
          console.warn("[Overlay] overlay_ready invoke failed:", err);
        });
      }
      return;
    }

    if (attemptsLeft > 0) {
      setTimeout(() => attachListener(attemptsLeft - 1), 50);
    } else {
      console.warn("[Overlay] Tauri event/invoke API not available after retries — window stays hidden.");
    }
  }

  attachListener(60);
}

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", initOverlay);
} else {
  initOverlay();
}
// Boot guard handshake (see boot_guard.js) — overlay window.
window.__nanoclick_overlay_boot_ok__ = true;

