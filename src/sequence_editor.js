// ── Multi-point sequence canvas editor (F9 v2) ────────────────────
// Coordinates are real screen coordinates (not canvas-local). The
// canvas shows a viewport of the primary screen and overlays grid
// lines every 50px so users can roughly estimate where they are
// clicking. Snap-to-grid (25 px) is available via the shift key.

const SE = (window.SequenceEditor = (() => {
  let points = [];
  let canvas, ctx, countEl, jsonEl;
  let draggingIdx = -1;
  let lastPrimaryW = 1920;
  let lastPrimaryH = 1080;
  const SNAP = 25;
  const GUTTER = 24;

  function getInvoke() {
    return window.invoke || window.__TAURI__?.core?.invoke || null;
  }

  async function refreshScreenSize() {
    try {
      const inv = getInvoke();
      if (!inv) return;
      const w = await inv("get_primary_screen_size").catch(() => null);
      if (w && w.width && w.height) {
        lastPrimaryW = w.width;
        lastPrimaryH = w.height;
      }
    } catch (_) {}
  }

  function getCanvasPos(ev) {
    const rect = canvas.getBoundingClientRect();
    const cx = (ev.clientX - rect.left) * (canvas.width / rect.width);
    const cy = (ev.clientY - rect.top) * (canvas.height / rect.height);
    return mapCanvasToScreen(cx, cy);
  }

  function mapCanvasToScreen(cx, cy) {
    const innerW = canvas.width - 2 * GUTTER;
    const innerH = canvas.height - 2 * GUTTER;
    const screenRatio = lastPrimaryW / lastPrimaryH;
    const canvasRatio = innerW / innerH;
    let scale, offsetX = GUTTER, offsetY = GUTTER;
    if (screenRatio > canvasRatio) {
      scale = innerH / lastPrimaryH;
      const w = lastPrimaryW * scale;
      offsetX = GUTTER + (innerW - w) / 2;
    } else {
      scale = innerW / lastPrimaryW;
      const h = lastPrimaryH * scale;
      offsetY = GUTTER + (innerH - h) / 2;
    }
    const sx = Math.round((cx - offsetX) / scale);
    const sy = Math.round((cy - offsetY) / scale);
    return { x: sx, y: sy, scale, offsetX, offsetY };
  }

  function screenToCanvas(x, y) {
    const r = mapCanvasToScreen(0, 0);
    return { cx: r.offsetX + x * r.scale, cy: r.offsetY + y * r.scale, scale: r.scale };
  }

  function findHit(rawX, rawY) {
    for (let i = 0; i < points.length; i++) {
      const p = screenToCanvas(points[i].x, points[i].y);
      const dx = rawX - p.cx;
      const dy = rawY - p.cy;
      if (dx * dx + dy * dy <= 100) return i;
    }
    return -1;
  }

  function maybeSnap(x, y, ev) {
    if (!ev.shiftKey) return { x, y };
    return { x: Math.round(x / SNAP) * SNAP, y: Math.round(y / SNAP) * SNAP };
  }

  function draw() {
    if (!ctx) return;
    const W = canvas.width;
    const H = canvas.height;
    ctx.clearRect(0, 0, W, H);
    ctx.fillStyle = "#0d1117";
    ctx.fillRect(0, 0, W, H);
    ctx.strokeStyle = "#30363d";
    ctx.lineWidth = 1;
    ctx.strokeRect(0.5, 0.5, W - 1, H - 1);

    // ── Cache transform coefficients once per frame ──────────────────────────
    // mapCanvasToScreen() is a pure function of (canvas.width, canvas.height,
    // lastPrimaryW, lastPrimaryH) — all constant within a single draw() call.
    // Calling it for every grid line and every point is unnecessary work.
    const innerW = W - 2 * GUTTER;
    const innerH = H - 2 * GUTTER;
    const screenRatio = lastPrimaryW / lastPrimaryH;
    const canvasRatio = innerW / innerH;
    let scale, offsetX = GUTTER, offsetY = GUTTER;
    if (screenRatio > canvasRatio) {
      scale = innerH / lastPrimaryH;
      const w = lastPrimaryW * scale;
      offsetX = GUTTER + (innerW - w) / 2;
    } else {
      scale = innerW / lastPrimaryW;
      const h = lastPrimaryH * scale;
      offsetY = GUTTER + (innerH - h) / 2;
    }

    // Inline converter: screen → canvas using the cached coefficients.
    const toCanvas = (sx, sy) => ({
      cx: offsetX + sx * scale,
      cy: offsetY + sy * scale,
    });

    // Grid every 50 screen px.
    const dx = 50 * scale;          // was: screenToCanvas(50,0).cx - GUTTER
    const dy = 50 * scale;          // was: screenToCanvas(0,50).cy - GUTTER
    if (dx > 8) {
      ctx.strokeStyle = "#1c2128";
      ctx.beginPath();
      for (let x = offsetX; x < W - GUTTER; x += dx) {
        ctx.moveTo(x, GUTTER);
        ctx.lineTo(x, H - GUTTER);
      }
      for (let y = offsetY; y < H - GUTTER; y += dy) {
        ctx.moveTo(GUTTER, y);
        ctx.lineTo(W - GUTTER, y);
      }
      ctx.stroke();
    }

    // Origin marker.
    // screenToCanvas(0,0) with cached values: cx = offsetX, cy = offsetY.
    if (offsetX > GUTTER && offsetY > GUTTER) {
      ctx.fillStyle = "#58a6ff";
      ctx.font = "10px monospace";
      ctx.textAlign = "left";
      ctx.textBaseline = "bottom";
      ctx.fillText("(0,0)", offsetX + 2, offsetY - 2);
    }

    // Path lines between points.
    if (points.length >= 2) {
      ctx.strokeStyle = "#58a6ff";
      ctx.lineWidth = 2;
      ctx.beginPath();
      for (let i = 0; i < points.length; i++) {
        const p = toCanvas(points[i].x, points[i].y);
        if (i === 0) ctx.moveTo(p.cx, p.cy);
        else ctx.lineTo(p.cx, p.cy);
      }
      ctx.stroke();
    }

    // Points.
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    points.forEach((p, i) => {
      const cp = toCanvas(p.x, p.y);
      const isFirst = i === 0;
      ctx.fillStyle = isFirst ? "#3fb950" : "#f78166";
      ctx.beginPath();
      ctx.arc(cp.cx, cp.cy, 7, 0, Math.PI * 2);
      ctx.fill();
      ctx.fillStyle = "#0d1117";
      ctx.font = "bold 10px sans-serif";
      ctx.fillText(String(i + 1), cp.cx, cp.cy);
      // Label with delay.
      ctx.fillStyle = "#c9d1d9";
      ctx.font = "9px monospace";
      ctx.textAlign = "left";
      ctx.fillText(`+${p.delay_ms || 0}ms`, cp.cx + 10, cp.cy - 10);
      ctx.textAlign = "center";
    });

    // Update count + JSON.
    if (countEl) countEl.textContent = String(points.length);
    if (jsonEl) jsonEl.value = JSON.stringify(points);
  }

  function setPoints(arr) {
    points = Array.isArray(arr)
      ? arr
          .filter((p) => p && Number.isFinite(p.x) && Number.isFinite(p.y))
          .map((p) => ({
            x: p.x | 0,
            y: p.y | 0,
            delay_ms: p.delay_ms | 0,
          }))
      : [];
    draw();
  }

  function getPoints() {
    return JSON.parse(JSON.stringify(points));
  }

  function bindCanvas() {
    canvas.addEventListener("click", (ev) => {
      if (draggingIdx >= 0) return;
      const hit = findHit(ev.offsetX, ev.offsetY);
      if (hit >= 0) return;
      const { x, y } = getCanvasPos(ev);
      const snapped = maybeSnap(x, y, ev);
      if (snapped.x < 0 || snapped.y < 0) return;
      points.push({ x: snapped.x, y: snapped.y, delay_ms: 50 });
      draw();
    });
    canvas.addEventListener("dblclick", (ev) => {
      const hit = findHit(ev.offsetX, ev.offsetY);
      if (hit >= 0) {
        points.splice(hit, 1);
        draw();
      }
    });
    canvas.addEventListener("mousedown", (ev) => {
      const hit = findHit(ev.offsetX, ev.offsetY);
      if (hit >= 0) {
        draggingIdx = hit;
        canvas.style.cursor = "grabbing";
        ev.preventDefault();
      }
    });
    window.addEventListener("mousemove", (ev) => {
      if (draggingIdx < 0) return;
      const rect = canvas.getBoundingClientRect();
      const cx = (ev.clientX - rect.left) * (canvas.width / rect.width);
      const cy = (ev.clientY - rect.top) * (canvas.height / rect.height);
      const { x, y } = mapCanvasToScreen(cx, cy);
      const snapped = maybeSnap(x, y, ev);
      points[draggingIdx].x = Math.max(0, snapped.x);
      points[draggingIdx].y = Math.max(0, snapped.y);
      draw();
    });
    window.addEventListener("mouseup", () => {
      if (draggingIdx >= 0) {
        draggingIdx = -1;
        canvas.style.cursor = "crosshair";
      }
    });
  }

  function bindToolbar() {
    const addBtn = document.getElementById("prPointsAdd");
    const clearBtn = document.getElementById("prPointsClear");
    const capBtn = document.getElementById("prPointsCapture");
    if (addBtn) {
      addBtn.addEventListener("click", () => {
        points.push({
          x: (lastPrimaryW / 2) | 0,
          y: (lastPrimaryH / 2) | 0,
          delay_ms: 50,
        });
        draw();
      });
    }
    if (clearBtn) {
      clearBtn.addEventListener("click", () => {
        points = [];
        draw();
      });
    }
    if (capBtn) {
      capBtn.addEventListener("click", async () => {
        try {
          const inv = getInvoke();
          if (!inv) return;
          const pos = await inv("get_cursor_pos_now");
          if (pos && Number.isFinite(pos.x) && Number.isFinite(pos.y)) {
            points.unshift({ x: pos.x | 0, y: pos.y | 0, delay_ms: 0 });
            draw();
          }
        } catch (e) {
          console.warn("[SequenceEditor] capture failed", e);
        }
      });
    }
  }

  async function init() {
    canvas = document.getElementById("prPointsCanvas");
    if (!canvas) return;
    try {
      ctx = canvas.getContext("2d");
      if (!ctx) return;
    } catch (e) {
      try { console.error("[sequence_editor] getContext('2d') failed:", e); } catch (_) {}
      return;
    }
    countEl = document.getElementById("prPointsCount");
    jsonEl = document.getElementById("prPoints");
    await refreshScreenSize();
    try { bindCanvas(); } catch (e) { try { console.error("[sequence_editor] bindCanvas failed:", e); } catch (_) {} }
    try { bindToolbar(); } catch (e) { try { console.error("[sequence_editor] bindToolbar failed:", e); } catch (_) {} }
    try { draw(); } catch (e) { try { console.error("[sequence_editor] draw failed:", e); } catch (_) {} }
    window.addEventListener("resize", () => { try { draw(); } catch (_) {} });
  }

  return { init, setPoints, getPoints, draw };
})());

if (document.readyState === "loading") {
  document.addEventListener("DOMContentLoaded", () => {
    try { SE.init(); } catch (e) { try { console.error("[sequence_editor] init failed:", e); } catch (_) {} }
  });
} else {
  try { SE.init(); } catch (e) { try { console.error("[sequence_editor] init failed:", e); } catch (_) {} }
}
