// ── nanoclick Statistics & Analytics Engine (thin client) ─────────
// Counting lives in Rust now (`src-tauri/src/stats_agg.rs`): the 66 ms
// telemetry tick folds into `SessionStats` via the `stats_tick` command and
// the backend persists totals itself. This module keeps ONLY what must live
// in the DOM: the live CPS ring for the Canvas chart, the DOM paint, and the
// localStorage backup (offline fallback when IPC is unavailable).

const LOCAL_STORAGE_KEY = "nanoclick_stats_backup";

const StatsEngine = {
  // Session counters mirrored from the backend (get_session_stats).
  // Never computed here — only painted.
  state: {
    sessionClicks: 0,
    runClicks: 0,
    sessionActiveMs: 0,
    activeNow: false,
    liveCpsHistory: [], // Max 60 rolling points for live Canvas chart
  },

  _lastCpsRecordMs: 0,
  _lastStatsRenderAt: 0,

  // Initialize or restore statistics from local backup if config was wiped
  init(config) {
    const st = this.ensureStatsConfig(config);
    this.restoreFromLocalStorage(st);
    this.saveToLocalStorage(st);
  },

  ensureStatsConfig(config) {
    if (!config || typeof config !== "object") return {};
    if (!config.stats || typeof config.stats !== "object") {
      config.stats = {
        total_clicks: 0,
        total_active_ms: 0,
        total_sessions: 0,
        presets_applied: 0,
        max_cps: 0.0,
        history: [],
      };
    }
    const st = config.stats;
    if (typeof st.total_clicks !== "number") st.total_clicks = 0;
    if (typeof st.total_active_ms !== "number") st.total_active_ms = 0;
    if (typeof st.total_sessions !== "number") st.total_sessions = 0;
    if (typeof st.presets_applied !== "number") st.presets_applied = 0;
    if (typeof st.max_cps !== "number") st.max_cps = 0;
    if (!Array.isArray(st.history)) st.history = [];
    return st;
  },

  // Record a rolling CPS point for the live Canvas chart
  recordCpsHistoryPoint(cps, active) {
    const now = Date.now();
    if (now - this._lastCpsRecordMs < 1000 && this.state.liveCpsHistory.length > 0) return;
    this._lastCpsRecordMs = now;
    this.state.liveCpsHistory.push({ time: now, cps: active ? (cps || 0) : 0 });
    if (this.state.liveCpsHistory.length > 60) {
      this.state.liveCpsHistory.shift();
    }
  },

  // Called on each IPC state tick from the Rust scheduler.
  // Counting + persistence live in the backend (stats_tick): this only
  // mirrors session counters for paint, keeps the CPS ring for Canvas,
  // and throttles the DOM render. saveConfigCallback stays as a fallback
  // for the offline path (no IPC — file:// preview, tests).
  async recordSessionTick(active, clicks_done, cps, config, _saveConfigCallback) {
    const nowMs = Date.now();
    const curCps = Math.max(0, Number(cps) || 0);
    const inv = window.__TAURI__?.core?.invoke;
    if (inv) {
      try {
        const res = await inv("stats_tick", {
          active: !!active,
          clicksDone: Math.max(0, Number(clicks_done) || 0),
          cps: curCps,
          nowMs,
        });
        if (res && typeof res === "object") {
          if (res.totals && config && typeof config === "object") {
            config.stats = res.totals;
          }
          try {
            const sess = await inv("get_session_stats");
            if (sess && typeof sess === "object") {
              this.state.sessionClicks = Number(sess.session_clicks) || 0;
              this.state.runClicks = Number(sess.run_clicks) || 0;
              this.state.sessionActiveMs = Number(sess.session_active_ms) || 0;
              this.state.activeNow = !!sess.active_now;
            }
          } catch (_) {}
        }
      } catch (_) {
        // IPC hiccup — fall through to paint with the last known state.
      }
    }

    this.recordCpsHistoryPoint(curCps, active);

    // Throttled stats rendering (max 4 Hz / every 250ms)
    const nowPerf = performance.now();
    if (nowPerf - this._lastStatsRenderAt >= 250) {
      this._lastStatsRenderAt = nowPerf;
      this.renderStats(config);
    }
  },

  fmtDuration(ms) {
    const sec = Math.floor(ms / 1000);
    if (sec < 60) return `${sec}s`;
    const min = Math.floor(sec / 60);
    if (min < 60) return `${min}m ${sec % 60}s`;
    const h = Math.floor(min / 60);
    return `${h}h ${min % 60}m`;
  },

  fmtNum(n) {
    const v = Number(n) || 0;
    return v > 999 ? v.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",") : String(v);
  },

  renderStats(config) {
    // Exact DOM ID match with index.html: <div id="viewStats" class="view">
    // Do NOT rename to "viewStatistics" — that ID does not exist in the DOM.
    const viewStats = document.getElementById("viewStats");
    if (!viewStats || !viewStats.classList.contains("active")) return;

    const sc = document.getElementById("statSessionClicks");
    const at = document.getElementById("statActiveTime");
    const ac = document.getElementById("statAvgCps");
    const tc = document.getElementById("statTotalClicks");
    const ta = document.getElementById("statTotalActiveTime");
    const mc = document.getElementById("statMaxCps");
    const statTotalSessionsEl = document.getElementById("statTotalSessions");
    const pa = document.getElementById("statPresetsApplied");

    if (sc) sc.textContent = this.fmtNum(this.state.sessionClicks);
    if (at) at.textContent = this.fmtDuration(this.state.sessionActiveMs);
    if (ac) {
      ac.textContent = this.state.sessionActiveMs > 500
        ? (this.state.sessionClicks / (this.state.sessionActiveMs / 1000)).toFixed(1)
        : "—";
    }

    const st = this.ensureStatsConfig(config);
    if (tc) tc.textContent = this.fmtNum(st.total_clicks);
    if (ta) ta.textContent = this.fmtDuration(st.total_active_ms);
    if (mc) mc.textContent = (Number(st.max_cps || 0)).toFixed(1);
    if (statTotalSessionsEl) statTotalSessionsEl.textContent = this.fmtNum(st.total_sessions);
    if (pa) pa.textContent = this.fmtNum(st.presets_applied);

    this.drawStatsChart();
  },

  drawStatsChart(canvasId = "statsChart") {
    const canvas = document.getElementById(canvasId);
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const rect = canvas.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return;

    const dpr = window.devicePixelRatio || 1;
    const targetW = Math.floor(rect.width * dpr);
    const targetH = Math.floor(rect.height * dpr);
    if (canvas.width !== targetW || canvas.height !== targetH) {
      canvas.width = targetW;
      canvas.height = targetH;
    }

    ctx.save();
    ctx.scale(dpr, dpr);
    const displayW = rect.width;
    const displayH = rect.height;

    ctx.clearRect(0, 0, displayW, displayH);

    // Background Grid Lines
    ctx.strokeStyle = "rgba(255, 255, 255, 0.06)";
    ctx.lineWidth = 1;
    for (let y = 20; y < displayH; y += 35) {
      ctx.beginPath();
      ctx.moveTo(0, y);
      ctx.lineTo(displayW, y);
      ctx.stroke();
    }

    const history = this.state.liveCpsHistory;
    if (!history || history.length < 2) {
      ctx.fillStyle = "rgba(255, 255, 255, 0.35)";
      ctx.font = "12px sans-serif";
      ctx.textAlign = "center";
      const placeholder = window.I18nEngine ? window.I18nEngine.t("stats_chart_empty", {}, "Start autoclicker to record & plot real-time CPS timeline") : "Start autoclicker to record & plot real-time CPS timeline";
      ctx.fillText(placeholder, displayW / 2, displayH / 2);
      ctx.restore();
      return;
    }

    let maxCps = 20;
    for (const p of history) {
      if (p.cps > maxCps) maxCps = p.cps;
    }
    maxCps *= 1.15;

    const paddingBottom = 20;
    const paddingTop = 15;
    const chartH = displayH - paddingTop - paddingBottom;
    const stepX = displayW / (Math.max(60, history.length) - 1);
    const startX = (60 - history.length) * stepX;

    // Gradient Area Fill
    const gradient = ctx.createLinearGradient(0, paddingTop, 0, displayH - paddingBottom);
    gradient.addColorStop(0, "rgba(6, 182, 212, 0.35)");
    gradient.addColorStop(1, "rgba(6, 182, 212, 0.0)");

    ctx.beginPath();
    ctx.moveTo(startX, displayH - paddingBottom);

    for (let i = 0; i < history.length; i++) {
      const x = startX + i * stepX;
      const y = displayH - paddingBottom - (history[i].cps / maxCps) * chartH;
      ctx.lineTo(x, y);
    }

    ctx.lineTo(startX + (history.length - 1) * stepX, displayH - paddingBottom);
    ctx.closePath();
    ctx.fillStyle = gradient;
    ctx.fill();

    // Glowing Line Path
    ctx.shadowColor = "#06b6d4";
    ctx.shadowBlur = 8;
    ctx.strokeStyle = "#06b6d4";
    ctx.lineWidth = 2.5;

    ctx.beginPath();
    for (let i = 0; i < history.length; i++) {
      const x = startX + i * stepX;
      const y = displayH - paddingBottom - (history[i].cps / maxCps) * chartH;
      if (i === 0) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    }
    ctx.stroke();
    ctx.shadowBlur = 0;

    // Current CPS Dot & Label
    const lastPoint = history[history.length - 1];
    const lastX = startX + (history.length - 1) * stepX;
    const lastY = displayH - paddingBottom - (lastPoint.cps / maxCps) * chartH;

    ctx.fillStyle = "#22d3ee";
    ctx.beginPath();
    ctx.arc(lastX, lastY, 4, 0, Math.PI * 2);
    ctx.fill();

    ctx.fillStyle = "#ffffff";
    ctx.font = "bold 11px 'Fira Code', monospace";
    ctx.textAlign = "right";
    ctx.fillText(`${lastPoint.cps.toFixed(1)} CPS`, displayW - 10, paddingTop + 10);

    ctx.restore();
  },

  resetStats(config, saveConfigCallback) {
    if (!config) return;
    config.stats = {
      total_clicks: 0,
      total_active_ms: 0,
      total_sessions: 0,
      presets_applied: 0,
      max_cps: 0.0,
      history: [],
    };
    this.state.sessionClicks = 0;
    this.state.sessionActiveMs = 0;
    try {
      localStorage.removeItem(LOCAL_STORAGE_KEY);
    } catch (_) {}
    if (typeof saveConfigCallback === "function") saveConfigCallback();
    this.renderStats(config);
  },

  saveToLocalStorage(st) {
    try {
      if (st && typeof st === "object") {
        localStorage.setItem(LOCAL_STORAGE_KEY, JSON.stringify(st));
      }
    } catch (_) {}
  },

  restoreFromLocalStorage(st) {
    try {
      if (st && st.total_clicks === 0) {
        const raw = localStorage.getItem(LOCAL_STORAGE_KEY);
        if (raw) {
          const parsed = JSON.parse(raw);
          if (parsed && typeof parsed === "object" && parsed.total_clicks > 0) {
            Object.assign(st, parsed);
          }
        }
      }
    } catch (_) {}
  },
};

// Expose globally for classic scripts
if (typeof window !== "undefined") {
  window.StatsEngine = StatsEngine;
}
