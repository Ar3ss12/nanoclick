// ── nanoclick Statistics & Analytics Engine ────────────────────────
// Independent module for tracking runtime clicks, session durations,
// real-time CPS graphing, and persistent storage across reinstalls.

const LOCAL_STORAGE_KEY = "nanoclick_stats_backup";

const StatsEngine = {
  // Runtime in-memory metrics for the current application session
  state: {
    sessionClicks: 0,    // Total clicks accumulated across ALL runs this app session
    runClicks: 0,        // Clicks in the CURRENT run only (resets on each START)
    sessionActiveMs: 0,
    lastUpdate: null,
    activeNow: false,
    lastClicksDone: 0,   // Last known clicks_done value; reset to 0 on STOP
    lastDiskSave: 0,
    liveCpsHistory: [], // Max 60 rolling points for live Canvas chart
    // ── Idempotent finalization (double-flush guard) ──
    // Zapiznilyi 66ms worker tick after STOP must be a no-op, never a 2nd push.
    lastFinalizedAt: 0,      // nowMs of the last history.push
    lastFinalizedClicks: -1, // clicks value that was finalized
    // ── Dirty flag: disk writes only when clicks actually happened ──
    statsDirty: false,
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

  // Called on each IPC state tick from the Rust scheduler
  // saveConfigCallback is called ONLY on: START, STOP-finalize, and the
  // 5s dirty-flush (when statsDirty). Never in idle — disk sleeps.
  recordSessionTick(active, clicks_done, cps, config, saveConfigCallback) {
    const nowMs = Date.now();
    const clicks = Math.max(0, Number(clicks_done) || 0);
    const st = this.ensureStatsConfig(config);
    const curCps = Math.max(0, Number(cps) || 0);
    const markDirty = () => { this.state.statsDirty = true; };

    if (active) {
      if (!this.state.activeNow) {
        // ── New run started: reset per-run baseline ──
        this.state.activeNow = true;
        this.state.runClicks = 0;       // Reset per-run counter on each START
        this.state.lastClicksDone = 0;  // Always count from 0 so deltas are correct
        this.state.lastUpdate = nowMs;
        st.total_sessions = (Number(st.total_sessions) || 0) + 1;
        this.saveToLocalStorage(st);
        if (typeof saveConfigCallback === "function") saveConfigCallback();
      }

      // ── Always accumulate delta for active ticks (including the first tick!) ──
      const deltaClicks = clicks > this.state.lastClicksDone ? (clicks - this.state.lastClicksDone) : 0;
      this.state.lastClicksDone = clicks;
      if (deltaClicks > 0) {
        this.state.runClicks += deltaClicks;      // Per-run accumulator
        this.state.sessionClicks += deltaClicks;  // Session-wide accumulator
        st.total_clicks = (Number(st.total_clicks) || 0) + deltaClicks;
        markDirty();
      }

      if (this.state.lastUpdate) {
        const deltaMs = nowMs - this.state.lastUpdate;
        if (deltaMs > 0 && deltaMs < 5000) {
          this.state.sessionActiveMs += deltaMs;
          st.total_active_ms = (Number(st.total_active_ms) || 0) + deltaMs;
        }
      }
      this.state.lastUpdate = nowMs;

      if (curCps > (Number(st.max_cps) || 0)) {
        st.max_cps = Number(curCps.toFixed(1));
      }

      // ── 5s dirty-flush: ONE disk write per 5s of clicking, ZERO in idle ──
      if (nowMs - (this.state.lastDiskSave || 0) > 5000) {
        this.state.lastDiskSave = nowMs;
        if (this.state.statsDirty) {
          this.state.statsDirty = false;
          this.saveToLocalStorage(st);
          if (typeof saveConfigCallback === "function") saveConfigCallback();
        }
      }
    } else {
      // Late echo AFTER finalize: activeNow is already false — pure no-op.
      // (The twin tick 550 -> 551 lands here, not in the branch below.)
      if (!this.state.activeNow) {
        return;
      }
      // ── Run ended: EXACTLY-ONCE finalize (single entry point) ──
      // Reached only on the true active->idle transition.
      // ── Capture final click delta, log it, and reset lastClicksDone ──
      const deltaClicks = clicks > this.state.lastClicksDone ? (clicks - this.state.lastClicksDone) : 0;
      if (deltaClicks > 0) {
        this.state.runClicks += deltaClicks;
        this.state.sessionClicks += deltaClicks;
        st.total_clicks = (Number(st.total_clicks) || 0) + deltaClicks;
      }

      this.state.activeNow = false;
      this.state.lastUpdate = null;
      this.state.lastClicksDone = 0; // Critical: reset so next START doesn't skip clicks
      // ── Junk filter: skip noise runs (accidental hotkey taps) ──
      // Counters above still grow; only the history chart stays clean.
      const isJunk = this.state.runClicks < 5 && this.state.sessionActiveMs < 1000;
      if (!isJunk && (this.state.runClicks > 0 || this.state.sessionActiveMs > 1000)) {
        const avgVal = this.state.sessionActiveMs > 0
          ? (this.state.runClicks / (this.state.sessionActiveMs / 1000))
          : 0;
        if (!Array.isArray(st.history)) st.history = [];
        st.history.push({
          timestamp: nowMs,
          clicks: this.state.runClicks,         // Log per-run clicks in history
          active_ms: this.state.sessionActiveMs,
          avg_cps: Number(avgVal.toFixed(1)),
        });
        // Ring buffer cap: stats.json never grows past ~6 KB.
        if (st.history.length > 50) st.history.shift();
        this.state.lastFinalizedAt = nowMs;
        this.state.lastFinalizedClicks = clicks;
      }
      this.state.statsDirty = false; // STOP always flushes synchronously below
      this.saveToLocalStorage(st);
      if (typeof saveConfigCallback === "function") saveConfigCallback();
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
