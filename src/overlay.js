// nanoclick Visual Click Ripple Overlay Engine
// Zero-overhead hardware-accelerated Canvas 2D ripple renderer.
// Completely eliminates DOM allocation churn, orphaned nodes, and timer leaks.

document.addEventListener("DOMContentLoaded", async () => {
  const tauri = window.__TAURI__;
  const canvas = document.getElementById("rippleCanvas");
  if (!canvas) return;

  const ctx = canvas.getContext("2d", { alpha: true });
  if (!ctx) return;

  let dpr = window.devicePixelRatio || 1;

  function updateCanvasSize() {
    dpr = window.devicePixelRatio || 1;
    const w = window.innerWidth;
    const h = window.innerHeight;
    canvas.width = Math.floor(w * dpr);
    canvas.height = Math.floor(h * dpr);
  }

  window.addEventListener("resize", updateCanvasSize);
  updateCanvasSize();

  const MAX_CONCURRENT_RIPPLES = 25;
  const RIPPLE_DURATION_MS = 450;
  let ripples = [];
  let isAnimating = false;

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
      isAnimating = false;
      ctx.clearRect(0, 0, window.innerWidth, window.innerHeight);
    }
  }

  function spawnRipple(visualX, visualY) {
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

  // Subscribe to click event emitted by Rust click loop
  if (tauri?.event?.listen) {
    tauri.event.listen("spawn-ripple", (event) => {
      const payload = event.payload;
      if (!payload) return;

      const currentDpr = window.devicePixelRatio || 1;

      // Handle coalesced batch of coordinates: [[x1, y1], [x2, y2], ...]
      if (Array.isArray(payload)) {
        if (payload.length > 0 && Array.isArray(payload[0])) {
          for (let i = 0; i < payload.length; i++) {
            const pt = payload[i];
            spawnRipple(pt[0] / currentDpr, pt[1] / currentDpr);
          }
          return;
        }
        if (payload.length >= 2 && typeof payload[0] === "number") {
          spawnRipple(payload[0] / currentDpr, payload[1] / currentDpr);
          return;
        }
      }

      const rawX = payload?.x ?? 0;
      const rawY = payload?.y ?? 0;
      spawnRipple(rawX / currentDpr, rawY / currentDpr);
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
