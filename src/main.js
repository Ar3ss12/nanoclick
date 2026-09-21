// ── Tauri 2.x API accessors ─────────────────────────────────────
// `withGlobalTauri: true` in tauri.conf.json exposes `window.__TAURI__`
// with all sub-modules: app, core (invoke), dpi, event, image, menu,
// mocks, path, tray, webview, webviewWindow, window.
//
// We use the public `core.invoke` for command calls and `event.listen`
// for event subscriptions — both are synchronous wrappers around the
// Tauri 2.x internals.
const TAURI = (typeof window !== "undefined" && window.__TAURI__) || null;

// ── BOOT CANARY (v1.1.0) ──────────────────────────────────────────
// WHY: when the UI "does nothing", the release log used to stay silent —
// frontend diagnostics are sent through the debug-gated logger, so a JS
// failure at boot left NO trace at all (we could only see that PageLoad
// finished). This canary is sent at "warn" level, which the backend always
// writes (see `debug_log_internal`), so the next boot tells us exactly how
// far the frontend got and which step failed.
function bootCanary(label) {
  try {
    const inv = window.__TAURI__?.core?.invoke;
    if (inv) { inv("debug_log", { level: "warn", message: `[BOOT] ${label}` }).catch(() => {}); }
  } catch (_) { /* the canary must never break the page */ }
}
bootCanary("main.js evaluated");

// Forward a client-side crash to the Rust log (%TEMP%\nanoclick_web.log).
// Level "error" (and "warn") is written even in release builds — the backend
// filters only "info" when debug mode is off (see `debug_log_internal` in
// src-tauri/src/lib.rs). Without this, an uncaught error in release builds was
// visible ONLY in DevTools, and release builds deliberately keep DevTools off.
function sendClientError(level, message) {
  try {
    const inv = getRawInvoke();
    if (!inv) return;
    inv("debug_log", {
      level: level === "warn" ? "warn" : "error",
      message: String(message).slice(0, 1500),
    }).catch(() => {});
  } catch (_) {
    /* a logger must never throw */
  }
}

// ── DEBUG MODE INFRASTRUCTURE ────────────────────────────────────
// Verbose UI & IPC logs are generated ONLY when DEBUG_UI is true.
// Toggled via config or setDebugMode().
//
// **Disabled by default** for v1.2.x release builds — the global console
// interceptor combined with `setInterval(flushLogBuffer, 1000)` was the
// root cause of the "Out of Memory" WebView2 crash on startup: every
// console.error during the brittle `init()` flow fed enqueueLog →
// logBuffer → batch IPC → backend → response, and the WebView2 child
// processes ballooned past the per-process limit before the page ever
// finished loading. Users can re-enable it from Settings → Debug.
let DEBUG_UI = false;

function setDebugMode(enabled) {
  DEBUG_UI = !!enabled;
  getRawInvoke()?.("set_debug_mode", { enabled: DEBUG_UI })?.catch(() => {});
  if (DEBUG_UI && _logFlushTimer === null) {
    _logFlushTimer = setInterval(flushLogBuffer, 1000);
  } else if (!DEBUG_UI && _logFlushTimer !== null) {
    clearInterval(_logFlushTimer);
    _logFlushTimer = null;
    // Drain whatever is still buffered so it doesn't leak.
    if (logBuffer.length > 0) flushLogBuffer();
  }
}

// ── LOGGING INFRASTRUCTURE ──────────────────────────────────────
const origLog = console.log;
const origError = console.error;
const origWarn = console.warn;

// ── BACKGROUND MEMORY GUARD (v1.1.0) ──────────────────────────────
// A hidden WebView2 window keeps its renderer alive, and page timers stop it
// from going idle — pure RAM/CPU burn for a clicker that spends most of its
// life minimized to tray while it works. `everyVisible` keeps the periodic UI
// work (stats render, toast polling) suspended while the window is hidden and
// resumes it automatically on show. The Rust click loop and the LL hooks are
// untouched: this gates the UI layer only (Zero-Jitter Mandate §2 explicitly
// allows UI-side savings; the hot path stays syscall-free).
function uiVisible() {
  try { return document.visibilityState !== "hidden"; } catch (_) { return true; }
}
function everyVisible(ms, fn) {
  return setInterval(() => { if (uiVisible()) fn(); }, ms);
}

const logBuffer = [];
let isFlushingLogs = false;

function enqueueLog(level, args) {
  if (!DEBUG_UI) return;
  if (logBuffer.length > 1000) logBuffer.shift();
  try {
    const msg = args.map(a => {
      if (typeof a === 'string') return a;
      try { return JSON.stringify(a); }
      catch { return String(a); }
    }).join(' ');
    logBuffer.push({ level, message: msg });
  } catch (_) {}
}

function flushLogBuffer() {
  if (!DEBUG_UI || isFlushingLogs || logBuffer.length === 0) return;
  isFlushingLogs = true;
  const batch = logBuffer.splice(0, 40);
  try {
    const inv = getRawInvoke();
    if (inv) {
      inv("debug_log_batch", { logs: batch }).catch(() => {});
    }
  } catch (_) {
  } finally {
    isFlushingLogs = false;
  }
}

// Log flushing timer — started lazily by setDebugMode(true) instead of at
// module load, so the disabled-by-default path has zero overhead.
let _logFlushTimer = null;

function dbg(...args) {
  if (DEBUG_UI) {
    origLog.call(console, "[UI-DBG]", ...args);
    enqueueLog("info", ["[UI-DBG]", ...args]);
  }
}

function ts() {
  const d = new Date();
  return d.toTimeString().slice(0, 8) + "." + String(d.getMilliseconds()).padStart(3, "0");
}

function logCall(direction, label, extra) {
  if (!DEBUG_UI) return;
  if (typeof label === "string" && label.startsWith("debug_log")) return;
  try {
    origLog.call(console, `[${ts()}] [${direction}] ${label}`, extra ?? "");
    enqueueLog("info", [`[${ts()}] [${direction}] ${label}`, extra ?? ""]);
  } catch (_) {}
}

function getRawInvoke() {
  return (typeof window !== "undefined" && window.__TAURI__?.core?.invoke)
      || (typeof window !== "undefined" && window.__TAURI_INTERNALS__?.invoke)
      || null;
}

function getRawListen() {
  return (typeof window !== "undefined" && window.__TAURI__?.event?.listen)
      || null;
}

console.log = (...args) => { origLog.apply(console, args); if (DEBUG_UI) enqueueLog("info", args); };
console.error = (...args) => { origError.apply(console, args); if (DEBUG_UI) enqueueLog("error", args); };
console.warn = (...args) => { origWarn.apply(console, args); if (DEBUG_UI) enqueueLog("warn", args); };

// ── GLOBAL ERROR TRAPS (fallback only — boot_guard.js owns the real ones) ──
// `boot_guard.js` is a CLASSIC script loaded before this module, so it is the
// only trap that can fire for a module-level SyntaxError (when the module dies,
// nothing in this file ever runs). These listeners stay as a fallback for the
// case where the guard is missing from the HTML, and they stand down while it
// is installed so that a single crash is never reported twice.
if (typeof window !== "undefined" && !window.__nanoclick_boot_guard_installed__) {
  window.addEventListener("error", (e) => {
    const msg = `[uncaught-error] ${e.message || "Unknown error"} at ${(e.filename || "main.js")}:${(e.lineno || 0)}:${(e.colno || 0)}`;
    origError.call(console, msg, e.error?.stack || "");
    dbg("FATAL EXCEPTION DETECTED:", msg);
    sendClientError("error", `${msg}${e.error?.stack ? "\n" + e.error.stack : ""}`);
  });
  window.addEventListener("unhandledrejection", (e) => {
    const reasonStr = e.reason instanceof Error ? (e.reason.stack || e.reason.message) : String(e.reason);
    origError.call(console, "[unhandled-rejection]", reasonStr);
    dbg("FATAL UNHANDLED PROMISE REJECTION:", reasonStr);
    sendClientError("error", `[unhandled-rejection] ${reasonStr}`);
  });
}

const invoke = async function(cmd, args) {
  if (cmd === "debug_log") {
    const rawInvoke = getRawInvoke();
    return rawInvoke ? rawInvoke("debug_log", args) : Promise.resolve();
  }
  const rawInvoke = getRawInvoke();
  if (!rawInvoke) {
    throw new Error("Tauri invoke not available (window.__TAURI__ missing)");
  }

  if (DEBUG_UI) {
    const start = performance.now();
    const argSummary = args
      ? Object.keys(args).reduce((acc, k) => {
          const v = args[k];
          acc[k] = Array.isArray(v) ? `[Array:${v.length}]`
                  : typeof v === "object" && v !== null ? `{${Object.keys(v).length} keys}`
                  : (typeof v === "string" && v.length > 30) ? `"${v.slice(0, 30)}..."` : v;
          return acc;
        }, {})
      : {};
    logCall("→IPC", `${cmd}`, argSummary);
    try {
      const result = await rawInvoke(cmd, args);
      const ms = (performance.now() - start).toFixed(1);
      const resultSummary = Array.isArray(result) ? `[Array:${result.length}]`
                          : typeof result === "object" && result !== null ? `{${Object.keys(result).length} keys}`
                          : String(result).slice(0, 60);
      logCall("←IPC", `${cmd} ✓ ${ms}ms`, resultSummary);
      return result;
    } catch (err) {
      const ms = (performance.now() - start).toFixed(1);
      logCall("✗IPC", `${cmd} FAILED after ${ms}ms — ${err?.message ?? err}`);
      throw err;
    }
  }

  return rawInvoke(cmd, args);
};
window.invoke = invoke;

// ── listen wrapper ──────────────────────────────────────
const listen = async function(eventName, handler) {
  if (DEBUG_UI) logCall("→SUB", `event="${eventName}"`);
  try {
    const rawListen = getRawListen();
    if (!rawListen) {
      if (DEBUG_UI) logCall("✗SUB", `event="${eventName}" subscribe skipped — window.__TAURI__.event missing`);
      return () => {};
    }
    const unlisten = await rawListen(eventName, (event) => {
      if (DEBUG_UI) {
        const payloadKeys = event?.payload && typeof event.payload === "object"
          ? Object.keys(event.payload).join(",") : typeof event?.payload;
        logCall("←EVT", `event="${eventName}"`, payloadKeys);
      }
      try {
        return handler(event);
      } catch (err) {
        if (DEBUG_UI) logCall("✗EVT", `event="${eventName}" handler threw — ${err?.message ?? err}`);
        throw err;
      }
    });
    if (DEBUG_UI) logCall("✓SUB", `event="${eventName}" registered (unlisten fn: ${typeof unlisten})`);
    return unlisten;
  } catch (err) {
    if (DEBUG_UI) logCall("✗SUB", `event="${eventName}" subscribe failed — ${err?.message ?? err}`);
    return () => {};
  }
};
window.listen = listen;

// Silent variant — no per-event payload log, only sub start/result.
const listenSilent = async function(eventName, handler) {
  if (DEBUG_UI) logCall("→SUB•", `event="${eventName}" (silent — handler will log)`);
  try {
    const rawListen = getRawListen();
    if (!rawListen) {
      if (DEBUG_UI) logCall("✗SUB", `event="${eventName}" subscribe skipped — window.__TAURI__.event missing`);
      return () => {};
    }
    const unlisten = await rawListen(eventName, (event) => {
      try {
        return handler(event);
      } catch (err) {
        if (DEBUG_UI) logCall("✗EVT", `event="${eventName}" handler threw — ${err?.message ?? err}`);
        throw err;
      }
    });
    if (DEBUG_UI) logCall("✓SUB", `event="${eventName}" registered (silent, unlisten fn: ${typeof unlisten})`);
    return unlisten;
  } catch (err) {
    if (DEBUG_UI) logCall("✗SUB", `event="${eventName}" subscribe failed — ${err?.message ?? err}`);
    return () => {};
  }
};

// ── GLOBAL ERROR TRAPS (see the guarded fallback above) ──────────────────
// The second pair of listeners that used to live here was removed in v1.1.0:
// it reported through the DEBUG_UI-gated logger, so in release builds an
// uncaught error never reached the Rust log at all, and it double-subscribed
// next to the pair above. The authoritative traps now live in boot_guard.js.

// ── STAGE-BASED DIAGNOSTIC LOGGER ──────────────────────────────
// Compact multi-step logger. Each run() records one stage line; pass=true
// logs as ✓, false/throw as ✗. The `detail` argument is auto-truncated.
//
// Usage:
//   const op = stage("Toggle");
//   await op.run("check-mode", () => currentConfig.active_mode !== "work");
//   await op.run("invoke", () => invoke("toggle_autoclicker"));
//   op.ok();
function stage(name) {
  const start = performance.now();
  let stepNum = 0;
  const log = (icon, label, detail = "") => {
    const elapsedMs = (performance.now() - start).toFixed(1);
    const safeDetail = String(detail || "").slice(0, 80);
    const tag = icon === "✓" ? "→STAGE✓" : icon === "✗" ? "→STAGE✗" : "→STAGE•";
    logCall(tag, `[${name}] #${++stepNum} ${label} (+${elapsedMs}ms)`, safeDetail);
  };
  return {
    // fn may return: undefined (no result), boolean (false=stage fails),
    // Promise (awaited), or throw (logged as failure).
    run: async (label, fn) => {
      try {
        const result = await fn();
        if (result === false) {
          log("✗", label, "returned false");
          throw new Error(`Stage[${name}] failed at: ${label}`);
        }
        log(result === undefined || result === true ? "✓" : "•", label,
            result === undefined || result === true ? "" : String(result));
        return result;
      } catch (err) {
        log("✗", label, err?.message ?? err);
        throw err;
      }
    },
    ok: (label = "complete") => log("✓", label, "all stages passed"),
    fail: (label, err) => log("✗", label, `ERROR: ${err?.message ?? err}`),
    log: (label, detail) => log("•", label, detail),
  };
}

// Helper to execute callback immediately if DOM is already parsed/interactive (common in ES modules),
// or subscribe to DOMContentLoaded if DOM is still loading.
function onDomReady(fn) {
  if (typeof document !== "undefined" && document.readyState !== "loading") {
    fn();
  } else if (typeof document !== "undefined") {
    document.addEventListener("DOMContentLoaded", fn);
  }
}
if (typeof window !== "undefined") window.onDomReady = onDomReady;

// Diagnostic banner: if Tauri globals are missing, dump the page state on DOMContentLoaded
onDomReady(() => {
  const currentTauri = (typeof window !== "undefined" && window.__TAURI__) || null;
  const currentInvoke = currentTauri?.core?.invoke || window.__TAURI_INTERNALS__?.invoke;
  if (!currentInvoke) {
    const err = document.createElement("div");
    err.style.cssText = "position:fixed;top:0;left:0;right:0;padding:24px;background:#7f1d1d;color:#fff;font:14px monospace;z-index:99999;white-space:pre-wrap";
    err.textContent = "NanoClick WebView error: window.__TAURI__ is " + (currentTauri ? "missing .core.invoke" : "undefined") + ".\n\nKeys present: " + (currentTauri ? Object.keys(currentTauri).join(", ") : "<none>");
    document.body.appendChild(err);
  }
});

// (invoke/listen wrappers are defined above with full logging)


// ===== Update checker =====
// Security: the Tauri updater plugin verifies the downloaded installer's
// minisign signature against the public key embedded in tauri.conf.json
// before installing. Downloads only happen from the configured endpoint
// (GitHub Releases of this repository). Nothing is installed without a
// valid signature.
const UPDATE_CHECK_INTERVAL_MS = 30 * 60 * 1000; // every 30 minutesurs
const UPDATE_DISMISS_KEY = "nanoclick_update_dismissed_version";

function showUpdateBar(version, notes) {
  const bar = document.getElementById("updateBar");
  if (!bar) return;
  document.getElementById("updateBarText").textContent =
    `NanoClick ${version} is available` + (notes ? ` — ${notes.slice(0, 80)}` : "");
  bar.classList.remove("hidden");
  const btn = document.getElementById("updateInstallBtn");
  btn.disabled = false;
  btn.textContent = "Download & install";
  // Re-binding on every update check is intentional: this is a REPLACEMENT slot,
  // so a handler from a previous version must not survive. addEventListener would
  // stack handlers and could fire several installs.
  // oxlint-disable-next-line unicorn/prefer-add-event-listener
  btn.onclick = async () => {
    btn.disabled = true;
    try {
      const update = await window.__TAURI__.updater.check();
      if (!update) { btn.textContent = "No update"; return; }
      // Signature is verified by the plugin before install completes.
      let total = 0, received = 0;
      await update.downloadAndInstall((event) => {
        if (event.event === "Started") {
          total = event.data.contentLength || 0;
          btn.textContent = "Downloading… 0%";
        } else if (event.event === "Progress") {
          received += event.data.chunkLength;
          if (total > 0) btn.textContent = `Downloading… ${Math.min(99, Math.round(received / total * 100))}%`;
        } else if (event.event === "Finished") {
          btn.textContent = "Installing…";
        }
      });
      // Only reached on success; restart into the new version.
      await invoke("relaunch_app");
    } catch (e) {
      console.error("[updater] install failed:", e);
      btn.textContent = "Failed — retry";
      btn.disabled = false;
    }
  };
  // Same replacement-slot reasoning as the install button above, plus this closure
  // captures `version`: with addEventListener + a one-time guard it would keep the
  // FIRST version and store a stale dismissal key.
  // oxlint-disable-next-line unicorn/prefer-add-event-listener
  document.getElementById("updateDismissBtn").onclick = () => {
    bar.classList.add("hidden");
    try { localStorage.setItem(UPDATE_DISMISS_KEY, version); } catch {}
  };
}

async function checkForAppUpdates(manual = false) {
  if (!TAURI) return;
  try {
    const info = await invoke("check_for_updates");
    if (info) {
      let dismissed = null;
      try { dismissed = localStorage.getItem(UPDATE_DISMISS_KEY); } catch {}
      console.log("[updater] latest:", info.version, "local:", "(see get_app_version)", "dismissed:", dismissed);
      if (manual || dismissed !== info.version) {
        showUpdateBar(info.version, info.body);
        if (manual) console.log("[updater] update", info.version, "available");
      } else {
        console.log("[updater] update", info.version, "available but previously dismissed by user");
      }
    } else if (manual) {
      console.log("[updater] up to date");
    } else {
      console.log("[updater] no update available");
    }
  } catch (e) {
    console.error("[updater] check failed:", e);
    if (manual) console.warn("[updater] check failed:", e);
  }
}

function startUpdateChecker() {
  if (!TAURI) return;
  console.log("[updater] starting checker; first check in 3s, then every", UPDATE_CHECK_INTERVAL_MS / 1000, "s");
  setTimeout(() => checkForAppUpdates(false), 3_000); // first check shortly after launch
  setInterval(() => checkForAppUpdates(false), UPDATE_CHECK_INTERVAL_MS);
}

let currentConfig = {
  first_run: true,
  active_mode: "autoclicker",
  engine: {
    target_cps: 29.0,
    jitter_percent: 7.5,
    click_limit: 0,
    jitter_radius_px: 0,
    button: "left",
    click_type: "single",
    position_mode: "cursor",
    fixed_x: 100,
    fixed_y: 100,
    repeat_mode: "unlimited",
    repeat_count: 0,
    hold_duration_ms: 500,
    hold_interval_ms: 1000,
    repeat_interval_ms: 1000,
    start_delay_ms: 0,
    stop_duration_min: 0,
    stop_time_str: "",
    gui_lock_ms: 1500,
    hotkey_debounce_ms: 80
  },
  hotkeys: {
    toggle: "R / K",
    mode_switch: "Ctrl+Alt+M",
    emergency_stop: "Escape",
    speed_up: "Ctrl+=",
    slow_down: "Ctrl+-",
    capture_pos: "Ctrl+P",
    record_hotkey: "Ctrl+Shift+R"
  },
  ui: {
    always_on_top: false,
    mode: "floating_hud",
    sound_feedback: false,
    visual_ripple: true
  }
};

let isRunning = false;
let isButtonLocked = false;
let guiLockTimer = null;

// DOM Elements — queried at module evaluation time.
// ES modules are always deferred: they execute AFTER the full HTML is parsed,
// so getElementById is safe here without any DOMContentLoaded wrapper.
const statusBadge     = document.getElementById("statusBadge");
const modeToggleBtn   = document.getElementById("modeToggleBtn");
const clickCounter    = document.getElementById("clickCounter");
const displayCps      = document.getElementById("displayCps");

// Click tooltip refs
const clickMetricInfoBtn  = document.getElementById("clickMetricInfoBtn");
const clickStatsTooltip   = document.getElementById("clickStatsTooltip");
const tooltipRunClicks      = document.getElementById("tooltipRunClicks");
const tooltipSessionClicks  = document.getElementById("tooltipSessionClicks");
const tooltipSettingsClicks = document.getElementById("tooltipSettingsClicks");
const tooltipSettingsRuns   = document.getElementById("tooltipSettingsRuns");
// Runtime counters: track runs and clicks with the current active config identity.
// Resets when engine settings are changed between runs.
let _runsThisSettings = 0;
let _clicksThisSettings = 0;
let _lastSettingsRunClicksDone = 0;
let _hasChangedSettings = false;
const toggleBtn       = document.getElementById("toggleBtn");
const toggleBtnText   = document.getElementById("toggleBtnText");

const cpsRange        = document.getElementById("cpsRange");
const cpsInput        = document.getElementById("cpsInput");
const randomRange     = document.getElementById("randomRange");
const randomInput     = document.getElementById("randomInput");
const limitInput      = document.getElementById("limitInput");
const limitRange      = document.getElementById("limitRange");
const clickTypeSelect = document.getElementById("clickTypeSelect");
const posXInput       = document.getElementById("posXInput");
const posYInput       = document.getElementById("posYInput");
const repeatCountInput    = document.getElementById("repeatCountInput");
const hotkeyRecordBtn     = document.getElementById("hotkeyRecordBtn");
const modeSwitchRecordBtn = document.getElementById("modeSwitchRecordBtn");
const recordMacroHotkeyBtn = document.getElementById("recordMacroHotkeyBtn");
const hotkeyRecordLabel   = document.getElementById("hotkeyRecordLabel");
const modeSwitchRecordLabel = document.getElementById("modeSwitchRecordLabel");
const recordMacroHotkeyLabel = document.getElementById("recordMacroHotkeyLabel");

/* configPathDisplay removed: full path now lives in the folder-button tooltip */
const guiLockDelayInput  = document.getElementById("guiLockDelayInput");
const jitterRadiusInput  = document.getElementById("jitterRadiusInput");
const rippleCheckbox     = document.getElementById("rippleCheckbox");
const hudCheckbox        = document.getElementById("hudCheckbox");
const footerModeShortcut = document.getElementById("footerModeShortcut");

// Modal — the START button inside it is re-queried on demand by the onboarding
// flow (see initOnboarding), so no module-level handle is kept for it.
const onboardingModal = document.getElementById("onboardingModal");

const limitBadge        = document.getElementById("limitBadge");
const holdSubSettings   = document.getElementById("holdSubSettings");
const holdDurationInput = document.getElementById("holdDurationInput");
const holdIntervalInput = document.getElementById("holdIntervalInput");
const repeatIntervalInput = document.getElementById("repeatIntervalInput");
const pickPosBtn        = document.getElementById("pickPosBtn");
const pickPosStatus     = document.getElementById("pickPosStatus");
const openConfigFolderBtn = document.getElementById("openConfigFolderBtn");

const startMinimizedCheckbox  = document.getElementById("startMinimizedCheckbox");
const autostartCheckbox       = document.getElementById("autostartCheckbox");
const minimizeToTrayCheckbox  = document.getElementById("minimizeToTrayCheckbox");
const notificationsCheckbox   = document.getElementById("notificationsCheckbox");
const pauseFocusLossCheckbox  = document.getElementById("pauseFocusLossCheckbox");
const rememberPosCheckbox     = document.getElementById("rememberPosCheckbox");
const alwaysOnTopCheckbox     = document.getElementById("alwaysOnTopCheckbox");

const speedUnitBtn            = document.getElementById("speedUnitBtn");
const speedUnitBadge          = document.getElementById("speedUnitBadge");
const speedUnitMenu           = document.getElementById("speedUnitMenu");
const delayUnitBtn            = document.getElementById("delayUnitBtn");
const delayUnitBadge          = document.getElementById("delayUnitBadge");
const delayUnitMenu           = document.getElementById("delayUnitMenu");
const delayUnitSuffix         = document.getElementById("delayUnitSuffix");
let currentSpeedUnit = "cps";
let currentDelayUnit = "sec";

function applyAlwaysOnTop(enable) {
  try {
    if (window.__TAURI__?.window?.getCurrentWindow) {
      window.__TAURI__.window.getCurrentWindow().setAlwaysOnTop(!!enable);
    }
  } catch (e) {
    console.warn("[always-on-top] failed to set:", e);
  }
}

function updateSpeedDisplay(targetCps) {
  const cps = Math.max(0.1, Math.min(160, Number(targetCps) || 29.0));
  if (displayCps) displayCps.textContent = cps.toFixed(1);

  if (currentSpeedUnit === "ms") {
    const msVal = Math.round(1000.0 / cps);
    if (cpsRange) {
      cpsRange.min = "5";
      cpsRange.max = "1000";
      cpsRange.step = "1";
      cpsRange.value = Math.min(1000, Math.max(5, msVal));
    }
    if (cpsInput) {
      cpsInput.min = "5";
      cpsInput.max = "10000";
      cpsInput.step = "1";
      cpsInput.value = msVal;
    }
  } else {
    if (cpsRange) {
      cpsRange.min = "0.1";
      cpsRange.max = "160";
      cpsRange.step = "0.5";
      cpsRange.value = cps;
    }
    if (cpsInput) {
      cpsInput.min = "0.1";
      cpsInput.max = "160";
      cpsInput.step = "0.5";
      cpsInput.value = cps;
    }
  }
}

function updateStartDelayDisplay(delayMs) {
  const ms = Math.max(0, Number(delayMs) || 0);
  const startDelayInput = document.getElementById("startDelayInput");
  if (!startDelayInput) return;

  if (currentDelayUnit === "ms") {
    startDelayInput.min = "0";
    startDelayInput.max = "300000";
    startDelayInput.step = "50";
    startDelayInput.value = ms;
  } else {
    startDelayInput.min = "0";
    startDelayInput.max = "300";
    startDelayInput.step = "1";
    startDelayInput.value = Math.round(ms / 1000);
  }
}

function setSpeedUnit(unit) {
  currentSpeedUnit = unit;
  if (speedUnitBadge) {
    speedUnitBadge.textContent = unit === "ms" ? getI18nText("unit_ms_short", {}, "ms") : "CPS";
  }
  if (speedUnitMenu) {
    speedUnitMenu.querySelectorAll(".unit-popover-item").forEach(btn => {
      btn.classList.toggle("active", btn.getAttribute("data-unit") === unit);
    });
    speedUnitMenu.classList.add("hidden");
  }
  updateSpeedDisplay(currentConfig?.engine?.target_cps || 29.0);
}

function setDelayUnit(unit) {
  currentDelayUnit = unit;
  const suffix = unit === "ms" ? getI18nText("unit_ms_short", {}, "ms") : (getI18nText("dash_unit_sec", {}, "sec"));
  if (delayUnitBadge) delayUnitBadge.textContent = suffix;
  if (delayUnitSuffix) delayUnitSuffix.textContent = suffix;
  if (delayUnitMenu) {
    delayUnitMenu.querySelectorAll(".unit-popover-item").forEach(btn => {
      btn.classList.toggle("active", btn.getAttribute("data-unit") === unit);
    });
    delayUnitMenu.classList.add("hidden");
  }
  updateStartDelayDisplay(currentConfig?.engine?.start_delay_ms || 0);
}

function wireUnitSelectors() {
  if (speedUnitBtn && speedUnitMenu) {
    speedUnitBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      speedUnitMenu.classList.toggle("hidden");
      if (delayUnitMenu) delayUnitMenu.classList.add("hidden");
    });
    speedUnitMenu.querySelectorAll(".unit-popover-item").forEach(item => {
      item.addEventListener("click", (e) => {
        e.stopPropagation();
        const unit = item.getAttribute("data-unit");
        if (unit) setSpeedUnit(unit);
      });
    });
  }

  if (delayUnitBtn && delayUnitMenu) {
    delayUnitBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      delayUnitMenu.classList.toggle("hidden");
      if (speedUnitMenu) speedUnitMenu.classList.add("hidden");
    });
    delayUnitMenu.querySelectorAll(".unit-popover-item").forEach(item => {
      item.addEventListener("click", (e) => {
        e.stopPropagation();
        const unit = item.getAttribute("data-unit");
        if (unit) setDelayUnit(unit);
      });
    });
  }

  document.addEventListener("click", () => {
    if (speedUnitMenu) speedUnitMenu.classList.add("hidden");
    if (delayUnitMenu) delayUnitMenu.classList.add("hidden");
  });
}
wireUnitSelectors();

const themeSelect    = document.getElementById("themeSelect");
const languageSelect = document.getElementById("languageSelect");
const accentSwatches = document.querySelectorAll("#accentSwatches .swatch");

const emergencyRecordBtn = document.getElementById("emergencyRecordBtn");
const speedUpRecordBtn   = document.getElementById("speedUpRecordBtn");
const slowDownRecordBtn  = document.getElementById("slowDownRecordBtn");
const pickPosRecordBtn   = document.getElementById("pickPosRecordBtn");

// ── DOM PRESENCE DIAGNOSTIC ──────────────────────────────────
// Logged at module-eval time (= after HTML is parsed, ES modules are deferred).
// If any key element shows null here, HTML IDs are out of sync with JS.
dbg("readyState at module eval:", document.readyState);
dbg("toggleBtn:",     toggleBtn     ? "✓" : "NULL — id=toggleBtn missing in HTML");
dbg("modeToggleBtn:", modeToggleBtn ? "✓" : "NULL — id=modeToggleBtn missing in HTML");
dbg("cpsRange:",      cpsRange      ? "✓" : "NULL — id=cpsRange missing in HTML");
dbg("limitRange:",    limitRange    ? "✓" : "NULL — id=limitRange missing in HTML");
dbg("nav-items:",     document.querySelectorAll(".nav-item").length, "found");

// ── SIDEBAR NAVIGATION (wired immediately — DOM is ready at module eval) ──
document.querySelectorAll(".nav-item").forEach(navBtn => {
  navBtn.addEventListener("click", async () => {
    const viewId = navBtn.getAttribute("data-view");
    dbg("Nav click — target viewId:", viewId);
    if (!viewId) return;

    // Auto-pause autoclicker when navigating away from Dashboard
    if (viewId !== "viewDashboard" && isRunning) {
      dbg("Navigating away from Dashboard while running — auto-pausing");
      try {
        const active = await invoke("toggle_autoclicker");
        setRunningState(active);
      } catch (err) {
        console.error("Auto-pause on navigation failed:", err);
      }
    }

    document.querySelectorAll(".nav-item").forEach(b => b.classList.remove("active"));
    navBtn.classList.add("active");
    document.querySelectorAll(".view").forEach(v => v.classList.remove("active"));
    const target = document.getElementById(viewId);
    if (target) {
      target.classList.add("active");
      if (viewId === "viewStats") renderStats();
      dbg("Nav success — view activated:", viewId);
    } else {
      dbg("Nav ERROR — target view element not found:", viewId);
    }
  });
});

// ── I18N DYNAMIC HELPER ─────────────────────────────────────────
function getI18nText(key, params = {}, fallback = "") {
  return (typeof window !== "undefined" && window.I18nEngine) ? window.I18nEngine.t(key, params, fallback) : (fallback || key);
}

function setAutomationButtonLabel(state, param) {
  if (!toggleBtnText) return;
  switch (state) {
    case "disabled_work":
      toggleBtnText.textContent = getI18nText("dash_btn_disabled_work", {}, "DISABLED IN WORK MODE");
      break;
    case "start":
      toggleBtnText.textContent = getI18nText("dash_start_btn", {}, "START AUTOMATION");
      break;
    case "stop":
      toggleBtnText.textContent = getI18nText("dash_stop_btn", {}, "STOP AUTOMATION");
      break;
    case "running_locked":
      toggleBtnText.textContent = getI18nText("dash_btn_running_locked", {}, "RUNNING (CLICK LOCKED)");
      break;
    case "starting_in":
      toggleBtnText.textContent = getI18nText("dash_btn_starting_in", { sec: param }, `STARTING IN ${param}s...`);
      break;
  }
}

// ── MODE DISPLAY ────────────────────────────────────────────
let _lastSetMode = null;
function setModeDisplay(mode, force = false) {
  currentConfig.active_mode = mode;
  if (mode === _lastSetMode && !force) return;
  _lastSetMode = mode;
  const switchKey = currentConfig.hotkeys ? currentConfig.hotkeys.mode_switch : "Ctrl+Alt+M";

  if (modeToggleBtn) {
    const modeIcon = modeToggleBtn.querySelector(".mode-icon");
    const modeText = modeToggleBtn.querySelector(".mode-text");
    const modeHint = modeToggleBtn.querySelector(".mode-shortcut-hint");

    if (mode === "autoclicker") {
      if (modeIcon) modeIcon.textContent = "⚡";
      if (modeText) {
        modeText.setAttribute("data-i18n", "mode_btn_autoclicker");
        modeText.textContent = getI18nText("mode_btn_autoclicker", {}, "AUTOCLICKER MODE");
      }
      if (modeHint) modeHint.textContent = `[ ${switchKey} ]`;
      modeToggleBtn.className = "prominent-mode-btn autoclicker";
      modeToggleBtn.title = getI18nText("mode_btn_title_to_work", { key: switchKey }, `Click or press ${switchKey} to switch to WORK mode`);
    } else {
      if (modeIcon) modeIcon.textContent = "⌨️";
      if (modeText) {
        modeText.setAttribute("data-i18n", "mode_btn_work");
        modeText.textContent = getI18nText("mode_btn_work", {}, "WORK MODE (PAUSED)");
      }
      if (modeHint) modeHint.textContent = `[ ${switchKey} ]`;
      modeToggleBtn.className = "prominent-mode-btn work";
      modeToggleBtn.title = getI18nText("mode_btn_title_to_auto", { key: switchKey }, `Click or press ${switchKey} to enable AUTOCLICKER mode`);
    }
  }

  // Lock/Unlock Start Button depending on mode
  if (toggleBtn) {
    const toggleBtnHint = toggleBtn.querySelector(".start-btn-hint");
    if (mode === "work") {
      toggleBtn.disabled = true;
      toggleBtn.classList.add("disabled-mode");
      setAutomationButtonLabel("disabled_work");
      if (toggleBtnHint) toggleBtnHint.textContent = getI18nText("dash_hint_switch_to_auto", {}, "Switch to Autoclicker Mode to Start");
    } else {
      toggleBtn.disabled = false;
      toggleBtn.classList.remove("disabled-mode");
      if (!isRunning) {
        setAutomationButtonLabel("start");
        if (toggleBtnHint) toggleBtnHint.textContent = currentConfig.hotkeys?.toggle || "R / K";
      }
    }
  }
}

// (All DOM elements initialized in initDomElements)

function updateLimitBadge(val) {
  if (!limitBadge) return;
  const num = parseInt(val, 10) || 0;
  if (num <= 0) {
    limitBadge.textContent = "∞";
    limitBadge.className = "config-suffix limit-badge-infinity";
    limitBadge.title = "0 = Unlimited";
  } else {
    limitBadge.textContent = num;
    limitBadge.className = "config-suffix limit-badge-infinity active-limit";
    limitBadge.title = `Limit: ${num} clicks`;
  }
}

function updateSubSettingsVisibility() {
  // Hold sub-settings
  const clickMode = getRadioChecked("clickMode", "single");
  if (holdSubSettings) {
    if (clickMode === "hold") {
      holdSubSettings.classList.remove("hidden");
    } else {
      holdSubSettings.classList.add("hidden");
    }
  }

  // Repeat sub-settings: show ONLY if repeatMode=="repeat" AND repeatCount > 0
  const repeatMode = getRadioChecked("repeatMode", "unlimited");
  const repeatCount = parseInt(repeatCountInput?.value, 10) || 0;
  const repeatSub = document.getElementById("repeatSubSettings");
  if (repeatSub) {
    if (repeatMode === "repeat" && repeatCount > 0) {
      repeatSub.classList.remove("hidden");
    } else {
      repeatSub.classList.add("hidden");
    }
  }

  // Visual ghosting / dimming for inactive radio options:
  // 1. Repeat Count input dimming when unlimited (opacity: 0.35 + disabled)
  if (repeatCountInput) {
    if (repeatMode === "unlimited") {
      repeatCountInput.classList.add("disabled-subgroup");
      repeatCountInput.setAttribute("disabled", "true");
    } else {
      repeatCountInput.classList.remove("disabled-subgroup");
      repeatCountInput.removeAttribute("disabled");
    }
  }

  // 2. Fixed coordinates row (X, Y and pick button) dimming when cursor mode (opacity: 0.35 + pointer-events: none)
  const posMode = getRadioChecked("posMode", "cursor");
  const xyRow = document.querySelector(".xy-row");
  if (xyRow) {
    if (posMode === "cursor") {
      xyRow.classList.add("disabled-subgroup");
      if (posXInput) posXInput.setAttribute("disabled", "true");
      if (posYInput) posYInput.setAttribute("disabled", "true");
      if (pickPosBtn) pickPosBtn.setAttribute("disabled", "true");
    } else {
      xyRow.classList.remove("disabled-subgroup");
      if (posXInput) posXInput.removeAttribute("disabled");
      if (posYInput) posYInput.removeAttribute("disabled");
      if (pickPosBtn) pickPosBtn.removeAttribute("disabled");
    }
  }
}

// ── CONFIG SYNC ─────────────────────────────────────────────
function updateUiFromConfig(config) {
  currentConfig = config;
  window.StatsEngine?.init(config);
  window.UipiManager?.init(config);
  setupClickTooltip(); // Wire ⓘ tooltip button (idempotent via internal flag)

  if (config.first_run) {
    showOnboardingMatrix();
  } else {
    onboardingModal?.classList.add("hidden");
  }

  // Force=true ensures button text + hint refresh even if mode hasn't changed (e.g. locale reload after first render)
  setModeDisplay(config.active_mode || "autoclicker", true);

  updateSpeedDisplay(config.engine.target_cps);

  if (randomRange) randomRange.value = config.engine.jitter_percent;
  if (randomInput) randomInput.value = config.engine.jitter_percent;

  const lim = config.engine.click_limit || 0;
  if (limitInput) limitInput.value = lim;
  if (limitRange) limitRange.value = lim;
  updateLimitBadge(lim);

  if (clickTypeSelect && config.engine.button) {
    clickTypeSelect.value = config.engine.button;
  }

  // Radio button sync
  setRadioChecked("clickMode", config.engine.click_type || "single");
  setRadioChecked("posMode", config.engine.position_mode || "cursor");
  setRadioChecked("repeatMode", config.engine.repeat_mode || "unlimited");

  if (posXInput) posXInput.value = config.engine.fixed_x ?? 100;
  if (posYInput) posYInput.value = config.engine.fixed_y ?? 100;
  if (repeatCountInput) repeatCountInput.value = config.engine.repeat_count || 0;
  if (holdDurationInput) holdDurationInput.value = config.engine.hold_duration_ms ?? 500;
  if (holdIntervalInput) holdIntervalInput.value = config.engine.hold_interval_ms ?? 1000;
  if (repeatIntervalInput) repeatIntervalInput.value = config.engine.repeat_interval_ms ?? 1000;

  const stopDurationInput = document.getElementById("stopDurationInput");
  const stopTimeInput = document.getElementById("stopTimeInput");

  // start_delay_ms is rendered by updateStartDelayDisplay(), which queries the
  // field itself and also switches its min/max/step per the active delay unit.
  updateStartDelayDisplay(config.engine.start_delay_ms || 0);
  if (stopDurationInput) stopDurationInput.value = config.engine.stop_duration_min || 0;
  if (stopTimeInput) stopTimeInput.value = config.engine.stop_time_str || "";

  // Call AFTER repeatCountInput is set so visibility logic reads correct value
  updateSubSettingsVisibility();

  if (guiLockDelayInput) guiLockDelayInput.value = config.engine.gui_lock_ms || 1500;
  applyDebounceFromConfig(config.engine.hotkey_debounce_ms || 80);

  if (config.hotkeys) {
    if (hotkeyRecordLabel) hotkeyRecordLabel.textContent = config.hotkeys.toggle || "R / K";
    if (modeSwitchRecordLabel) {
      modeSwitchRecordLabel.textContent = config.hotkeys.mode_switch || "Ctrl+Alt+M";
      if (footerModeShortcut) footerModeShortcut.textContent = config.hotkeys.mode_switch;
    }
    const emergencyLabel = document.getElementById("emergencyRecordLabel");
    if (emergencyLabel) emergencyLabel.textContent = config.hotkeys.emergency_stop || "Escape";
    const speedUpLabel = document.getElementById("speedUpRecordLabel");
    if (speedUpLabel) speedUpLabel.textContent = config.hotkeys.speed_up || "Ctrl+=";
    const slowDownLabel = document.getElementById("slowDownRecordLabel");
    if (slowDownLabel) slowDownLabel.textContent = config.hotkeys.slow_down || "Ctrl+-";
    const pickPosLabel = document.getElementById("pickPosRecordLabel");
    if (pickPosLabel) pickPosLabel.textContent = config.hotkeys.capture_pos || "Ctrl+P";
    if (recordMacroHotkeyLabel) recordMacroHotkeyLabel.textContent = config.hotkeys.record_hotkey || "Ctrl+Shift+R";
    const smartRecordCb = document.getElementById("smartRecordCheckbox");
    if (smartRecordCb) smartRecordCb.checked = config.hotkeys.smart_record !== false;
    const keyTtlInp = document.getElementById("keyTtlInput");
    if (keyTtlInp) keyTtlInp.value = config.hotkeys.key_ttl_ms ?? 500;
  }

  if (jitterRadiusInput) jitterRadiusInput.value = config.engine.jitter_radius_px ?? 0;
  const jitterRadiusRangeEl = document.getElementById("jitterRadiusRange");
  if (jitterRadiusRangeEl) jitterRadiusRangeEl.value = config.engine.jitter_radius_px ?? 0;
  if (rippleCheckbox) rippleCheckbox.checked = config.ui.visual_ripple;
  if (hudCheckbox) {
    hudCheckbox.checked = !!config.ui.show_hud;
    if (config.ui?.show_hud && typeof invoke === "function") {
      setTimeout(() => {
        invoke("toggle_hud_window", { show: true }).catch(() => {});
      }, 150);
    }
  }

  if (alwaysOnTopCheckbox) {
    alwaysOnTopCheckbox.checked = !!config.ui?.always_on_top;
    applyAlwaysOnTop(config.ui?.always_on_top);
  }
  if (startMinimizedCheckbox) startMinimizedCheckbox.checked = !!config.ui.start_minimized;
  if (autostartCheckbox) autostartCheckbox.checked = !!config.ui.autostart;
  if (minimizeToTrayCheckbox) minimizeToTrayCheckbox.checked = config.ui.minimize_to_tray !== false;
  if (notificationsCheckbox) notificationsCheckbox.checked = config.ui.show_notifications !== false;
  if (pauseFocusLossCheckbox) pauseFocusLossCheckbox.checked = !!config.ui.pause_on_focus_loss;
  if (rememberPosCheckbox) rememberPosCheckbox.checked = config.ui.remember_window_position !== false;
  if (window.SmartGuard) window.SmartGuard.hydrate(config);

  if (themeSelect && config.ui.theme) {
    themeSelect.value = config.ui.theme;
  }
  const targetLang = config.ui?.language || "ua";
  if (languageSelect) {
    languageSelect.value = targetLang;
  }
  if (window.I18nEngine) {
    window.I18nEngine.init(targetLang).then(() => {
      setModeDisplay(currentConfig.active_mode || "autoclicker", true);
      updateSubSettingsVisibility();
      if (window.UipiManager?.updateHeaderBadge) window.UipiManager.updateHeaderBadge();
    });
  }
  applyTheme(config.ui.theme || "cyberpunk", config.ui.accent_color || "#06b6d4");
  updateSwatchActiveState(config.ui.accent_color || "#06b6d4");

  renderPresetsGrid();
}

function setRadioChecked(name, val) {
  document.querySelectorAll(`input[name="${name}"]`).forEach(r => {
    r.checked = (r.value === val);
    const parentLabel = r.closest(".radio-item");
    if (parentLabel) {
      if (r.checked) parentLabel.classList.add("selected");
      else parentLabel.classList.remove("selected");
    }
  });
}

function getRadioChecked(name, defaultVal) {
  const checked = document.querySelector(`input[name="${name}"]:checked`);
  return checked ? checked.value : defaultVal;
}

// ===== Platform capabilities (v4.2) =====
// The backend reports what the current OS actually supports. On stub
// platforms (Linux/macOS builds) input features are unavailable — we show
// a persistent warning bar instead of letting clicks silently do nothing.
function showCapabilityBar(message) {
  let bar = document.getElementById("capabilityBar");
  if (!bar) {
    bar = document.createElement("div");
    bar.id = "capabilityBar";
    bar.style.cssText =
      "position:fixed;bottom:56px;left:0;right:0;z-index:9999;" +
      "padding:10px 16px;background:#7f1d1d;color:#fff;" +
      "font:13px system-ui,sans-serif;display:flex;align-items:center;" +
      "justify-content:center;gap:12px";
    const text = document.createElement("span");
    text.id = "capabilityBarText";
    const close = document.createElement("button");
    close.textContent = "\u2715";
    close.style.cssText =
      "background:none;border:none;color:#fff;font-size:14px;cursor:pointer;padding:2px 6px";
    close.addEventListener("click", () => bar.remove());
    bar.appendChild(text);
    bar.appendChild(close);
    document.body.appendChild(bar);
  }
  document.getElementById("capabilityBarText").textContent = message;
}

// ===== Version display sync =====
// The header/about version labels always mirror the real backend version
// (Cargo.toml / tauri.conf.json). No more hardcoded "vX.Y" in the HTML.
async function syncVersionDisplay() {
  try {
    const v = await invoke("get_app_version");
    // Show the FULL version string (e.g. "v1.0.0-beta").
    const display = "v" + String(v);
    document.querySelectorAll(".ver").forEach((el) => {
      el.textContent = display;
    });
    // Click on version badge = manual update check (idempotent wiring).
    document.querySelectorAll(".ver").forEach((el) => {
      el.style.cursor = "pointer";
      el.title = "Click to check for updates";
      if (!el._updateHandlerWired) {
        el.addEventListener("click", () => {
          console.log("[updater] manual check via version badge");
          checkForAppUpdates(true);
        });
        el._updateHandlerWired = true;
      }
    });
    document.title = "NanoClick " + display;
    console.log("[NanoClick] UI version synced to", v);
    wireAboutTabActions();
  } catch (err) {
    console.warn("[NanoClick] version sync failed:", err);
  }
}

// ===== About Tab Actions (GitHub, Updates & License) =====
function wireAboutTabActions() {
  const ghBtn = document.getElementById("aboutGithubBtn");
  if (ghBtn && !ghBtn._wired) {
    ghBtn._wired = true;
    ghBtn.addEventListener("click", async () => {
      try {
        if (window.__TAURI__?.core?.invoke) {
          await window.__TAURI__.core.invoke("open_external_url", { url: "https://github.com/Ar3ss12/nanoclick" });
        }
      } catch (e) {
        console.warn("[About] failed to open GitHub URL:", e);
      }
    });
  }

  const licenseBtn = document.getElementById("aboutLicenseBtn");
  if (licenseBtn && !licenseBtn._wired) {
    licenseBtn._wired = true;
    licenseBtn.addEventListener("click", async () => {
      // Full text lives in LICENSE.md (repo root + bundled release).
      // Open the canonical copy on GitHub; fall back to an in-app notice offline.
      try {
        if (window.__TAURI__?.core?.invoke) {
          await window.__TAURI__.core.invoke("open_external_url", { url: "https://github.com/Ar3ss12/nanoclick/blob/main/LICENSE.md" });
          return;
        }
      } catch (e) {
        console.warn("[About] failed to open license URL:", e);
      }
      try {
        window.open("https://github.com/Ar3ss12/nanoclick/blob/main/LICENSE.md", "_blank", "noopener");
      } catch {}
    });
  }

  const updateBtn = document.getElementById("aboutCheckUpdateBtn");
  const updateBtnText = document.getElementById("aboutUpdateBtnText");
  if (updateBtn && !updateBtn._wired) {
    updateBtn._wired = true;
    updateBtn.addEventListener("click", async () => {
      const origKey = "about_btn_update";
      const origText = updateBtnText ? updateBtnText.textContent : "Check for Updates";
      if (updateBtnText) {
        updateBtnText.textContent = getI18nText("about_checking_update", {}, "Checking...");
      }
      updateBtn.disabled = true;
      try {
        await checkForAppUpdates(true);
      } finally {
        setTimeout(() => {
          if (updateBtnText) {
            updateBtnText.textContent = getI18nText(origKey, {}, origText);
          }
          updateBtn.disabled = false;
        }, 1200);
      }
    });
  }

  const resetBtn = document.getElementById("aboutResetConfigBtn");
  const resetModal = document.getElementById("resetConfirmModal");
  const resetCancelXBtn = document.getElementById("resetCancelXBtn");
  const resetCancelBtn = document.getElementById("resetCancelBtn");
  const resetConfirmBtn = document.getElementById("resetConfirmBtn");
  const resetConfirmTimerText = document.getElementById("resetConfirmTimerText");

  let resetTimer = null;
  let countdownSeconds = 5;

  function closeResetModal() {
    if (resetTimer) { clearInterval(resetTimer); resetTimer = null; }
    if (resetModal) resetModal.classList.add("hidden");
    if (resetConfirmBtn) resetConfirmBtn.disabled = true;
  }

  function openResetModal() {
    if (!resetModal) return;
    resetModal.classList.remove("hidden");
    countdownSeconds = 5;
    if (resetConfirmBtn) {
      resetConfirmBtn.disabled = true;
      const thinkText = getI18nText("reset_modal_think", { n: countdownSeconds }, `Подумай (${countdownSeconds})`);
      if (resetConfirmTimerText) resetConfirmTimerText.textContent = thinkText;
      else resetConfirmBtn.textContent = thinkText;
    }
    if (resetTimer) clearInterval(resetTimer);
    resetTimer = setInterval(() => {
      countdownSeconds--;
      if (countdownSeconds > 0) {
        const thinkText = getI18nText("reset_modal_think", { n: countdownSeconds }, `Подумай (${countdownSeconds})`);
        if (resetConfirmTimerText) resetConfirmTimerText.textContent = thinkText;
        else resetConfirmBtn.textContent = thinkText;
      } else {
        clearInterval(resetTimer);
        resetTimer = null;
        if (resetConfirmBtn) {
          resetConfirmBtn.disabled = false;
          const confirmText = getI18nText("reset_modal_confirm", {}, "Підтвердити скидання");
          if (resetConfirmTimerText) resetConfirmTimerText.textContent = confirmText;
          else resetConfirmBtn.textContent = confirmText;
        }
      }
    }, 1000);
  }

  if (resetBtn && !resetBtn._wired) {
    resetBtn._wired = true;
    resetBtn.addEventListener("click", openResetModal);
  }
  if (resetCancelXBtn && !resetCancelXBtn._wired) {
    resetCancelXBtn._wired = true;
    resetCancelXBtn.addEventListener("click", closeResetModal);
  }
  if (resetCancelBtn && !resetCancelBtn._wired) {
    resetCancelBtn._wired = true;
    resetCancelBtn.addEventListener("click", closeResetModal);
  }
  if (resetModal && !resetModal._wiredBackdrop) {
    resetModal._wiredBackdrop = true;
    resetModal.addEventListener("click", (e) => {
      if (e.target === resetModal) closeResetModal();
    });
  }
  if (resetConfirmBtn && !resetConfirmBtn._wired) {
    resetConfirmBtn._wired = true;
    resetConfirmBtn.addEventListener("click", async () => {
      if (resetConfirmBtn.disabled) return;
      closeResetModal();
      try {
        const freshConfig = await invoke("reset_config_to_defaults");
        if (freshConfig) {
          updateUiFromConfig(freshConfig);
          if (typeof showToast === "function") {
            showToast(getI18nText("reset_success_notify", {}, "Налаштування успішно скинуто! Бекап збережено в config.json.bak"), "success");
          }
        }
      } catch (err) {
        console.error("Failed to reset config:", err);
        if (typeof showToast === "function") {
          showToast("Failed to reset config: " + err, "error");
        }
      }
    });
  }
}
wireAboutTabActions();

async function checkPlatformCapabilities() {
  try {
    const caps = await invoke("get_platform_capabilities");
    console.log("[NanoClick] platform capabilities:", caps);
    window.__nanoclickCaps = caps;
    if (!caps.can_play_macros) {
      showCapabilityBar(
        "Macro playback is unavailable on this platform (no mouse/keyboard injection)."
      );
      const btn = document.getElementById("toggleBtn");
      if (btn) {
        btn.disabled = true;
        btn.title = "Input injection unavailable on this platform";
      }
    } else {
      if (!caps.global_hotkeys) {
        showCapabilityBar(
          "Global hotkeys are unavailable on this platform. Use the in-app buttons instead."
        );
      } else if (!caps.global_input_recording) {
        showCapabilityBar(
          "Macro recording is unavailable on this platform."
        );
      }
    }
  } catch (err) {
    // Older backends may not expose the command — non-fatal.
    console.warn("[nanoclick] capabilities check failed:", err);
  }
}

let _configPath = "";
function updateConfigFolderTooltip() {
  if (typeof openConfigFolderBtn === "undefined" || !openConfigFolderBtn) return;
  const base = getI18nText("settings_open_folder_title", {}, "Open Config Folder in Explorer");
  openConfigFolderBtn.title = _configPath ? `${base} — ${_configPath}` : base;
}

async function loadConfig() {
  try {
    const config = await invoke("get_app_config");
    updateUiFromConfig(config);
    const path = await invoke("get_config_path");
    _configPath = path || "";
    updateConfigFolderTooltip();
    // Deadbolt Modal queue: every boot notice needs its own OK click.
    // No backdrop/Esc/[X] dismiss — the app stays bolted until drained.
    // i18n dict may load a beat later: retry resolve is inside the modal,
    // and toasts start polling in parallel (non-blocking).
    try {
      const notices = await invoke("get_startup_notices");
      if (Array.isArray(notices) && notices.length) showDeadboltQueue(notices);
    } catch { /* notices are best-effort, never block boot */ }
    try { startFileToastPolling(); } catch (_) {}
  } catch (err) {
    console.error("Failed to load app config:", err);
  }
}

// ── Deadbolt Modal: mandatory-acknowledge queue (vanilla JS, no libs) ─
// Rules: no [X] in markup, backdrop clicks swallowed via stopPropagation,
// Esc killed at window level while bolted, ONLY exit is the OK button.
// Multiple notices chain strictly in order until the queue is empty.
let _deadboltEscGuard = null;
function showDeadboltQueue(notices) {
  const queue = Array.isArray(notices) ? notices.slice() : [];
  if (!queue.length) return;
  // Kill Esc while any deadbolt is on screen.
  if (!_deadboltEscGuard) {
    _deadboltEscGuard = (e) => {
      if ((e.key === "Escape" || e.key === "Esc") && document.getElementById("deadboltModal")) {
        e.preventDefault();
        e.stopPropagation();
      }
    };
    window.addEventListener("keydown", _deadboltEscGuard, true);
  }
  const showNext = () => {
    const notice = queue.shift();
    if (!notice) {
      const guard = _deadboltEscGuard;
      _deadboltEscGuard = null;
      if (guard) window.removeEventListener("keydown", guard, true);
      return;
    }
    showDeadboltModal(notice, showNext);
  };
  showNext();
}

function resolveNoticeText(key, fallback) {
  // Keys arrive as "key" or "key|param" (count / file name after the pipe).
  if (typeof key !== "string" || !key) return fallback || "";
  const pipe = key.indexOf("|");
  const base = pipe >= 0 ? key.slice(0, pipe) : key;
  const param = pipe >= 0 ? key.slice(pipe + 1) : "";
  const dict = (typeof window !== "undefined" && window.I18nEngine && window.I18nEngine.dict) || null;
  let text = (dict && dict[base]) || fallback || base;
  // {n} for counts (repair details), {file} for watcher verdicts.
  if (param) {
    text = String(text).split("{n}").join(param).split("{file}").join(param);
  }
  return text;
}

function showDeadboltModal(notice, onOk) {
  const prev = document.getElementById("deadboltModal");
  if (prev) prev.remove();
  const level = String(notice.level || "info").toLowerCase();
  const accent = level === "critical" ? "#ef4444" : level === "warning" ? "#f59e0b" : "#06b6d4";
  const overlay = document.createElement("div");
  overlay.id = "deadboltModal";
  overlay.className = "modal-overlay";
  overlay.style.display = "flex";
  // Backdrop clicks die here: stopPropagation, no dismiss.
  overlay.addEventListener("click", (e) => e.stopPropagation(), true);
  overlay.addEventListener("mousedown", (e) => e.stopPropagation(), true);
  let detailsHtml = "";
  if (notice.details) {
    detailsHtml = '<div class="modal-body"><code style="word-break:break-all;font-size:12px;opacity:.85">' +
      escapeHtml(String(notice.details)) + "</code></div>";
  }
  const okLabel = resolveNoticeText("deadbolt_ok", "OK");
  overlay.innerHTML =
    '<div class="modal-card" role="alertdialog" aria-modal="true" aria-labelledby="deadboltTitle" ' +
    'style="border-top:3px solid ' + accent + '">' +
    '<h3 id="deadboltTitle">' + escapeHtml(resolveNoticeText(notice.title, "Configuration notice")) + "</h3>" +
    '<div class="modal-body"><p style="line-height:1.5">' + escapeHtml(resolveNoticeText(notice.message || notice.body, "")) + "</p></div>" +
    detailsHtml +
    '<div class="modal-footer"><button id="deadboltOk" class="btn primary">' + escapeHtml(okLabel) + "</button></div>" +
    "</div>";
  document.body.appendChild(overlay);
  const ok = document.getElementById("deadboltOk");
  // No [X] in markup on purpose. No Esc. No backdrop close. Only OK.
  if (ok) {
    ok.focus();
    ok.addEventListener("click", () => { overlay.remove(); if (typeof onOk === "function") onOk(); });
  }
}

// ── Runtime file-health toasts (observer watcher, non-blocking) ────
// Poll every 3s; auto-dismiss after 6s. NEVER bolt the UI: observer only.
function showFileToast(notice) {
  const host = document.getElementById("toastHost") || document.body;
  const el = document.createElement("div");
  el.className = "toast toast-file-health toast-" + String(notice.level || "info").toLowerCase();
  const title = resolveNoticeText(notice.title, "File changed");
  const msg = resolveNoticeText(notice.message, "");
  el.innerHTML = "<strong>" + escapeHtml(title) + "</strong><span>" + escapeHtml(msg) + "</span>";
  el.style.cssText = "pointer-events:auto;margin-top:8px;padding:10px 14px;border-radius:8px;" +
    "background:rgba(20,24,32,.95);border:1px solid rgba(255,255,255,.12);" +
    "font-size:13px;line-height:1.4;max-width:340px;box-shadow:0 8px 24px rgba(0,0,0,.45)";
  host.appendChild(el);
  setTimeout(() => { if (el.parentNode) el.parentNode.removeChild(el); }, 6000);
}

// ── Generic UI toasts (message, level) ─────────────────────────────
// Found by oxlint `no-undef` (2026-09-21): 13 call sites across the UI called
// `showToast(message, level)` while the function did not exist anywhere in the
// frontend. 10 of them were guarded with `typeof showToast === "function"`
// (i.e. silent no-ops — a knowingly tolerated gap), and 3 called it directly,
// throwing `ReferenceError` inside the flow (focus-loss pause notice, "template
// added", macro-list refresh). `showFileToast` above is the watcher-flavoured
// counterpart; this restores the local notifications with the same look.
function ensureToastHost() {
  let host = document.getElementById("toastHost");
  if (host) return host;
  host = document.createElement("div");
  host.id = "toastHost";
  // Fixed overlay so toasts never shift the layout; clicks pass through.
  host.style.cssText = "position:fixed;right:16px;bottom:16px;z-index:9999;" +
    "display:flex;flex-direction:column;align-items:flex-end;pointer-events:none;";
  (document.body || document.documentElement).appendChild(host);
  return host;
}

function showToast(message, level) {
  try {
    const host = ensureToastHost();
    if (!host) return;
    const el = document.createElement("div");
    el.className = "toast toast-" + String(level || "info").toLowerCase();
    el.textContent = String(message == null ? "" : message);
    el.style.cssText = "pointer-events:auto;margin-top:8px;padding:10px 14px;border-radius:8px;" +
      "background:rgba(20,24,32,.95);border:1px solid rgba(255,255,255,.12);" +
      "font-size:13px;line-height:1.4;max-width:340px;box-shadow:0 8px 24px rgba(0,0,0,.45)";
    host.appendChild(el);
    setTimeout(() => { if (el.parentNode) el.parentNode.removeChild(el); }, 6000);
  } catch (_) {
    /* toasts are best-effort — never break the calling flow */
  }
}

// Module scope on purpose: this closes over nothing but module-level helpers, so
// building it inside startFileToastPolling() only allocated a fresh closure each
// call (oxlint: unicorn/consistent-function-scoping). The literal call text
// `everyVisible(3000, tick)` below is pinned by a Rust wiring test — keep it.
const tick = async () => {
  try {
    const inv = getRawInvoke();
    const toasts = inv ? await inv("poll_file_toasts") : [];
    if (Array.isArray(toasts)) {
      for (const t of toasts) showFileToast(t);
    }
  } catch (_) { /* watcher toasts are best-effort */ }
};

function startFileToastPolling() {
  if (startFileToastPolling._started) return;
  startFileToastPolling._started = true;
  everyVisible(3000, tick);
}

function escapeHtml(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({
    "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
  }[c]));
}

// Helper: safely parse int preserving 0 as a valid value
function safeInt(val, fallback) {
  const n = parseInt(val, 10);
  return isNaN(n) ? fallback : n;
}

// ── saveConfigThrottled ──────────────────────────────────────
// Slider "input" events fire dozens of times per second while dragging.
// Persisting on every tick means dozens of disk writes per second.
// This wrapper coalesces bursts: at most one real saveConfig() per 250ms,
// with a trailing call so the FINAL value always lands on disk.
let _saveCfgLast = 0;
let _saveCfgTimer = null;
function saveConfigThrottled() {
  const now = Date.now();
  if (now - _saveCfgLast >= 250) {
    _saveCfgLast = now;
    saveConfig();
    return;
  }
  if (_saveCfgTimer) clearTimeout(_saveCfgTimer);
  _saveCfgTimer = setTimeout(() => {
    _saveCfgTimer = null;
    _saveCfgLast = Date.now();
    saveConfig();
  }, 250);
}

async function saveConfig() {
  const op = stage("SaveConfig");
  try {
    await op.run("collect-input", () => {
      if (currentSpeedUnit === "ms") {
        const msVal = Math.max(5, parseFloat(cpsInput?.value) || 34.0);
        currentConfig.engine.target_cps = Math.max(0.1, Math.min(160, 1000.0 / msVal));
      } else {
        currentConfig.engine.target_cps = Math.max(0.1, Math.min(160, parseFloat(cpsInput?.value) || 29.0));
      }
      currentConfig.engine.jitter_percent = parseFloat(randomInput?.value) || 0.0;
      currentConfig.engine.click_limit = safeInt(limitInput?.value, 0);
      currentConfig.engine.gui_lock_ms = safeInt(guiLockDelayInput?.value, 1500) || 1500;
      currentConfig.engine.hotkey_debounce_ms = safeInt(document.getElementById("debounceSlider")?.value, 80);
      currentConfig.engine.jitter_radius_px = safeInt(jitterRadiusInput?.value, 0);

      const startDelayInput = document.getElementById("startDelayInput");
      const stopDurationInput = document.getElementById("stopDurationInput");
      const stopTimeInput = document.getElementById("stopTimeInput");

      if (startDelayInput) {
        const dVal = Math.max(0, safeInt(startDelayInput.value, 0));
        currentConfig.engine.start_delay_ms = currentDelayUnit === "ms" ? dVal : dVal * 1000;
      }
      if (stopDurationInput) currentConfig.engine.stop_duration_min = safeInt(stopDurationInput.value, 0);
      if (stopTimeInput) currentConfig.engine.stop_time_str = stopTimeInput.value || "";

      if (clickTypeSelect) currentConfig.engine.button = clickTypeSelect.value;
      currentConfig.engine.click_type = getRadioChecked("clickMode", "single");
      currentConfig.engine.position_mode = getRadioChecked("posMode", "cursor");
      currentConfig.engine.repeat_mode = getRadioChecked("repeatMode", "unlimited");
      if (posXInput) currentConfig.engine.fixed_x = safeInt(posXInput.value, 100) || 100;
      if (posYInput) currentConfig.engine.fixed_y = safeInt(posYInput.value, 100) || 100;
      if (repeatCountInput) currentConfig.engine.repeat_count = safeInt(repeatCountInput.value, 0);
      // Hold/repeat fields: 0 IS a valid value (no pause) — must NOT use ||
      if (holdDurationInput) currentConfig.engine.hold_duration_ms = Math.max(10, safeInt(holdDurationInput.value, 10));
      if (holdIntervalInput) currentConfig.engine.hold_interval_ms = Math.max(0, safeInt(holdIntervalInput.value, 0));
      if (repeatIntervalInput) currentConfig.engine.repeat_interval_ms = Math.max(0, safeInt(repeatIntervalInput.value, 0));

      if (!currentConfig.hotkeys) {
        currentConfig.hotkeys = { toggle: "R / K", mode_switch: "Ctrl+Alt+M", emergency_stop: "Escape", speed_up: "Ctrl+=", slow_down: "Ctrl+-", capture_pos: "Ctrl+P", record_hotkey: "Ctrl+Shift+R", key_ttl_ms: 500, smart_record: true };
      }
      const smartRecordCb = document.getElementById("smartRecordCheckbox");
      if (smartRecordCb) currentConfig.hotkeys.smart_record = smartRecordCb.checked;
      const keyTtlInp = document.getElementById("keyTtlInput");
      if (keyTtlInp) currentConfig.hotkeys.key_ttl_ms = Math.max(100, Math.min(5000, safeInt(keyTtlInp.value, 500)));
      if (rippleCheckbox) currentConfig.ui.visual_ripple = rippleCheckbox.checked;
      if (hudCheckbox) currentConfig.ui.show_hud = hudCheckbox.checked;

      if (!currentConfig.ui) currentConfig.ui = {};
      if (alwaysOnTopCheckbox) currentConfig.ui.always_on_top = alwaysOnTopCheckbox.checked;
      if (startMinimizedCheckbox) currentConfig.ui.start_minimized = startMinimizedCheckbox.checked;
      if (autostartCheckbox) currentConfig.ui.autostart = autostartCheckbox.checked;
      if (minimizeToTrayCheckbox) currentConfig.ui.minimize_to_tray = minimizeToTrayCheckbox.checked;
      if (notificationsCheckbox) currentConfig.ui.show_notifications = notificationsCheckbox.checked;
      if (pauseFocusLossCheckbox) currentConfig.ui.pause_on_focus_loss = pauseFocusLossCheckbox.checked;
      if (rememberPosCheckbox) currentConfig.ui.remember_window_position = rememberPosCheckbox.checked;
      if (window.SmartGuard) window.SmartGuard.collect(currentConfig);
      if (themeSelect) currentConfig.ui.theme = themeSelect.value;
      if (languageSelect) currentConfig.ui.language = languageSelect.value;
      const alwaysAdminCb = document.getElementById("alwaysRunAsAdminCheckbox");
      if (alwaysAdminCb) currentConfig.ui.always_run_as_admin = alwaysAdminCb.checked;

      if (displayCps) displayCps.textContent = currentConfig.engine.target_cps.toFixed(1);
      return `cps=${currentConfig.engine.target_cps} jitter=${currentConfig.engine.jitter_percent}%`;
    });

    await op.run("write-to-disk", () => invoke("save_app_config", { config: currentConfig }));
    op.ok();
  } catch (err) {
    op.fail("save-config", err);
    console.error("[SaveConfig] aborted:", err);
  }
}

if (limitRange) limitRange.addEventListener("input", (e) => {
  // Parse the raw slider value — step=1 means it can hit exactly 0
  const val = parseInt(e.target.value, 10);
  const safeVal = isNaN(val) ? 0 : val;
  if (limitInput) limitInput.value = safeVal;
  updateLimitBadge(safeVal);
  saveConfigThrottled();
});

// ── CPS slider + number input (Dashboard) ────────────────────
// Mirrors the slider into the number input and vice versa, updates the
// live counter, then persists via saveConfig(). Mirrors limitRange pattern.
if (cpsRange) cpsRange.addEventListener("input", (e) => {
  const val = parseFloat(e.target.value);
  if (currentSpeedUnit === "ms") {
    const safeMs = isNaN(val) ? 34 : Math.max(5, Math.min(1000, val));
    if (cpsInput) cpsInput.value = safeMs;
    const targetCps = Math.max(0.1, Math.min(160, 1000.0 / safeMs));
    if (displayCps) displayCps.textContent = targetCps.toFixed(1);
    if (currentConfig?.engine) currentConfig.engine.target_cps = targetCps;
  } else {
    const safeVal = isNaN(val) ? 29 : Math.max(0.1, Math.min(160, val));
    if (cpsInput) cpsInput.value = safeVal;
    if (displayCps) displayCps.textContent = safeVal.toFixed(1);
    if (currentConfig?.engine) currentConfig.engine.target_cps = safeVal;
  }
  saveConfigThrottled();
});
if (cpsInput) {
  cpsInput.addEventListener("input", (e) => {
    let val = parseFloat(e.target.value);
    if (currentSpeedUnit === "ms") {
      if (!isNaN(val) && val > 10000) { val = 10000; e.target.value = 10000; }
      const safeMs = isNaN(val) ? 34 : Math.max(5, val);
      if (cpsRange) cpsRange.value = Math.min(1000, safeMs);
      const targetCps = Math.max(0.1, Math.min(160, 1000.0 / safeMs));
      if (displayCps) displayCps.textContent = targetCps.toFixed(1);
      if (currentConfig?.engine) currentConfig.engine.target_cps = targetCps;
    } else {
      if (!isNaN(val) && val > 160) {
        val = 160;
        e.target.value = 160;
      }
      const safeVal = isNaN(val) ? 29 : Math.max(0.1, Math.min(160, val));
      if (cpsRange) cpsRange.value = safeVal;
      if (displayCps) displayCps.textContent = safeVal.toFixed(1);
      if (currentConfig?.engine) currentConfig.engine.target_cps = safeVal;
    }
    saveConfigThrottled();
  });
  cpsInput.addEventListener("change", (e) => {
    const val = parseFloat(e.target.value);
    if (currentSpeedUnit === "ms") {
      const safeMs = isNaN(val) ? 34 : Math.max(5, Math.min(10000, val));
      e.target.value = safeMs;
      if (cpsRange) cpsRange.value = Math.min(1000, safeMs);
      const targetCps = Math.max(0.1, Math.min(160, 1000.0 / safeMs));
      if (displayCps) displayCps.textContent = targetCps.toFixed(1);
      if (currentConfig?.engine) currentConfig.engine.target_cps = targetCps;
    } else {
      const safeVal = isNaN(val) ? 29 : Math.max(0.1, Math.min(160, val));
      e.target.value = safeVal;
      if (cpsRange) cpsRange.value = safeVal;
      if (displayCps) displayCps.textContent = safeVal.toFixed(1);
      if (currentConfig?.engine) currentConfig.engine.target_cps = safeVal;
    }
    saveConfig();
  });
}

// ── Jitter slider + number input (Dashboard) ─────────────────
if (randomRange) randomRange.addEventListener("input", (e) => {
  const val = parseFloat(e.target.value);
  const safeVal = isNaN(val) ? 0 : Math.max(0, Math.min(35, val));
  if (randomInput) randomInput.value = safeVal;
  if (currentConfig?.engine) currentConfig.engine.jitter_percent = safeVal;
  saveConfigThrottled();
});
if (randomInput) randomInput.addEventListener("input", (e) => {
  const val = parseFloat(e.target.value);
  const safeVal = isNaN(val) ? 0 : Math.max(0, Math.min(35, val));
  if (randomRange) randomRange.value = safeVal;
  if (currentConfig?.engine) currentConfig.engine.jitter_percent = safeVal;
  saveConfigThrottled();
});

if (limitInput) limitInput.addEventListener("input", (e) => {
  // When user types a number, dynamically expand the slider range if needed
  const val = parseInt(e.target.value, 10);
  const safeVal = isNaN(val) || val < 0 ? 0 : val;
  if (limitRange) {
    const currentMax = parseInt(limitRange.max, 10) || 10000;
    if (safeVal > currentMax) limitRange.max = safeVal;
    limitRange.value = safeVal;
  }
  updateLimitBadge(safeVal);
  saveConfig();
});

if (clickTypeSelect) clickTypeSelect.addEventListener("change", saveConfig);
if (posXInput) posXInput.addEventListener("change", saveConfig);
if (posYInput) posYInput.addEventListener("change", saveConfig);
if (repeatCountInput) repeatCountInput.addEventListener("input", () => {
  updateSubSettingsVisibility();
  saveConfig();
});
if (holdDurationInput) holdDurationInput.addEventListener("change", saveConfig);
if (holdIntervalInput) holdIntervalInput.addEventListener("change", saveConfig);
if (repeatIntervalInput) repeatIntervalInput.addEventListener("change", saveConfig);
if (guiLockDelayInput) guiLockDelayInput.addEventListener("change", saveConfig);
const smartRecordCbEl = document.getElementById("smartRecordCheckbox");
if (smartRecordCbEl) smartRecordCbEl.addEventListener("change", saveConfig);
const keyTtlInpEl = document.getElementById("keyTtlInput");
if (keyTtlInpEl) {
  keyTtlInpEl.addEventListener("change", saveConfig);
  keyTtlInpEl.addEventListener("input", () => {
    if (!currentConfig.hotkeys) currentConfig.hotkeys = {};
    const val = parseInt(keyTtlInpEl.value, 10);
    if (!isNaN(val)) {
      currentConfig.hotkeys.key_ttl_ms = Math.max(100, Math.min(5000, val));
      saveConfigThrottled();
    }
  });
}
// ── Jitter radius slider + number input (Dashboard, two-way mirror) ──
const jitterRadiusRange = document.getElementById("jitterRadiusRange");
if (jitterRadiusRange) jitterRadiusRange.addEventListener("input", (e) => {
  const val = parseInt(e.target.value, 10);
  const safeVal = isNaN(val) ? 0 : Math.max(0, Math.min(50, val));
  if (jitterRadiusInput) jitterRadiusInput.value = safeVal;
  if (currentConfig && currentConfig.engine) currentConfig.engine.jitter_radius_px = safeVal;
  saveConfigThrottled();
});
if (jitterRadiusInput) {
  jitterRadiusInput.addEventListener("input", (e) => {
    const val = parseInt(e.target.value, 10);
    const safeVal = isNaN(val) ? 0 : Math.max(0, Math.min(50, val));
    const range = document.getElementById("jitterRadiusRange");
    if (range) range.value = safeVal;
    if (currentConfig && currentConfig.engine) currentConfig.engine.jitter_radius_px = safeVal;
    saveConfigThrottled();
  });
  jitterRadiusInput.addEventListener("change", saveConfig);
}
if (rippleCheckbox) rippleCheckbox.addEventListener("change", async () => {
  currentConfig.ui.visual_ripple = rippleCheckbox.checked;
  // NOTE: save_app_config (Rust) already syncs the lazy overlay WebView when
  // visual_ripple flips. This direct invoke is a fast-path for instant feedback;
  // failures are non-fatal because saveConfig() below triggers the same sync.
  try {
    await invoke("toggle_overlay", { show: rippleCheckbox.checked });
  } catch (e) {
    console.error("toggle_overlay failed:", e);
  }
  saveConfig();
});
if (hudCheckbox) hudCheckbox.addEventListener("change", async () => {
  currentConfig.ui.show_hud = hudCheckbox.checked;
  // Same as above: save_app_config syncs the lazy HUD WebView as fallback.
  try {
    await invoke("toggle_hud_window", { show: hudCheckbox.checked });
  } catch (e) {
    console.error("toggle_hud_window failed:", e);
  }
  saveConfig();
});

// Radio buttons change listener
document.querySelectorAll('input[type="radio"]').forEach(r => {
  r.addEventListener("change", () => {
    document.querySelectorAll(`input[name="${r.name}"]`).forEach(item => {
      const parentLabel = item.closest(".radio-item");
      if (parentLabel) {
        if (item.checked) parentLabel.classList.add("selected");
        else parentLabel.classList.remove("selected");
      }
    });
    updateSubSettingsVisibility();
    saveConfig();
  });
});

// ── CURSOR POSITION PICKER (🔍 BUTTON) ──────────────────────────
let isPickingPos = false;
let pickPosInterval = null;

function stopPositionPicker() {
  isPickingPos = false;
  if (pickPosInterval) {
    clearInterval(pickPosInterval);
    pickPosInterval = null;
  }
  if (pickPosBtn) {
    pickPosBtn.classList.remove("picking");
    pickPosBtn.textContent = "🔍";
  }
  if (pickPosStatus) pickPosStatus.classList.add("hidden");
  window.removeEventListener("keydown", handlePickPosKeyDown);
}

function handlePickPosKeyDown(e) {
  if (e.key === "Enter" || e.code === "Enter") {
    e.preventDefault();
    e.stopPropagation();
    setRadioChecked("posMode", "fixed");
    saveConfig();
    stopPositionPicker();
  } else if (e.key === "Escape" || e.code === "Escape") {
    stopPositionPicker();
  }
}

if (pickPosBtn) {
  pickPosBtn.addEventListener("click", () => {
    if (isPickingPos) {
      stopPositionPicker();
      return;
    }

    isPickingPos = true;
    pickPosBtn.classList.add("picking");
    pickPosBtn.textContent = "🎯";
    if (pickPosStatus) pickPosStatus.classList.remove("hidden");

    window.addEventListener("keydown", handlePickPosKeyDown);

    pickPosInterval = setInterval(async () => {
      if (!isPickingPos) return;
      try {
        const [x, y] = await invoke("get_current_mouse_pos");
        if (posXInput) posXInput.value = x;
        if (posYInput) posYInput.value = y;
      } catch (err) {
        console.error("Failed to poll mouse position:", err);
      }
    }, 50);
  });
}

// ── HOTKEY KEY COMBINATION RECORDER ──────────────────────────
let activeRecordingBtn = null;

function codeToPhysicalKey(code, key) {
  if (code.startsWith("Key")) return code.slice(3).toUpperCase();
  if (code.startsWith("Digit")) return code.slice(5);
  if (code === "NumpadMultiply") return "*";
  if (code === "NumpadAdd") return "+";
  if (code === "NumpadSubtract") return "-";
  if (code === "NumpadDivide") return "/";
  if (code === "NumpadDecimal") return ".";
  if (code.startsWith("Numpad")) return code.slice(6);
  if (code === "Space") return "Space";
  if (code.startsWith("F") && !isNaN(code.slice(1))) return code;
  if (code === "Escape") return "Escape";
  if (code === "Insert") return "Insert";
  if (code === "Delete") return "Delete";
  if (code === "Home") return "Home";
  if (code === "End") return "End";
  if (code === "PageUp") return "PageUp";
  if (code === "PageDown") return "PageDown";
  if (code === "Pause") return "Pause";
  if (code === "ScrollLock") return "ScrollLock";
  if (code === "ControlLeft" || code === "ControlRight") return "Ctrl";
  if (code === "AltLeft" || code === "AltRight") return "Alt";
  if (code === "ShiftLeft" || code === "ShiftRight") return "Shift";
  if (key === "*") return "*";
  if (key === "+") return "+";
  return key ? key.toUpperCase() : code;
}

function setupHotkeyRecorder(btn, targetKey) {
  if (!btn) return;
  btn.addEventListener("click", () => {
    if (activeRecordingBtn) return;
    dbg("Hotkey recorder STARTED for targetKey:", targetKey);
    activeRecordingBtn = btn;
    btn.classList.add("recording");
    const labelEl = btn.querySelector("span:last-child");
    labelEl.textContent = "Press key...";

    // Array of { key: string, pressedAt: number, releasedAt: number | null }
    const keyEvents = [];
    let finishTimeout = null;

    const ttlMs = Math.max(100, Math.min(5000, Number(currentConfig.hotkeys?.key_ttl_ms) || 500));
    const isSmartRecord = currentConfig.hotkeys?.smart_record !== false;

    const cleanExpiredKeys = (now) => {
      for (let i = keyEvents.length - 1; i >= 0; i--) {
        const item = keyEvents[i];
        if (item.releasedAt !== null) {
          if (!isSmartRecord || (now - item.releasedAt > ttlMs)) {
            keyEvents.splice(i, 1);
          }
        }
      }
    };

    const getBindingString = () => {
      const modifiers = [];
      const regularKeys = [];
      for (const item of keyEvents) {
        const k = item.key;
        if (["Ctrl", "Alt", "Shift"].includes(k)) {
          if (!modifiers.includes(k)) modifiers.push(k);
        } else {
          if (!regularKeys.includes(k)) regularKeys.push(k);
        }
      }
      return [...modifiers, ...regularKeys].join("+");
    };

    const handleKeyDown = (e) => {
      e.preventDefault();
      e.stopPropagation();

      const physicalName = codeToPhysicalKey(e.code, e.key);
      const now = Date.now();

      cleanExpiredKeys(now);

      let existing = keyEvents.find(k => k.key === physicalName);
      if (!existing) {
        keyEvents.push({ key: physicalName, pressedAt: now, releasedAt: null });
      } else {
        existing.releasedAt = null;
      }

      const bindingStr = getBindingString();
      labelEl.textContent = bindingStr || "Press key...";
      dbg("Hotkey keydown:", physicalName, "current sequence:", bindingStr, "smart_ttl:", ttlMs);

      if (finishTimeout) clearTimeout(finishTimeout);
      finishTimeout = setTimeout(() => {
        dbg("Hotkey recording finalized for:", targetKey, "->", bindingStr);
        finalizeRecording(bindingStr);
      }, 400);
    };

    const handleKeyUp = (e) => {
      const physicalName = codeToPhysicalKey(e.code, e.key);
      const now = Date.now();

      let existing = keyEvents.find(k => k.key === physicalName);
      if (existing) {
        existing.releasedAt = now;
      }

      cleanExpiredKeys(now);

      if (keyEvents.length > 0) {
        if (finishTimeout) clearTimeout(finishTimeout);
        finishTimeout = setTimeout(() => {
          cleanExpiredKeys(Date.now());
          const bindingStr = getBindingString();
          finalizeRecording(bindingStr);
        }, isSmartRecord ? 200 : 100);
      }
    };

    function finalizeRecording(bindingStr) {
      if (!bindingStr) {
        labelEl.textContent = currentConfig.hotkeys?.[targetKey] || "R / K";
        btn.classList.remove("recording");
        activeRecordingBtn = null;
        window.removeEventListener("keydown", handleKeyDown, true);
        window.removeEventListener("keyup", handleKeyUp, true);
        return;
      }
      if (targetKey === "toggle") {
        currentConfig.hotkeys.toggle = bindingStr;
      } else if (targetKey === "mode_switch") {
        currentConfig.hotkeys.mode_switch = bindingStr;
        if (footerModeShortcut) footerModeShortcut.textContent = bindingStr;
        if (modeSwitchRecordLabel) modeSwitchRecordLabel.textContent = bindingStr;
        _lastSetMode = null;
        setModeDisplay(currentConfig.active_mode || "autoclicker");
      } else if (targetKey === "emergency_stop") {
        currentConfig.hotkeys.emergency_stop = bindingStr;
      } else if (targetKey === "speed_up") {
        currentConfig.hotkeys.speed_up = bindingStr;
      } else if (targetKey === "slow_down") {
        currentConfig.hotkeys.slow_down = bindingStr;
      } else if (targetKey === "capture_pos") {
        currentConfig.hotkeys.capture_pos = bindingStr;
      } else if (targetKey === "record_hotkey") {
        currentConfig.hotkeys.record_hotkey = bindingStr;
        if (recordMacroHotkeyLabel) recordMacroHotkeyLabel.textContent = bindingStr;
      }
      labelEl.textContent = bindingStr;
      btn.classList.remove("recording");
      activeRecordingBtn = null;
      window.removeEventListener("keydown", handleKeyDown, true);
      window.removeEventListener("keyup", handleKeyUp, true);
      saveConfig();
    }

    window.addEventListener("keydown", handleKeyDown, true);
    window.addEventListener("keyup", handleKeyUp, true);
  });
}

// Wire hotkey recorder buttons at module-eval time (DOM is ready for ES modules)
setupHotkeyRecorder(hotkeyRecordBtn, "toggle");
setupHotkeyRecorder(modeSwitchRecordBtn, "mode_switch");
setupHotkeyRecorder(recordMacroHotkeyBtn, "record_hotkey");

let lastRecordToggleAt = 0;

async function finishRecordedMacros(macros, source = "recording") {
  const arr = Array.isArray(macros) ? macros : [];
  if (arr.length === 0) return 0;
  const m = arr[0];
  const name = prompt("Name this macro:", m.name || "Untitled");
  if (name && name.trim()) {
    m.name = name.trim();
    await saveMacro(m);
    await renderMacroList();
    logCall("→MACRO", `saved recorded macro from ${source}`, `${m.name} (${m.actions?.length || 0} actions)`);
  }
  return arr.length;
}

async function toggleRecordingFromHotkey(source = "global") {
  const now = Date.now();
  if (now - lastRecordToggleAt < 250) return;
  lastRecordToggleAt = now;
  try {
    const status = document.getElementById("automationRecordingStatus");
    const recordBtn = document.getElementById("automationRecordBtn");
    if (!isRecording) {
      await invoke("record_start", { mode: "smart", recordHotkey: currentConfig.hotkeys?.record_hotkey || "Ctrl+Shift+R" });
      isRecording = true;
      recordActionCount = 0;
      showRecordingOverlay(true);
      if (status) status.style.display = "block";
      if (recordBtn) recordBtn.disabled = true;
    } else {
      const macros = await invoke("record_stop");
      isRecording = false;
      showRecordingOverlay(false);
      if (status) status.style.display = "none";
      if (recordBtn) recordBtn.disabled = false;
      await finishRecordedMacros(macros, source);
    }
    logCall("→HOTKEY", `record toggle handled from ${source}`, `recording=${isRecording}`);
  } catch (err) {
    console.error("Record toggle hotkey failed:", err);
  }
}

listen("global-record-toggle", async () => {
  await toggleRecordingFromHotkey("global");
});

listen("global-cps-change", async (event) => {
  const next = Number(event.payload);
  if (!Number.isFinite(next) || !currentConfig?.engine) return;
  currentConfig.engine.target_cps = next;
  if (cpsRange) cpsRange.value = next;
  if (cpsInput) cpsInput.value = next;
  if (displayCps) displayCps.textContent = next.toFixed(1);
  await invoke("save_app_config", { config: currentConfig });
});

listen("global-capture-pos", async (event) => {
  const payload = event.payload;
  const x = Array.isArray(payload) ? payload[0] : payload?.x;
  const y = Array.isArray(payload) ? payload[1] : payload?.y;
  if (!Number.isFinite(Number(x)) || !Number.isFinite(Number(y)) || !currentConfig?.engine) return;
  currentConfig.engine.position_mode = "fixed";
  currentConfig.engine.fixed_x = Number(x);
  currentConfig.engine.fixed_y = Number(y);
  if (posXInput) posXInput.value = currentConfig.engine.fixed_x;
  if (posYInput) posYInput.value = currentConfig.engine.fixed_y;
  const fixedRadio = document.querySelector('input[name="posMode"][value="fixed"]');
  if (fixedRadio) fixedRadio.checked = true;
  await invoke("save_app_config", { config: currentConfig });
});

// Preset slot hotkeys: payload = zero-based slot index (0..8).
listen("global-preset-hotkey", async (event) => {
  const slot = Number(event.payload);
  if (!Number.isInteger(slot) || slot < 0 || slot > 8) return;
  ensurePresetsExist();
  const preset = currentConfig.presets?.[slot];
  if (!preset) {
    console.warn(`preset slot ${slot + 1} is empty`);
    return;
  }
  await applyPreset(preset.id);
});

// App-profile auto-switch: payload = preset id chosen by the window-title
// rule that matched the current foreground window.
listen("app-profile-activate", async (event) => {
  const presetId = String(event.payload || "");
  if (!presetId) return;
  await applyPreset(presetId);
});
setupHotkeyRecorder(emergencyRecordBtn, "emergency_stop");
setupHotkeyRecorder(speedUpRecordBtn, "speed_up");
setupHotkeyRecorder(slowDownRecordBtn, "slow_down");
setupHotkeyRecorder(pickPosRecordBtn, "capture_pos");

// ── OPEN CONFIG FOLDER BUTTON ────────────────────────────────
if (openConfigFolderBtn) {
  openConfigFolderBtn.addEventListener("click", async () => {
    try { await invoke("open_config_folder"); }
    catch (err) { console.error("Failed to open config folder:", err); }
  });
}

// ── BEHAVIOR AUTOSTART LISTENER ──────────────────────────────
if (autostartCheckbox) {
  autostartCheckbox.addEventListener("change", async () => {
    saveConfig();
    try { await invoke("set_windows_autostart", { enable: autostartCheckbox.checked }); }
    catch (err) { console.error("Failed to set Windows autostart:", err); }
  });
}
if (alwaysOnTopCheckbox) {
  alwaysOnTopCheckbox.addEventListener("change", () => {
    applyAlwaysOnTop(alwaysOnTopCheckbox.checked);
    saveConfig();
  });
}
if (startMinimizedCheckbox) startMinimizedCheckbox.addEventListener("change", saveConfig);
if (minimizeToTrayCheckbox) minimizeToTrayCheckbox.addEventListener("change", saveConfig);
if (notificationsCheckbox) notificationsCheckbox.addEventListener("change", saveConfig);
if (pauseFocusLossCheckbox) pauseFocusLossCheckbox.addEventListener("change", saveConfig);
if (rememberPosCheckbox) rememberPosCheckbox.addEventListener("change", saveConfig);

// ── DYNAMIC THEMES & ACCENT ENGINE ───────────────────────────
function applyTheme(themeName, accentHex) {
  document.documentElement.setAttribute("data-theme", themeName || "cyberpunk");
  if (accentHex) {
    document.documentElement.style.setProperty("--cyan", accentHex);
    document.documentElement.style.setProperty("--border-cyan", accentHex + "4d");
    if (!currentConfig.ui) currentConfig.ui = {};
    currentConfig.ui.accent_color = accentHex;
  }
}

function updateSwatchActiveState(accentHex) {
  if (accentSwatches && accentSwatches.forEach) {
    accentSwatches.forEach(swatch => {
      if (swatch.getAttribute("data-accent").toLowerCase() === accentHex.toLowerCase()) {
        swatch.classList.add("active");
      } else {
        swatch.classList.remove("active");
      }
    });
  }
}

// Wire language, theme & accent listeners
if (languageSelect) {
  languageSelect.addEventListener("change", async () => {
    const chosenLang = languageSelect.value;
    if (window.I18nEngine) {
      await window.I18nEngine.setLanguage(chosenLang);
    }
    if (currentConfig && currentConfig.ui) {
      currentConfig.ui.language = chosenLang;
    }
    saveConfigThrottled();
  });
}

if (themeSelect) {
  themeSelect.addEventListener("change", () => {
    applyTheme(themeSelect.value, currentConfig.ui?.accent_color);
    saveConfig();
  });
}
accentSwatches.forEach(swatch => {
  swatch.addEventListener("click", () => {
    const color = swatch.getAttribute("data-accent");
    updateSwatchActiveState(color);
    applyTheme(themeSelect?.value || "cyberpunk", color);
    saveConfig();
  });
});

// ── MODE TOGGLE ─────────────────────────────────────────────
if (modeToggleBtn) {
  dbg("modeToggleBtn found — wiring click listener");
  modeToggleBtn.addEventListener("click", async () => {
    dbg("modeToggleBtn CLICKED — requesting mode toggle");
    try {
      const newMode = await invoke("toggle_mode");
      dbg("modeToggleBtn toggled successfully — newMode:", newMode);
      setModeDisplay(newMode, true);
      saveConfig();
    } catch (err) {
      console.error("Failed to toggle mode:", err);
      dbg("modeToggleBtn ERROR:", err);
    }
  });
}

// ── PRESETS MANAGER V2 ──────────────────────────────────────
const defaultPresetList = [
  {
    id: "fast_cps",
    name: "Fast CPS",
    description: "29 CPS | 7.5% Jitter | Single Left",
    icon: "⚡",
    target_cps: 29.0,
    jitter_percent: 7.5,
    click_limit: 0,
    button: "left",
    click_type: "single",
    position_mode: "cursor",
    fixed_x: 100,
    fixed_y: 100,
    hold_duration_ms: 500,
    hold_interval_ms: 1000,
    is_default: true
  },
  {
    id: "gaming_boost",
    name: "Gaming Boost",
    description: "15 CPS | 5.0% Jitter | Single Left",
    icon: "🎮",
    target_cps: 15.0,
    jitter_percent: 5.0,
    click_limit: 0,
    button: "left",
    click_type: "single",
    position_mode: "cursor",
    fixed_x: 100,
    fixed_y: 100,
    hold_duration_ms: 500,
    hold_interval_ms: 1000,
    is_default: true
  },
  {
    id: "human_emulation",
    name: "Human Emulation",
    description: "8 CPS | 15.0% Jitter | Single Left",
    icon: "👤",
    target_cps: 8.0,
    jitter_percent: 15.0,
    click_limit: 0,
    button: "left",
    click_type: "single",
    position_mode: "cursor",
    fixed_x: 100,
    fixed_y: 100,
    hold_duration_ms: 500,
    hold_interval_ms: 1000,
    is_default: true
  },
  {
    id: "afk_farm",
    name: "AFK Farm",
    description: "2 CPS | 2.0% Jitter | Single Left",
    icon: "🌾",
    target_cps: 2.0,
    jitter_percent: 2.0,
    click_limit: 0,
    button: "left",
    click_type: "single",
    position_mode: "cursor",
    fixed_x: 100,
    fixed_y: 100,
    hold_duration_ms: 500,
    hold_interval_ms: 1000,
    is_default: true
  }
];

function ensurePresetsExist() {
  if (!currentConfig.presets || !Array.isArray(currentConfig.presets) || currentConfig.presets.length === 0) {
    currentConfig.presets = JSON.parse(JSON.stringify(defaultPresetList));
  }
}

// Build all preset cards into a single HTML string and attach handlers via
// event delegation on the container. This avoids per-card innerHTML + createElement
// (which was doubling DOM work) and prevents full-grid rerenders from being slow.
function renderPresetsGrid() {
  const container = document.getElementById("presetGridContainer");
  if (!container) return;

  ensurePresetsExist();
  const emptyState = document.getElementById("presetsEmptyState");

  if (currentConfig.presets.length === 0) {
    container.innerHTML = "";
    if (emptyState) emptyState.classList.remove("hidden");
    return;
  }
  if (emptyState) emptyState.classList.add("hidden");

  // Build the whole grid in one innerHTML write
  const html = currentConfig.presets.map(p => {
    const posStr = p.position_mode === "fixed"
      ? getI18nText("presets_card_tag_fixed", { x: p.fixed_x, y: p.fixed_y }, `Fixed (${p.fixed_x},${p.fixed_y})`)
      : getI18nText("presets_card_tag_cursor", {}, "Cursor");
    const clickTypeKey = `dash_type_${p.click_type || "single"}`;
    const clickTypeStr = getI18nText(clickTypeKey, {}, p.click_type ? (p.click_type.charAt(0).toUpperCase() + p.click_type.slice(1)) : "Single");
    const btnKey = `dash_btn_${p.button || "left"}`;
    const btnName = getI18nText(btnKey, {}, p.button ? (p.button.charAt(0).toUpperCase() + p.button.slice(1)) : "Left");
    const btnStr = getI18nText("presets_card_tag_btn", { btn: btnName }, `${btnName} Btn`);
    const limitStr = p.click_limit > 0 ? getI18nText("presets_card_tag_limit", { val: p.click_limit }, `Limit: ${p.click_limit}`) : null;
    const jitterStr = getI18nText("presets_card_tag_jitter", { val: p.jitter_percent }, `±${p.jitter_percent}% Jitter`);
    const runBtnText = getI18nText("presets_card_btn_run", {}, "▶ Run");
    const runTitle = getI18nText("presets_card_btn_run_title", {}, "Apply and run now");
    const applyBtnText = getI18nText("presets_card_btn_apply", {}, "⚡ Apply");
    const applyTitle = getI18nText("presets_card_btn_apply_title", {}, "Apply preset");
    const inspectTitle = getI18nText("presets_card_btn_inspect_title", {}, "View details");
    const editTitle = getI18nText("presets_card_btn_edit_title", {}, "Edit");
    const deleteTitle = getI18nText("presets_card_btn_delete_title", {}, "Delete");
    const icon = p.icon || '🎯';
    const name = escapeHtml(p.name);
    // data-action hook for delegated click handler below
    return `
      <div class="preset-card-new" data-id="${p.id}">
        <div class="preset-card-accent"></div>
        <div class="preset-card-body">
          <div class="preset-card-top">
            <div class="preset-card-icon-title">
              <span class="preset-card-emoji">${icon}</span>
              <span class="preset-card-name">${name}</span>
            </div>
            <span class="preset-card-cps-badge">${p.target_cps} CPS</span>
          </div>
          <div class="preset-card-tags">
            <span class="preset-tag">${jitterStr}</span>
            <span class="preset-tag">${clickTypeStr}</span>
            <span class="preset-tag">${btnStr}</span>
            <span class="preset-tag">${posStr}</span>
            ${limitStr ? `<span class="preset-tag">${limitStr}</span>` : ''}
          </div>
        </div>
        <div class="preset-card-footer">
          <button type="button" class="preset-run-btn" data-action="run" data-id="${escapeHtml(p.id)}" title="${runTitle}">
              ${runBtnText}
          </button>
          <button type="button" class="preset-apply-btn" data-action="apply" data-id="${escapeHtml(p.id)}" title="${applyTitle}">
              ${applyBtnText}
          </button>
          <button type="button" class="preset-icon-btn" data-action="inspect" data-id="${escapeHtml(p.id)}" title="${inspectTitle}">👁️</button>
          <button type="button" class="preset-icon-btn edit" data-action="edit" data-id="${escapeHtml(p.id)}" title="${editTitle}">✏️</button>
          <button type="button" class="preset-icon-btn danger" data-action="delete" data-id="${escapeHtml(p.id)}" title="${deleteTitle}">🗑️</button>
        </div>
      </div>`;
  }).join("");

  container.innerHTML = html;
  // Single delegated listener for the entire grid (replaces 4 listeners per card).
  // Idempotent: re-bind only if not already attached.
  if (!container.dataset.bound) {
    container.addEventListener("click", (e) => {
      const btn = e.target.closest("[data-action]");
      if (!btn) return;
      const id = btn.dataset.id;
      switch (btn.dataset.action) {
        case "run": void runPreset(id); break;
        case "apply": void applyPreset(id); break;
        case "inspect": inspectPreset(id); break;
        case "edit": {
          const p = currentConfig.presets.find(x => x.id === id);
          if (p) openPresetEditModal(p);
          break;
        }
        case "delete": deletePreset(id); break;
      }
    });
    container.dataset.bound = "1";
  }
}

// ── VISUAL EDITOR HELPERS ─────────────────────────────────────────────────
function closeEditor(backdrop) {
  if (backdrop) backdrop.remove();
  else {
    const b = document.getElementById("visualEditorBackdrop");
    if (b) b.remove();
  }
}

function showRowContextMenu(targetBtn, idx, macro, refresh, closeEditorFn) {
  // Remove any existing context menu and unbind its listeners.
  if (window._activeContextMenuClose) {
    window._activeContextMenuClose();
  }
  document.querySelectorAll(".ve-context-menu").forEach(m => m.remove());

  const rect = targetBtn.getBoundingClientRect();
  const menu = document.createElement("div");
  menu.className = "ve-context-menu";
  menu.style.cssText = `
    position:fixed;left:${rect.right + 4}px;top:${rect.top}px;z-index:100000;
    background:var(--bg-elev);border:1px solid var(--border);border-radius:6px;
    box-shadow:0 8px 24px rgba(0,0,0,0.5);min-width:180px;padding:4px 0;
  `;

  let onDoc = null;
  const closeMenu = () => {
    if (menu.parentNode) {
      menu.remove();
    }
    if (onDoc) {
      document.removeEventListener("click", onDoc, true);
      onDoc = null;
    }
    if (window._activeContextMenuClose === closeMenu) {
      window._activeContextMenuClose = null;
    }
  };
  window._activeContextMenuClose = closeMenu;

  const items = [
    { icon: "▶️", label: "Run from here",
      action: async () => {
        try {
          await invoke("rewind_macro");
          await invoke("play_macro_from", { m: macro, startIdx: idx });
          closeEditorFn();
        } catch (e) { console.error(e); }
      } },
    { icon: "⏭", label: "Step (one action)",
      action: async () => {
        try {
          await invoke("step_macro", { m: macro });
        } catch (e) { console.error(e); }
      } },
    { icon: "⏮", label: "Rewind to start",
      action: async () => {
        try { await invoke("rewind_macro"); }
        catch (e) { console.error(e); }
      } },
    { icon: "📋", label: "Duplicate",
      action: () => {
        macro.actions.splice(idx + 1, 0, JSON.parse(JSON.stringify(macro.actions[idx])));
        macro.enabled.splice(idx + 1, 0, macro.enabled[idx]);
        refresh();
      } },
  ];
  for (const it of items) {
    const a = document.createElement("button");
    a.textContent = `${it.icon} ${it.label}`;
    a.style.cssText = "display:block;width:100%;padding:6px 12px;background:none;border:none;text-align:left;cursor:pointer;color:var(--text-bright);font-size:13px;";
    a.addEventListener("mouseover", () => { a.style.background = "var(--accent)"; a.style.color = "white"; });
    a.addEventListener("mouseout", () => { a.style.background = "none"; a.style.color = "var(--text-bright)"; });
    a.addEventListener("click", () => {
      closeMenu();
      it.action();
    });
    menu.appendChild(a);
  }
  document.body.appendChild(menu);
  // Close on click-away.
  setTimeout(() => {
    onDoc = (ev) => {
      if (!menu.contains(ev.target)) {
        closeMenu();
      }
    };
    document.addEventListener("click", onDoc, true);
  }, 0);
}

async function reRecordMacro(macro, onChange) {
  if (!confirm(getI18nText("dialog_confirm_rerecord_macro", { name: macro.name }, `Re-record "${macro.name}"? The existing actions will be replaced once you stop recording.`))) {
    return;
  }
  try {
    await invoke("record_start", { mode: "smart", recordHotkey: currentConfig.hotkeys?.record_hotkey || "Ctrl+Shift+R" });
    isRecording = true;
    showRecordingOverlay(true);
    const status = document.getElementById("automationRecordingStatus");
    if (status) status.style.display = "block";
    // Schedule auto-stop after macro closes: we don't know when user stops,
    // so we just close the editor; user must click stop after.
    closeEditor();
    // Hand control back to the caller: openVisualEditor passes renderMacroList,
    // so the list refreshes as soon as the re-record flow takes over.
    if (typeof onChange === "function") onChange();
  } catch (e) { console.error("re-record failed:", e); }
}

async function applyPreset(presetId) {
  const op = stage("ApplyPreset");
  try {
    await op.run("lookup", () => {
      ensurePresetsExist();
      const p = currentConfig.presets.find(x => x.id === presetId);
      if (!p) { op.log("not-found", presetId); return false; }
      op.log("loaded", `name="${p.name}" cps=${p.target_cps} jitter=${p.jitter_percent}%`);
      return true;
    });

    const p = currentConfig.presets.find(x => x.id === presetId);
    if (!p) { op.fail("apply-aborted"); return false; }

    await op.run("copy-fields", () => {
      const st = ensureStatsConfig();
      st.presets_applied = Number(st.presets_applied || 0) + 1;
      saveConfigThrottled();
      currentConfig.engine.target_cps = p.target_cps;
      currentConfig.engine.jitter_percent = p.jitter_percent;
      currentConfig.engine.click_limit = p.click_limit || 0;
      currentConfig.engine.button = p.button || "left";
      currentConfig.engine.click_type = p.click_type || "single";
      currentConfig.engine.position_mode = p.position_mode || "cursor";
      if (p.fixed_x !== undefined) currentConfig.engine.fixed_x = p.fixed_x;
      if (p.fixed_y !== undefined) currentConfig.engine.fixed_y = p.fixed_y;
      currentConfig.engine.hold_duration_ms = Number(p.hold_duration_ms) || 500;
      currentConfig.engine.hold_interval_ms = Number(p.hold_interval_ms) || 1000;
      currentConfig.engine.jitter_radius_px = Number(p.jitter_radius_px) || 0;
      currentConfig.engine.repeat_mode = p.repeat_mode || "unlimited";
      currentConfig.engine.repeat_count = Number(p.repeat_count) || 0;
      currentConfig.engine.repeat_interval_ms = p.repeat_interval_ms == null ? 1000 : Number(p.repeat_interval_ms);
      currentConfig.engine.start_delay_ms = Number(p.start_delay_ms) || 0;
      currentConfig.engine.stop_duration_min = Number(p.stop_duration_min) || 0;
      currentConfig.engine.stop_time_str = p.stop_time_str || "";
      // Multi-point sequence: copy onto engine so the scheduler picks it up.
      currentConfig.engine.sequence_points = Array.isArray(p.points)
        ? JSON.parse(JSON.stringify(p.points))
        : [];
      return "engine fields copied";
    });

    await op.run("refresh-ui", async () => {
      updateUiFromConfig(currentConfig);
      await saveConfig();
      return "UI + config saved";
    });

    await op.run("navigate-dashboard", () => {
      document.querySelectorAll(".nav-item").forEach(b => b.classList.remove("active"));
      const dashBtn = document.getElementById("navDashboard");
      if (dashBtn) dashBtn.classList.add("active");
      document.querySelectorAll(".view").forEach(v => v.classList.remove("active"));
      const dash = document.getElementById("viewDashboard");
      if (dash) dash.classList.add("active");
      return "switched to Dashboard";
    });

    op.ok();
    return true;
  } catch (err) {
    op.fail("apply-preset", err);
    console.error("[ApplyPreset] aborted:", err);
    return false;
  }
}

// One-click "Run preset": apply settings + start autoclicker immediately.
// Bridges Presets tab → Dashboard Start button → executeStartAutomation().
async function runPreset(presetId) {
  ensurePresetsExist();
  const p = currentConfig.presets.find(x => x.id === presetId);
  if (!p) return;

  // 1) apply preset values into currentConfig (same path as the ⚡ Apply button)
  const applied = await applyPreset(presetId);
  if (!applied) return;

  // 2) warn if work-mode blocks Start
  if (currentConfig.active_mode === "work") {
    console.warn("[runPreset] blocked by work-mode — switch to Autoclicker first");
    if (typeof showToast === "function") {
      showToast("⚠️ " + getI18nText("dash_hint_switch_to_auto", {}, "Switch to Autoclicker Mode to Start"), "warn");
    }
    return;
  }

  // 3) honor the preset's persisted start delay
  const startDelayMs = Number(p.start_delay_ms ?? currentConfig.engine.start_delay_ms ?? 0) || 0;
  const startDelaySec = Math.round(startDelayMs / 1000);

  if (startDelayMs > 0) {
    if (typeof showToast === "function") {
      showToast(`▶ Running "${p.name}" in ${startDelaySec}s...`, "info");
    }
    setTimeout(async () => {
      await executeStartAutomation();
    }, startDelayMs);
  } else {
    if (typeof showToast === "function") {
      showToast(`▶ "${p.name}" started`, "success");
    }
    await executeStartAutomation();
  }
}

function inspectPreset(presetId) {
  ensurePresetsExist();
  const p = currentConfig.presets.find(x => x.id === presetId);
  if (!p) return;

  const modal = document.getElementById("presetInspectModal");
  const title = document.getElementById("inspectTitle");
  const body = document.getElementById("inspectBodyContent");
  const applyBtn = document.getElementById("inspectApplyBtn");

  if (title) title.textContent = getI18nText("inspect_modal_title", { name: `${p.icon || ''} ${p.name}` }, `🔍 Preset details: ${p.icon || ''} ${p.name}`);

  const intervalMs = (1000 / p.target_cps).toFixed(2);
  const clickTypeStr = getI18nText(`dash_type_${p.click_type || 'single'}`, {}, p.click_type || 'single');
  const btnKey = `dash_btn_${p.button || 'left'}`;
  const btnStr = getI18nText(btnKey, {}, p.button || 'left');
  const posModeStr = p.position_mode === 'fixed'
    ? getI18nText("presets_card_tag_fixed", { x: p.fixed_x, y: p.fixed_y }, `Fixed (X: ${p.fixed_x}, Y: ${p.fixed_y})`)
    : getI18nText("preset_pos_follow_cursor", {}, 'Follow cursor');
  const limitStr = p.click_limit > 0
    ? `${p.click_limit} clicks`
    : getI18nText("preset_repeat_unlimited", {}, 'Unlimited');

  if (body) {
    body.innerHTML = `
      <div class="inspect-row">
        <span class="inspect-label">${getI18nText("inspect_label_name", {}, "Name:")}</span>
        <span class="inspect-value">${escapeHtml(p.name)}</span>
      </div>
      <div class="inspect-row">
        <span class="inspect-label">${getI18nText("inspect_label_speed", {}, "Click speed (CPS):")}</span>
        <span class="inspect-value">${p.target_cps} CPS (${intervalMs} ms)</span>
      </div>
      <div class="inspect-row">
        <span class="inspect-label">${getI18nText("inspect_label_jitter", {}, "Randomization (Jitter):")}</span>
        <span class="inspect-value">±${p.jitter_percent}%</span>
      </div>
      <div class="inspect-row">
        <span class="inspect-label">${getI18nText("inspect_label_click_type", {}, "Click type:")}</span>
        <span class="inspect-value">${clickTypeStr}</span>
      </div>
      <div class="inspect-row">
        <span class="inspect-label">${getI18nText("inspect_label_button", {}, "Mouse button:")}</span>
        <span class="inspect-value">${btnStr}</span>
      </div>
      <div class="inspect-row">
        <span class="inspect-label">${getI18nText("inspect_label_pos", {}, "Position mode:")}</span>
        <span class="inspect-value">${posModeStr}</span>
      </div>
      <div class="inspect-row">
        <span class="inspect-label">${getI18nText("inspect_label_limit", {}, "Click limit:")}</span>
        <span class="inspect-value">${limitStr}</span>
      </div>
    `;
  }

  if (applyBtn) {
    // Runs on EVERY modal open with a different `p.id`, so the handler must be
    // replaced rather than appended — addEventListener would stack handlers and
    // make a single click apply several presets.
    // oxlint-disable-next-line unicorn/prefer-add-event-listener
    applyBtn.onclick = () => {
      modal.classList.add("hidden");
      void applyPreset(p.id);
    };
  }

  modal.classList.remove("hidden");
}

// ── PRESET HOTKEY SLOTS (1-9) ────────────────────────────────
// Each preset can be bound to a global hotkey. Bindings are stored in
// currentConfig.hotkeys.preset_hotkeys[slotIndex] (empty string = unbound).
function renderPresetHotkeySlots() {
  const grid = document.getElementById("presetHotkeySlots");
  if (!grid) return;
  if (!Array.isArray(currentConfig.hotkeys.preset_hotkeys)) currentConfig.hotkeys.preset_hotkeys = [];
  const slots = currentConfig.hotkeys.preset_hotkeys;
  while (slots.length < 9) slots.push("");

  const captureBinding = (btn, slotIdx, labelEl) => {
    btn.classList.add("recording");
    labelEl.textContent = "Press key...";
    const pressed = [];
    let finishTimer = null;

    const cleanup = () => {
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("keyup", onKeyUp, true);
      window.removeEventListener("click", onClickAway, true);
      btn.classList.remove("recording");
    };

    const onClickAway = (ev) => {
      if (!btn.contains(ev.target)) {
        cleanup();
        labelEl.textContent = slots[slotIdx] || "Not set";
      }
    };

    const onKey = (e) => {
      e.preventDefault();
      e.stopPropagation();
      const name = codeToPhysicalKey(e.code, e.key);
      if (!pressed.includes(name)) pressed.push(name);
      const mods = pressed.filter(k => ["Ctrl", "Alt", "Shift"].includes(k));
      const keys = pressed.filter(k => !["Ctrl", "Alt", "Shift"].includes(k));
      labelEl.textContent = [...mods, ...keys].join("+") || "Press key...";
    };

    const onKeyUp = () => {
      if (finishTimer) clearTimeout(finishTimer);
      finishTimer = setTimeout(() => {
        cleanup();
        const mods = pressed.filter(k => ["Ctrl", "Alt", "Shift"].includes(k));
        const keys = pressed.filter(k => !["Ctrl", "Alt", "Shift"].includes(k));
        const binding = [...mods, ...keys].join("+");
        if (binding) slots.forEach((v, i2) => { if (v === binding && i2 !== slotIdx) slots[i2] = ""; });
        slots[slotIdx] = binding;
        labelEl.textContent = binding || "Not set";
        saveConfig();
      }, 150);
    };

    window.addEventListener("keydown", onKey, true);
    window.addEventListener("keyup", onKeyUp, true);
    setTimeout(() => window.addEventListener("click", onClickAway, true), 0);
  };

  grid.innerHTML = slots.map((binding, idx) => `
    <div style="display:flex;align-items:center;gap:4px;font-size:11px;">
      <span style="color:var(--text-dim);min-width:12px;">${idx + 1}</span>
      <button class="ph-slot-btn" data-slot="${idx}" title="Click then press keys"
        style="flex:1;background:var(--bg-elev);border:1px solid var(--border);border-radius:5px;color:var(--text);padding:3px 6px;cursor:pointer;text-align:left;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">${binding || "Not set"}</button>
    </div>`).join("");

  grid.querySelectorAll(".ph-slot-btn").forEach(btn => {
    btn.addEventListener("click", () => {
      const slotIdx = Number(btn.dataset.slot);
      captureBinding(btn, slotIdx, btn);
    });
    btn.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      const slotIdx = Number(btn.dataset.slot);
      currentConfig.hotkeys.preset_hotkeys[slotIdx] = "";
      btn.textContent = "Not set";
      saveConfig();
    });
  });
}
function openPresetEditModal(p = null) {
  const modal = document.getElementById("presetEditModal");
  const editIdInput = document.getElementById("presetEditId");
  const nameInput = document.getElementById("presetNameInput");
  const iconSelect = document.getElementById("presetIconSelect");
  // These six element handles carry the `preset` prefix because the dashboard
  // declares identical identifiers at module scope (cpsRange, clickTypeSelect,
  // holdDurationInput, holdIntervalInput, repeatCountInput, repeatIntervalInput).
  // Shadowing them is legal but a trap: code moved between the modal and the
  // dashboard would silently retarget the wrong widget. (oxlint: no-shadow)
  const presetCpsRangeEl = document.getElementById("presetCpsRange");
  const cpsVal = document.getElementById("presetCpsVal");
  const jitterRange = document.getElementById("presetJitterRange");
  const jitterVal = document.getElementById("presetJitterVal");
  const presetClickTypeSelectEl = document.getElementById("presetClickTypeSelect");
  const presetHoldDurationEl = document.getElementById("presetHoldDuration");
  const presetHoldIntervalEl = document.getElementById("presetHoldInterval");
  const buttonSelect = document.getElementById("presetButtonSelect");
  const positionSelect = document.getElementById("presetPositionSelect");
  const fixedXInput = document.getElementById("presetFixedX");
  const fixedYInput = document.getElementById("presetFixedY");
  const clickLimitInput = document.getElementById("presetClickLimit");
  const startDelayInput = document.getElementById("presetStartDelaySec");
  const stopDurationInput = document.getElementById("presetStopDurationMin");
  const stopTimeInput = document.getElementById("presetStopTime");
  const repeatModeSelect = document.getElementById("presetRepeatModeSelect");
  const presetRepeatCountEl = document.getElementById("presetRepeatCount");
  const presetRepeatIntervalEl = document.getElementById("presetRepeatInterval");
  const coordRow = document.getElementById("presetFixedCoordRow");
  const modalTitle = document.getElementById("presetModalTitle");

  renderPresetHotkeySlots();
  if (p) {
    if (modalTitle) modalTitle.textContent = "✏️ Edit Preset";
    if (editIdInput) editIdInput.value = p.id;
    if (nameInput) nameInput.value = p.name || "";
    if (iconSelect) iconSelect.value = p.icon || "⚡";
    if (presetCpsRangeEl) {
      presetCpsRangeEl.value = p.target_cps || 29;
      if (cpsVal) cpsVal.textContent = p.target_cps || 29;
    }
    if (jitterRange) {
      jitterRange.value = p.jitter_percent || 7.5;
      if (jitterVal) jitterVal.textContent = p.jitter_percent || 7.5;
    }
    if (presetClickTypeSelectEl) presetClickTypeSelectEl.value = p.click_type || "single";
    if (presetHoldDurationEl) presetHoldDurationEl.value = p.hold_duration_ms ?? 500;
    if (presetHoldIntervalEl) presetHoldIntervalEl.value = p.hold_interval_ms ?? 1000;
    if (buttonSelect) buttonSelect.value = p.button || "left";
    if (positionSelect) positionSelect.value = p.position_mode || "cursor";
    if (fixedXInput) fixedXInput.value = p.fixed_x ?? 100;
    if (fixedYInput) fixedYInput.value = p.fixed_y ?? 100;
    if (clickLimitInput) clickLimitInput.value = p.click_limit || 0;
    if (startDelayInput) startDelayInput.value = Math.round((p.start_delay_ms || 0) / 1000);
    if (stopDurationInput) stopDurationInput.value = p.stop_duration_min || 0;
    if (stopTimeInput) stopTimeInput.value = p.stop_time_str || "";
    if (repeatModeSelect) repeatModeSelect.value = p.repeat_mode || "unlimited";
    if (presetRepeatCountEl) presetRepeatCountEl.value = p.repeat_count || 0;
    if (presetRepeatIntervalEl) presetRepeatIntervalEl.value = p.repeat_interval_ms ?? 1000;
  } else {
    if (modalTitle) modalTitle.textContent = "✨ New Preset";
    if (editIdInput) editIdInput.value = "";
    if (nameInput) nameInput.value = "New Preset";
    if (iconSelect) iconSelect.value = "⚡";
    if (presetCpsRangeEl) {
      presetCpsRangeEl.value = currentConfig.engine.target_cps || 29;
      if (cpsVal) cpsVal.textContent = currentConfig.engine.target_cps || 29;
    }
    if (jitterRange) {
      jitterRange.value = currentConfig.engine.jitter_percent || 7.5;
      if (jitterVal) jitterVal.textContent = currentConfig.engine.jitter_percent || 7.5;
    }
    if (presetClickTypeSelectEl) presetClickTypeSelectEl.value = currentConfig.engine.click_type || "single";
    if (presetHoldDurationEl) presetHoldDurationEl.value = currentConfig.engine.hold_duration_ms ?? 500;
    if (presetHoldIntervalEl) presetHoldIntervalEl.value = currentConfig.engine.hold_interval_ms ?? 1000;
    if (buttonSelect) buttonSelect.value = currentConfig.engine.button || "left";
    if (positionSelect) positionSelect.value = currentConfig.engine.position_mode || "cursor";
    if (fixedXInput) fixedXInput.value = currentConfig.engine.fixed_x ?? 100;
    if (fixedYInput) fixedYInput.value = currentConfig.engine.fixed_y ?? 100;
    if (clickLimitInput) clickLimitInput.value = currentConfig.engine.click_limit || 0;
    if (startDelayInput) startDelayInput.value = Math.round((currentConfig.engine.start_delay_ms || 0) / 1000);
    if (stopDurationInput) stopDurationInput.value = currentConfig.engine.stop_duration_min || 0;
    if (stopTimeInput) stopTimeInput.value = currentConfig.engine.stop_time_str || "";
    if (repeatModeSelect) repeatModeSelect.value = currentConfig.engine.repeat_mode || "unlimited";
    if (presetRepeatCountEl) presetRepeatCountEl.value = currentConfig.engine.repeat_count || 0;
    if (presetRepeatIntervalEl) presetRepeatIntervalEl.value = currentConfig.engine.repeat_interval_ms ?? 1000;
  }

  if (p && p.points && Array.isArray(p.points)) {
    window.SequenceEditor?.setPoints(p.points);
  } else {
    window.SequenceEditor?.setPoints([]);
  }

  if (positionSelect && coordRow) {
    coordRow.classList.toggle("hidden", positionSelect.value !== "fixed");
  }

  modal.classList.remove("hidden");
  setTimeout(() => window.SequenceEditor?.draw(), 50);
}

function savePresetFromModal() {
  const editId = document.getElementById("presetEditId")?.value;
  const name = document.getElementById("presetNameInput")?.value?.trim() || "Preset";
  const icon = document.getElementById("presetIconSelect")?.value || "⚡";
  const cps = parseFloat(document.getElementById("presetCpsRange")?.value) || 29;
  const jitter = parseFloat(document.getElementById("presetJitterRange")?.value) || 0;
  const clickType = document.getElementById("presetClickTypeSelect")?.value || "single";
  const holdDurationMs = Math.max(10, parseInt(document.getElementById("presetHoldDuration")?.value, 10) || 500);
  const holdIntervalMs = Math.max(0, parseInt(document.getElementById("presetHoldInterval")?.value, 10) || 0);
  const button = document.getElementById("presetButtonSelect")?.value || "left";
  const positionMode = document.getElementById("presetPositionSelect")?.value || "cursor";
  const fixedX = parseInt(document.getElementById("presetFixedX")?.value, 10) || 100;
  const fixedY = parseInt(document.getElementById("presetFixedY")?.value, 10) || 100;
  const clickLimit = parseInt(document.getElementById("presetClickLimit")?.value, 10) || 0;
  const startDelaySec = Math.max(0, parseInt(document.getElementById("presetStartDelaySec")?.value, 10) || 0);
  const stopDurationMin = Math.max(0, parseInt(document.getElementById("presetStopDurationMin")?.value, 10) || 0);
  const stopTimeStr = document.getElementById("presetStopTime")?.value || "";
  const repeatMode = document.getElementById("presetRepeatModeSelect")?.value || "unlimited";
  const repeatCount = Math.max(0, parseInt(document.getElementById("presetRepeatCount")?.value, 10) || 0);
  const repeatIntervalMs = Math.max(0, parseInt(document.getElementById("presetRepeatInterval")?.value, 10) || 0);

  ensurePresetsExist();

  if (editId) {
    const idx = currentConfig.presets.findIndex(x => x.id === editId);
    if (idx !== -1) {
      const points = window.SequenceEditor?.getPoints() || [];
      currentConfig.presets[idx] = {
        ...currentConfig.presets[idx],
        name,
        icon,
        target_cps: cps,
        jitter_percent: jitter,
        click_type: clickType,
        button,
        position_mode: positionMode,
        fixed_x: fixedX,
        fixed_y: fixedY,
        click_limit: clickLimit,
        hold_duration_ms: holdDurationMs,
        hold_interval_ms: holdIntervalMs,
        repeat_mode: repeatMode,
        repeat_count: repeatCount,
        repeat_interval_ms: repeatIntervalMs,
        start_delay_ms: startDelaySec * 1000,
        stop_duration_min: stopDurationMin,
        stop_time_str: stopTimeStr,
        points,
      };
    }
  } else {
    const points = window.SequenceEditor?.getPoints() || [];
    const newId = "preset_" + Date.now();
    currentConfig.presets.push({
      id: newId,
      name,
      description: `${cps} CPS | ${clickType}`,
      icon,
      target_cps: cps,
      jitter_percent: jitter,
      jitter_radius_px: 0,
      click_limit: clickLimit,
      button,
      click_type: clickType,
      position_mode: positionMode,
      fixed_x: fixedX,
      fixed_y: fixedY,
      repeat_mode: repeatMode,
      repeat_count: repeatCount,
      repeat_interval_ms: repeatIntervalMs,
      start_delay_ms: startDelaySec * 1000,
      stop_duration_min: stopDurationMin,
      stop_time_str: stopTimeStr,
      hold_duration_ms: holdDurationMs,
      hold_interval_ms: holdIntervalMs,
      is_default: false,
      points,
    });
  }

  renderPresetsGrid();
  saveConfig();
  document.getElementById("presetEditModal")?.classList.add("hidden");
}

async function deletePreset(presetId) {
  ensurePresetsExist();
  currentConfig.presets = currentConfig.presets.filter(x => x.id !== presetId);
  renderPresetsGrid();
  await saveConfig();
}

// Hoisted out of setupPresetListeners(): it touches only `document`, so nesting
// it just re-allocated the closure on every setup pass (oxlint:
// unicorn/consistent-function-scoping). The call sites are pinned by a Rust
// wiring test — keep them verbatim.
const bindBackdropClose = (modalId) => {
  const overlay = document.getElementById(modalId);
  if (!overlay || overlay.dataset.backdropBound === "1") return;
  overlay.dataset.backdropBound = "1";
  overlay.addEventListener("click", (e) => {
    // Only close when clicking on the overlay itself, not on children (the card)
    if (e.target === overlay) overlay.classList.add("hidden");
  });
};

// Hoisted out of setupPresetListeners() for the same reason as bindBackdropClose
// above: it captures nothing from its parent scope, so nesting it re-allocated the
// closure on every setup pass (oxlint: unicorn/consistent-function-scoping).
// `presetBound` keeps each binding idempotent across rerenders. Named explicitly
// rather than `on`: a bare two-letter module-level binding is flagged by no-shadow
// and reads badly next to the DOM event-handler globals.
const bindPresetControl = (id, event, handler) => {
  const element = document.getElementById(id);
  if (!element || element.dataset.presetBound === "1") return;
  element.addEventListener(event, handler);
  element.dataset.presetBound = "1";
};

// Preset listeners setup. Keep all controls in one idempotent setup so a
// rerender or a delayed WebView DOM cannot leave only some buttons active.
function setupPresetListeners() {

  bindPresetControl("createNewPresetBtn", "click", () => openPresetEditModal(null));
  bindPresetControl("saveCurrentAsPresetBtn", "click", () => {
    openPresetEditModal({
      id: "",
      name: "My Config",
      icon: "🎯",
      target_cps: currentConfig.engine.target_cps,
      jitter_percent: currentConfig.engine.jitter_percent,
      click_limit: currentConfig.engine.click_limit,
      button: currentConfig.engine.button,
      click_type: currentConfig.engine.click_type,
      position_mode: currentConfig.engine.position_mode,
      fixed_x: currentConfig.engine.fixed_x,
      fixed_y: currentConfig.engine.fixed_y,
      hold_duration_ms: currentConfig.engine.hold_duration_ms,
      hold_interval_ms: currentConfig.engine.hold_interval_ms,
      repeat_mode: currentConfig.engine.repeat_mode,
      repeat_count: currentConfig.engine.repeat_count,
      repeat_interval_ms: currentConfig.engine.repeat_interval_ms,
      start_delay_ms: currentConfig.engine.start_delay_ms,
      stop_duration_min: currentConfig.engine.stop_duration_min,
      stop_time_str: currentConfig.engine.stop_time_str,
      points: Array.isArray(currentConfig.engine.sequence_points) ? JSON.parse(JSON.stringify(currentConfig.engine.sequence_points)) : []
    });
  });
  bindPresetControl("presetSaveBtn", "click", () => void savePresetFromModal());
  bindPresetControl("presetCancelBtn", "click", () => {
    document.getElementById("presetEditModal")?.classList.add("hidden");
  });
  bindPresetControl("inspectCloseBtn", "click", () => {
    document.getElementById("presetInspectModal")?.classList.add("hidden");
  });

  // --- Escape key: close any open preset modal ---
  // Use a named handler so it is idempotent (won't stack on hot-reload).
  if (!window._presetEscapeHandlerBound) {
    window._presetEscapeHandlerBound = true;
    document.addEventListener("keydown", (e) => {
      if (e.key !== "Escape") return;
      const editModal    = document.getElementById("presetEditModal");
      const inspectModal = document.getElementById("presetInspectModal");
      const onbModal     = document.getElementById("onboardingModal");
      if (editModal    && !editModal.classList.contains("hidden"))    editModal.classList.add("hidden");
      if (inspectModal && !inspectModal.classList.contains("hidden")) inspectModal.classList.add("hidden");
      if (onbModal     && !onbModal.classList.contains("hidden"))     onbModal.classList.add("hidden");
    });
  }

  // --- Backdrop click: click on the overlay dim area (not the card) to close ---
  // (bindBackdropClose itself lives at module scope — see above.)
  bindBackdropClose("presetEditModal");
  bindBackdropClose("presetInspectModal");

  bindPresetControl("exportPresetsBtn", "click", () => {
    try {
      ensurePresetsExist();
      const json = JSON.stringify(currentConfig.presets, null, 2);
      const blob = new Blob([json], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = `nanoclick_presets_${Date.now()}.json`;
      anchor.style.display = "none";
      document.body.appendChild(anchor);
      anchor.click();
      anchor.remove();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
      console.log(`[Presets] exported ${currentConfig.presets.length} preset(s)`);
    } catch (err) {
      console.error("[Presets] export failed:", err);
      alert(getI18nText("dialog_alert_export_presets_fail", {}, "Failed to export presets."));
    }
  });

  bindPresetControl("importPresetsBtn", "click", () => {
    const fileInput = document.createElement("input");
    fileInput.type = "file";
    fileInput.accept = ".json,application/json";
    fileInput.addEventListener("change", async (event) => {
      const file = event.target.files?.[0];
      if (!file) return;
      try {
        const parsed = JSON.parse(await file.text());
        const list = Array.isArray(parsed) ? parsed : parsed?.presets;
        if (!Array.isArray(list)) throw new Error("expected an array of presets");

        const imported = list
          .filter(p => p && typeof p === "object" && String(p.name || "").trim() && Number.isFinite(Number(p.target_cps)))
          .map(p => ({
            id: `preset_${Date.now()}_${Math.floor(Math.random() * 100000)}`,
            name: String(p.name).trim(),
            description: String(p.description || `${p.target_cps} CPS`),
            icon: p.icon || "🎯",
            target_cps: Math.max(1, Math.min(160, Number(p.target_cps))),
            jitter_percent: Math.max(0, Math.min(30, Number(p.jitter_percent) || 0)),
            click_limit: Math.max(0, parseInt(p.click_limit, 10) || 0),
            button: p.button || "left",
            click_type: p.click_type || "single",
            position_mode: p.position_mode || "cursor",
            fixed_x: Number.isFinite(Number(p.fixed_x)) ? Number(p.fixed_x) : 100,
            fixed_y: Number.isFinite(Number(p.fixed_y)) ? Number(p.fixed_y) : 100,
            hold_duration_ms: Number(p.hold_duration_ms) || 500,
            hold_interval_ms: Number(p.hold_interval_ms) || 1000,
            repeat_mode: p.repeat_mode || "unlimited",
            repeat_count: Math.max(0, parseInt(p.repeat_count, 10) || 0),
            repeat_interval_ms: Math.max(0, Number(p.repeat_interval_ms) || 1000),
            start_delay_ms: Math.max(0, Number(p.start_delay_ms) || 0),
            stop_duration_min: Math.max(0, parseInt(p.stop_duration_min, 10) || 0),
            stop_time_str: p.stop_time_str || "",
            is_default: false
          }));

        if (imported.length === 0) throw new Error("no valid presets");
        ensurePresetsExist();
        currentConfig.presets.push(...imported);
        renderPresetsGrid();
        await saveConfig();
        alert(getI18nText("dialog_alert_import_presets_success", { count: imported.length }, `Successfully imported ${imported.length} presets`));
      } catch (err) {
        console.error("[Presets] import failed:", err);
        alert(getI18nText("dialog_alert_import_presets_invalid", {}, "Invalid format or empty presets file."));
      }
    });
    fileInput.click();
  });
}

setupPresetListeners();

// ── TIMER ACTIVE BADGE ──────────────────────────────────────
function updateTimerBadge() {
  const badge = document.getElementById("timerActiveBadge");
  if (!badge) return;
  const dur = parseInt(document.getElementById("stopDurationInput")?.value, 10) || 0;
  const timeVal = document.getElementById("stopTimeInput")?.value || "";
  if (dur > 0 || timeVal) {
    badge.classList.remove("hidden");
  } else {
    badge.classList.add("hidden");
  }
}

document.getElementById("stopDurationInput")?.addEventListener("input", updateTimerBadge);
document.getElementById("stopTimeInput")?.addEventListener("input", updateTimerBadge);
updateTimerBadge();

document.getElementById("presetCpsRange")?.addEventListener("input", (e) => {
  // Mirror to the text badge and persist into currentConfig so the click loop sees it.
  const val = document.getElementById("presetCpsVal");
  const cps = parseFloat(e.target.value);
  const safeCps = isNaN(cps) ? 29 : cps;
  if (val) val.textContent = safeCps;
  if (currentConfig?.engine) currentConfig.engine.target_cps = safeCps;
  saveConfig();
});
document.getElementById("presetJitterRange")?.addEventListener("input", (e) => {
  const val = document.getElementById("presetJitterVal");
  const jitter = parseFloat(e.target.value);
  const safeJitter = isNaN(jitter) ? 0 : Math.max(0, Math.min(30, jitter));
  if (val) val.textContent = safeJitter;
  if (currentConfig?.engine) currentConfig.engine.jitter_percent = safeJitter;
  saveConfig();
});
document.getElementById("presetPositionSelect")?.addEventListener("change", (e) => {
  const coordRow = document.getElementById("presetFixedCoordRow");
  if (coordRow) coordRow.classList.toggle("hidden", e.target.value !== "fixed");
});

// ── ADVANCED TIMERS LOGIC ───────────────────────────────────
let startDelayTimer = null;
let stopDurationTimer = null;
let stopTimeTimer = null;

function clearAutoTimers() {
  if (startDelayTimer) { clearInterval(startDelayTimer); startDelayTimer = null; }
  if (stopDurationTimer) { clearTimeout(stopDurationTimer); stopDurationTimer = null; }
  if (stopTimeTimer) { clearTimeout(stopTimeTimer); stopTimeTimer = null; }
}

// Cancel an in-flight Start countdown (if user clicks Start, starts a 5s delay,
// then clicks the toggle again before countdown completes — without this,
// pointerEvents stays "none" and the button is dead).
function cancelStartDelay() {
  if (startDelayTimer) {
    clearInterval(startDelayTimer);
    startDelayTimer = null;
    if (toggleBtn) toggleBtn.style.pointerEvents = "auto";
    setAutomationButtonLabel("start");
    console.log("[cancelStartDelay] countdown aborted, button restored");
  }
}

async function executeStartAutomation() {
  const op = stage("Toggle");
  try {
    // ── STEP 1: Verify UI is in a togglable state ─────────────────────
    await op.run("check-active-mode", () => {
      if (currentConfig.active_mode === "work") {
        op.log("blocked", "WORK mode — cannot toggle");
        return false; // blocks the stage
      }
      return `mode=${currentConfig.active_mode} ✓`;
    });

    // ── STEP 2: Lock the button (debounce window) ─────────────────────
    if (!isRunning) {
      await op.run("apply-button-lock", () => {
        isButtonLocked = true;
        if (toggleBtn) toggleBtn.style.pointerEvents = "none";
        setAutomationButtonLabel("running_locked");
        return `pointerEvents=none for ${parseInt(guiLockDelayInput?.value, 10) || 1500}ms`;
      });

      await op.run("start-gui-lock-timer", () => {
        const lockDuration = parseInt(guiLockDelayInput?.value, 10) || 1500;
        if (guiLockTimer) clearTimeout(guiLockTimer);
        guiLockTimer = setTimeout(() => {
          isButtonLocked = false;
          if (toggleBtn) toggleBtn.style.pointerEvents = "auto";
          if (isRunning) setAutomationButtonLabel("stop");
          logCall("→STAGE✓", "[Toggle] gui-lock released");
        }, lockDuration);
        return "timer scheduled";
      });

      // ── STEP 3: Optional stop-duration timer (auto-stop after N min) ─
      await op.run("setup-stop-duration", () => {
        const stopMin = parseInt(document.getElementById("stopDurationInput")?.value, 10) || 0;
        if (stopDurationTimer) clearTimeout(stopDurationTimer);
        if (stopMin > 0) {
          stopDurationTimer = setTimeout(async () => {
            const sub = stage("StopDuration");
            await sub.run("check-still-running", () => isRunning || false);
            const active = await invoke("toggle_autoclicker");
            await sub.run("rust-toggle", () => `returned active=${active}`);
            setRunningState(active);
            sub.ok("auto-stopped after duration");
          }, stopMin * 60 * 1000);
          return `${stopMin} min until auto-stop`;
        }
        return "no stop-duration set";
      });

      // ── STEP 4: Optional stop-time timer (HH:MM wall clock) ─────────
      await op.run("setup-stop-time", () => {
        const stopTimeVal = document.getElementById("stopTimeInput")?.value;
        if (!stopTimeVal) return "no stop-time set";
        const [targetH, targetM] = stopTimeVal.split(":").map(Number);
        const now = new Date();
        const targetDate = new Date(now.getFullYear(), now.getMonth(), now.getDate(), targetH, targetM, 0);
        if (targetDate <= now) targetDate.setDate(targetDate.getDate() + 1);
        const msUntilTarget = targetDate.getTime() - now.getTime();

        if (stopTimeTimer) clearTimeout(stopTimeTimer);
        stopTimeTimer = setTimeout(async () => {
          const sub = stage("StopTime");
          await sub.run("check-still-running", () => isRunning || false);
          const active = await invoke("toggle_autoclicker");
          await sub.run("rust-toggle", () => `returned active=${active}`);
          setRunningState(active);
          sub.ok("auto-stopped at wall-clock time");
        }, msUntilTarget);
        return `stops at ${stopTimeVal} (in ${(msUntilTarget / 60000).toFixed(1)} min)`;
      });
    } else {
      // Stop path — clear all auto timers first
      await op.run("clear-auto-timers", () => {
        clearAutoTimers();
        return "stop-duration + stop-time timers cancelled";
      });
    }

    // ── STEP 5: Verify the click wasn't eaten by another handler ──────
    await op.run("verify-rust-reachable", () => {
      if (typeof invoke !== "function") return false;
      return "invoke function available";
    });

    // ── STEP 6: Talk to Rust — the actual toggle ─────────────────────
    const active = await invoke("toggle_autoclicker");
    await op.run("rust-toggle-autoclicker", () => `Rust says active=${active}`);

    // ── STEP 7: Mirror Rust state to UI ──────────────────────────────
    await op.run("apply-ui-state", () => {
      setRunningState(active);
      return active ? "UI → RUNNING" : "UI → IDLE";
    });

    op.ok("toggle complete");
  } catch (err) {
    op.fail("toggle aborted", err);
    console.error("[Toggle] aborted:", err);
  }
}

// ── TOGGLE AUTOCLICKER ──────────────────────────────────────
if (toggleBtn) {
  dbg("toggleBtn found — wiring click listener");
  let toggleBusy = false;

  toggleBtn.addEventListener("click", async () => {
    dbg("toggleBtn CLICKED — isRunning:", isRunning, "isButtonLocked:", isButtonLocked, "toggleBusy:", toggleBusy);
    const clickOp = stage("Click");
    try {

      // ── STAGE 1: Was the click delivered to the right element? ────────
      await clickOp.run("click-registered", () => {
        if (!toggleBtn) return false;
        const rect = toggleBtn.getBoundingClientRect();
        return `toggleBtn at (${Math.round(rect.left)},${Math.round(rect.top)}) ${Math.round(rect.width)}×${Math.round(rect.height)}`;
      });

      // ── STAGE 2: Is anyone else holding the button locked? ────────────
      await clickOp.run("button-not-locked", () => {
        if (isButtonLocked) {
          clickOp.log("blocked", "guiLock active — accidental click suppressed");
          return false;
        }
        return `isButtonLocked=${isButtonLocked}`;
      });

      // ── STAGE 3: Is a previous IPC call still pending? ───────────────
      await clickOp.run("not-busy", () => {
        if (toggleBusy) {
          clickOp.log("debounced", "previous toggle still in flight — click ignored");
          return false;
        }
        return `toggleBusy=${toggleBusy}`;
      });

      // ── STAGE 4: Did the user intend to cancel a start-delay? ────────
      await clickOp.run("check-start-delay", () => {
        if (startDelayTimer) {
          cancelStartDelay();
          clickOp.log("cancelled", "start-delay countdown aborted by second click");
          return "cancelled-countdown";
        }
        return "no countdown in flight";
      });

      const startDelaySec = currentDelayUnit === "ms"
        ? Math.ceil((parseInt(document.getElementById("startDelayInput")?.value, 10) || 0) / 1000)
        : (parseInt(document.getElementById("startDelayInput")?.value, 10) || 0);

      // ── STAGE 5: Was a start-delay configured? ──────────────────────
      if (startDelaySec > 0 && !isRunning) {
        await clickOp.run("begin-countdown", () => {
          let remaining = startDelaySec;
          setAutomationButtonLabel("starting_in", remaining);
          if (toggleBtn) toggleBtn.style.pointerEvents = "none";

          startDelayTimer = setInterval(async () => {
            remaining--;
            if (remaining > 0) {
              setAutomationButtonLabel("starting_in", remaining);
            } else {
              clearInterval(startDelayTimer);
              startDelayTimer = null;
              if (toggleBtn) toggleBtn.style.pointerEvents = "auto";
              const sub = stage("CountdownFire");
              await sub.run("countdown-elapsed", () => `${startDelaySec}s passed`);
              await executeStartAutomation();
              sub.ok("started after countdown");
            }
          }, 1000);
          return `countdown ${startDelaySec}s — release button to abort`;
        });
        clickOp.ok(`start-delay queued (${startDelaySec}s)`);
        return;
      }

      // ── STAGE 6: Run the toggle now (no countdown) ───────────────────
      await clickOp.run("acquire-busy-lock", () => {
        toggleBusy = true;
        return "busy flag set";
      });

      try {
        await clickOp.run("execute-toggle", () => executeStartAutomation());

        // Brief settle window so the Rust worker's status-update can land
        // and update isRunning BEFORE we release the busy lock. Without this,
        // a too-fast second click could fire while Rust still says "not running".
        await new Promise((r) => setTimeout(r, 80));
        await clickOp.run("settle-state", () => `isRunning=${isRunning} after toggle`);
      } finally {
        toggleBusy = false;
        clickOp.log("busy-released", "ready for next click");
      }

      clickOp.ok("toggle dispatched");
    } catch (err) {
      clickOp.fail("click aborted", err);
      console.error("[Click] aborted:", err);
    }
  });
}

// ── ONBOARDING ──────────────────────────────────────────────
let selectedOnboardingPreset = { id: "boost", cps: 15.0, jitter: 5.0 };
let selectedOnboardingLang = "en";
let onboardingTimer = null;
let onboardingCountdown = 2;

function detectSystemLanguage() {
  const navLang = (typeof navigator !== "undefined" && (navigator.language || navigator.userLanguage || "en")).toLowerCase();
  if (navLang.startsWith("uk") || navLang.startsWith("ua")) return "ua";
  if (navLang.startsWith("ru")) return "ru";
  return "en";
}

function updateOnboardingLangButtons(lang) {
  const langGrid = document.getElementById("onboardingLangGrid");
  if (!langGrid) return;
  langGrid.querySelectorAll(".onboarding-lang-btn").forEach(btn => {
    btn.classList.toggle("active", btn.getAttribute("data-lang") === lang);
  });
}

function showOnboardingMatrix() {
  if (!onboardingModal) return;
  onboardingModal.classList.remove("hidden");
  const card = onboardingModal.querySelector(".onboarding-matrix-card");
  if (card) card.classList.remove("fade-out");

  const initialLang = currentConfig?.ui?.language || detectSystemLanguage();
  selectedOnboardingLang = initialLang;
  updateOnboardingLangButtons(initialLang);

  if (window.I18nEngine && window.I18nEngine.currentLang !== initialLang) {
    window.I18nEngine.setLanguage(initialLang);
  }

  selectedOnboardingPreset = { id: "boost", cps: 15.0, jitter: 5.0 };
  const presetGrid = document.getElementById("onboardingPresetGrid");
  if (presetGrid) {
    presetGrid.querySelectorAll(".onboarding-preset-card").forEach(c => {
      c.classList.toggle("active", c.dataset.preset === "boost");
    });
  }

  const startBtn = document.getElementById("onboardingBtn");
  const startBtnText = document.getElementById("onboardingBtnText");
  const consentCb = document.getElementById("onboardingConsentCheckbox");
  if (startBtn) {
    startBtn.disabled = true;
    onboardingCountdown = 2;
    if (startBtnText) {
      startBtnText.textContent = getI18nText("onboarding_btn_start_timer", { n: onboardingCountdown }, `🚀 ПОЧАТИ РОБОТУ (${onboardingCountdown})`);
    }
    if (onboardingTimer) clearInterval(onboardingTimer);
    onboardingTimer = setInterval(() => {
      onboardingCountdown--;
      if (onboardingCountdown > 0) {
        if (startBtnText) {
          startBtnText.textContent = getI18nText("onboarding_btn_start_timer", { n: onboardingCountdown }, `🚀 ПОЧАТИ РОБОТУ (${onboardingCountdown})`);
        }
      } else {
        clearInterval(onboardingTimer);
        onboardingTimer = null;
        if (consentCb ? consentCb.checked : true) {
          startBtn.disabled = false;
        }
        if (startBtnText) {
          startBtnText.textContent = getI18nText("onboarding_btn_start", {}, "🚀 ПОЧАТИ РОБОТУ");
        }
      }
    }, 1000);
  }
}

function wireOnboardingModal() {
  const langGrid = document.getElementById("onboardingLangGrid");
  if (langGrid && !langGrid._wired) {
    langGrid._wired = true;
    langGrid.querySelectorAll(".onboarding-lang-btn").forEach(btn => {
      btn.addEventListener("click", () => {
        const lang = btn.getAttribute("data-lang");
        if (!lang) return;
        selectedOnboardingLang = lang;
        updateOnboardingLangButtons(lang);
        if (languageSelect) languageSelect.value = lang;
        if (currentConfig?.ui) currentConfig.ui.language = lang;

        const card = onboardingModal?.querySelector(".onboarding-matrix-card");
        if (card) {
          card.classList.add("lang-switch-anim");
          setTimeout(() => card.classList.remove("lang-switch-anim"), 200);
        }

        if (window.I18nEngine) {
          window.I18nEngine.setLanguage(lang);
        }

        const startBtnText = document.getElementById("onboardingBtnText");
        if (startBtnText) {
          if (onboardingCountdown > 0) {
            startBtnText.textContent = getI18nText("onboarding_btn_start_timer", { n: onboardingCountdown }, `🚀 ПОЧАТИ РОБОТУ (${onboardingCountdown})`);
          } else {
            startBtnText.textContent = getI18nText("onboarding_btn_start", {}, "🚀 ПОЧАТИ РОБОТУ");
          }
        }
      });
    });
  }

  const presetGrid = document.getElementById("onboardingPresetGrid");
  if (presetGrid && !presetGrid._wired) {
    presetGrid._wired = true;
    presetGrid.querySelectorAll(".onboarding-preset-card").forEach(card => {
      card.addEventListener("click", () => {
        presetGrid.querySelectorAll(".onboarding-preset-card").forEach(c => c.classList.remove("active"));
        card.classList.add("active");
        selectedOnboardingPreset = {
          id: card.dataset.preset || "boost",
          cps: parseFloat(card.dataset.cps) || 15.0,
          jitter: parseFloat(card.dataset.jitter) || 5.0
        };
      });
    });
  }

  const consentCb = document.getElementById("onboardingConsentCheckbox");
  const startBtn = document.getElementById("onboardingBtn");
  if (consentCb && !consentCb._wired) {
    consentCb._wired = true;
    consentCb.addEventListener("change", () => {
      if (startBtn) {
        startBtn.disabled = !consentCb.checked || (onboardingCountdown > 0);
      }
    });
  }

  const licenseLink = document.getElementById("onboardingLicenseLink");
  if (licenseLink && !licenseLink._wired) {
    licenseLink._wired = true;
    licenseLink.addEventListener("click", async () => {
      try {
        if (window.__TAURI__?.core?.invoke) {
          await window.__TAURI__.core.invoke("open_external_url", { url: "https://github.com/Ar3ss12/nanoclick/blob/main/LICENSE.md" });
          return;
        }
      } catch (_) {}
      try { window.open("https://github.com/Ar3ss12/nanoclick/blob/main/LICENSE.md", "_blank", "noopener"); } catch (_) {}
    });
  }

  if (startBtn && !startBtn._wired) {
    startBtn._wired = true;
    startBtn.addEventListener("click", async () => {
      if (startBtn.disabled) return;
      dbg("Completing onboarding with preset:", selectedOnboardingPreset, "lang:", selectedOnboardingLang);

      const card = onboardingModal?.querySelector(".onboarding-matrix-card");
      if (card) card.classList.add("fade-out");

      if (currentConfig) {
        if (!currentConfig.engine) currentConfig.engine = {};
        if (!currentConfig.ui) currentConfig.ui = {};
        currentConfig.engine.target_cps = selectedOnboardingPreset.cps;
        currentConfig.engine.jitter_percent = selectedOnboardingPreset.jitter;
        currentConfig.ui.language = selectedOnboardingLang;
      }

      try {
        const updatedConfig = await invoke("complete_onboarding");
        if (updatedConfig) {
          updatedConfig.engine.target_cps = selectedOnboardingPreset.cps;
          updatedConfig.engine.jitter_percent = selectedOnboardingPreset.jitter;
          updatedConfig.ui.language = selectedOnboardingLang;
          updateUiFromConfig(updatedConfig);
        }
        await saveConfig();
      } catch (err) {
        console.warn("complete_onboarding invoke failed:", err);
      } finally {
        setTimeout(() => {
          if (onboardingModal) onboardingModal.classList.add("hidden");
          if (card) card.classList.remove("fade-out");
        }, 200);
      }
    });
  }

  const closeBtn = document.getElementById("onboardingCloseBtn");
  if (closeBtn && !closeBtn._wired) {
    closeBtn._wired = true;
    closeBtn.addEventListener("click", () => {
      if (onboardingModal) onboardingModal.classList.add("hidden");
    });
  }
}
wireOnboardingModal();

// ── STATE DISPLAY ───────────────────────────────────────────
let _lastRunningStateActive = null;
let _lastRunningStateStatusText = null;
let _lastRunningStateLocked = null;

function setRunningState(active, statusText = "") {
  isRunning = active;
  if (active === _lastRunningStateActive && statusText === _lastRunningStateStatusText && isButtonLocked === _lastRunningStateLocked) {
    return;
  }
  _lastRunningStateActive = active;
  _lastRunningStateStatusText = statusText;
  _lastRunningStateLocked = isButtonLocked;

  if (!active) {
    clearAutoTimers();
  }
  if (currentConfig.active_mode === "work") {
    if (toggleBtn) {
      toggleBtn.disabled = true;
      toggleBtn.classList.add("disabled-mode");
      setAutomationButtonLabel("disabled_work");
    }
    return;
  }

  if (active) {
    if (statusBadge) {
      statusBadge.textContent = "RUNNING";
      statusBadge.className = "status-badge running";
    }
    if (toggleBtn) {
      toggleBtn.disabled = false;
      toggleBtn.className = "start-btn stop";
    }
    if (isButtonLocked) {
      // Lock still active — keep pointerEvents = "none" so the user can't
      // accidentally double-click and stop the clicker during the safety window.
      // The guiLockTimer will release the lock once it fires.
      setAutomationButtonLabel("running_locked");
      if (toggleBtn) toggleBtn.style.pointerEvents = "none";
    } else {
      // Lock already released (or never set). Make sure pointerEvents is "auto"
      // — it could be stuck on "none" from a previous run if the lock timer
      // fired during a `setRunningState(false)` window.
      if (toggleBtn) toggleBtn.style.pointerEvents = "auto";
      setAutomationButtonLabel("stop");
    }
  } else {
    // Stop path — full reset of all lock state. Without this, a previous run's
    // pending guiLockTimer / stale pointerEvents would carry over to the next
    // Start attempt, leaving the button visually dead.
    if (guiLockTimer) { clearTimeout(guiLockTimer); guiLockTimer = null; }
    isButtonLocked = false;
    if (toggleBtn) {
      toggleBtn.disabled = false;
      toggleBtn.style.pointerEvents = "auto";  // ← KEY: always restore on stop
      toggleBtn.className = "start-btn";
    }
    if (statusBadge) {
      statusBadge.textContent = statusText || "IDLE";
      statusBadge.className = "status-badge idle";
    }
    setAutomationButtonLabel("start");
  }
}

// ── CLICK TOOLTIP ────────────────────────────────────────────────────────────
// Shows/hides a stats popup when hovering or clicking the ⓘ info icon on the
// click counter metric card.
// Design Note:
// - Appears IMMEDIATELY on hover (mouseenter) over ⓘ or the metric card top.
// - Supports clicking to pin/unpin the tooltip open.
// - Updates current figures (This run, Session clicks, Settings runs) instantaneously.
let _tooltipSetupDone = false;
let _currentRunClicks = 0;
let _tooltipHideTimeout = null;
let _tooltipPinned = false;

function showClickTooltip() {
  if (!clickStatsTooltip) return;
  if (_tooltipHideTimeout) {
    clearTimeout(_tooltipHideTimeout);
    _tooltipHideTimeout = null;
  }
  clickStatsTooltip.style.display = "block";
  updateClickTooltipValues();
}

function hideClickTooltip(delayMs = 0) {
  if (!clickStatsTooltip || _tooltipPinned) return;
  if (_tooltipHideTimeout) clearTimeout(_tooltipHideTimeout);
  if (delayMs > 0) {
    _tooltipHideTimeout = setTimeout(() => {
      if (!_tooltipPinned && clickStatsTooltip) {
        clickStatsTooltip.style.display = "none";
      }
      _tooltipHideTimeout = null;
    }, delayMs);
  } else {
    clickStatsTooltip.style.display = "none";
  }
}

function setupClickTooltip() {
  if (!clickMetricInfoBtn || !clickStatsTooltip || _tooltipSetupDone) return;
  _tooltipSetupDone = true;

  // 1. Hover on ⓘ button
  clickMetricInfoBtn.addEventListener("mouseenter", () => showClickTooltip());
  clickMetricInfoBtn.addEventListener("mouseleave", () => hideClickTooltip(250));

  // 2. Click on ⓘ button (toggles pinned state)
  clickMetricInfoBtn.addEventListener("click", (e) => {
    e.stopPropagation();
    _tooltipPinned = !_tooltipPinned;
    if (_tooltipPinned) {
      showClickTooltip();
      clickMetricInfoBtn.style.background = "rgba(6,182,212,0.3)";
    } else {
      hideClickTooltip(0);
      clickMetricInfoBtn.style.background = "rgba(6,182,212,0.1)";
    }
  });

  // 3. Hover on the tooltip itself (prevents auto-closing while reading)
  clickStatsTooltip.addEventListener("mouseenter", () => showClickTooltip());
  clickStatsTooltip.addEventListener("mouseleave", () => hideClickTooltip(200));

  // Stop clicks inside popup from closing it
  clickStatsTooltip.addEventListener("click", (e) => e.stopPropagation());

  // 4. Click outside to dismiss pinned tooltip
  document.addEventListener("click", () => {
    if (_tooltipPinned) {
      _tooltipPinned = false;
      clickMetricInfoBtn.style.background = "rgba(6,182,212,0.1)";
      hideClickTooltip(0);
    }
  });

  // Initial population of values so numbers are ready immediately
  updateClickTooltipValues();
}

/**
 * Refreshes values inside the tooltip popup instantaneously.
 * Uses _currentRunClicks, StatsEngine session clicks, and _runsThisSettings.
 */
/** Thousands separator for the click tooltip. Module scope → allocated once. */
const formatCount = (n) => {
  const v = Number(n) || 0;
  return v > 999 ? v.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",") : String(v);
};

function updateClickTooltipValues(runClicks = null) {
  if (typeof runClicks === "number") {
    _currentRunClicks = runClicks;
  }
  if (!clickStatsTooltip || clickStatsTooltip.style.display === "none") {
    return;
  }
  const eng = window.StatsEngine;
  const activeRun = typeof _currentRunClicks === "number" ? _currentRunClicks : 0;
  // Session clicks must NEVER be less than the current run clicks.
  const rawSessionClicks = Number(eng?.state?.sessionClicks) || 0;
  const sessionClicksVal = Math.max(activeRun, rawSessionClicks);

  // Total clicks across all runs using this exact configuration snapshot.
  // If settings never changed in this session, all session clicks belong to this config.
  const effectiveSettingsClicks = !_hasChangedSettings ? sessionClicksVal : _clicksThisSettings;
  const settingsClicksVal = Math.max(activeRun, effectiveSettingsClicks);

  if (tooltipRunClicks)        tooltipRunClicks.textContent        = formatCount(activeRun);
  if (tooltipSessionClicks)    tooltipSessionClicks.textContent    = formatCount(sessionClicksVal);
  if (tooltipSettingsClicks)   tooltipSettingsClicks.textContent   = formatCount(settingsClicksVal);
  if (tooltipSettingsRuns)     tooltipSettingsRuns.textContent     = formatCount(_runsThisSettings);
}

// ── VISUAL CLICK RIPPLE (DELEGATED TO MODULAR OVERLAY) ────────────────────────
// Click ripples are rendered in a dedicated transparent fullscreen click-through
// window (src/overlay.html, src/overlay.js) driven by hardware coordinates from Rust.

// ── IPC LISTENER ────────────────────────────────────────────
// Rust emits status-update on every click (~29/s at default CPS). That's
// useful for the live click counter but produces ~30 log lines per second.
// Use listenSilent (no auto per-event log) and throttle manually — UI
// updates on every tick, log only at meaningful boundaries.
let _lastStatusLogAt = 0;
let _lastStatusLogActive = null;
let _lastStatusLogClicks = -1;
let _lastStatsRenderAt = 0;
// Snapshot of key engine params taken at each START — used to detect if
// settings changed between runs (resets _runsThisSettings counter).
let _settingsSnapshot = null;

/** Returns a compact string of the engine params that define a "config identity" */
function _configSignature(cfg) {
  if (!cfg?.engine) return "";
  const e = cfg.engine;
  return [
    e.target_cps, e.button, e.click_type, e.position_mode,
    e.fixed_x, e.fixed_y, e.click_limit, e.repeat_mode,
    e.hold_duration_ms, e.jitter_percent,
  ].join("|");
}

listenSilent("status-update", (event) => {
  try {
    const payload = event?.payload || {};
    const { active, mode, clicks_done, cps, status_text } = payload;

    // 1. Core UI state update (lightweight, mandatory)
    const n = clicks_done || 0;
    if (mode) {
      const modeChanged = mode !== currentConfig.active_mode;
      if (modeChanged) {
        setModeDisplay(mode, true);
        saveConfig();
      }
    }
    setRunningState(active, status_text);
    if (clickCounter) {
      clickCounter.textContent = n > 999 ? n.toString().replace(/\B(?=(\d{3})+(?!\d))/g, ",") : String(n);
    }

    // 2. Track transitions (idle <-> active) & config identity
    const stateChanged = active !== _lastStatusLogActive;
    if (stateChanged) {
      _lastStatusLogActive = active; // Always sync immediately
      if (active) {
        const sig = _configSignature(currentConfig);
        if (_settingsSnapshot !== null && _settingsSnapshot !== sig) {
          // Settings changed between runs → start fresh count for this configuration
          _hasChangedSettings = true;
          _runsThisSettings = 1;
          _clicksThisSettings = 0;
        } else {
          // Same configuration reused → increment run counter, clicks continue accumulating
          _runsThisSettings += 1;
        }
        _settingsSnapshot = sig;
        _lastSettingsRunClicksDone = 0;
      } else {
        // Run ended: capture final click delta before resetting baseline
        const finalDelta = n > _lastSettingsRunClicksDone ? (n - _lastSettingsRunClicksDone) : 0;
        if (finalDelta > 0) {
          _clicksThisSettings += finalDelta;
        }
        _lastSettingsRunClicksDone = 0;
      }
    }

    // Accumulate clicks for current configuration across all runs this app session
    if (active) {
      const deltaClicks = n > _lastSettingsRunClicksDone ? (n - _lastSettingsRunClicksDone) : 0;
      _lastSettingsRunClicksDone = n;
      if (deltaClicks > 0) {
        _clicksThisSettings += deltaClicks;
      }
    }

    // 3. Statistics calculation & disk save batching (delegated to StatsEngine)
    if (window.StatsEngine) {
      window.StatsEngine.recordSessionTick(active, clicks_done, cps, currentConfig, saveConfigThrottled);
    }

    // 4. Click tooltip live update
    updateClickTooltipValues(n);

    // 5. Diagnostic logging: skip unless (a) state changed, (b) 50 clicks boundary, or (c) >= 1000ms
    const nowPerf = performance.now();
    const milestoneCrossed = Math.floor((clicks_done || 0) / 50) !== Math.floor(_lastStatusLogClicks / 50);
    const intervalElapsed = nowPerf - _lastStatusLogAt >= 1000;
    if (stateChanged || milestoneCrossed || intervalElapsed) {
      logCall("←EVT•",
        `status-update [active=${active} mode=${mode} clicks=${clicks_done} cps=${cps?.toFixed?.(1) ?? cps} text="${status_text}"]`, "");
      _lastStatusLogAt = nowPerf;
      _lastStatusLogClicks = clicks_done || 0;
    }
  } catch (err) {
    origError.call(console, "[status-update error]", err);
    dbg("STATUS UPDATE ERROR:", err?.message || String(err));
  }
});

// ── FOCUS LOSS AUTO-PAUSE (Smart Guard) ──────────────────────────
// The Rust click loop stopped the run because the foreground app changed
// mid-run (Alt+Tab, click-away, Win key, system toast). The final
// status-update (active=false) already flipped the UI to idle — this
// listener only explains WHY. Payload = the exe that took focus (or null).
listen("focus-loss-paused", (event) => {
  try {
    const exe = typeof event?.payload === "string" && event.payload ? event.payload : null;
    const base = getI18nText("focus_loss_paused_notify", {}, "Auto-paused: window focus changed");
    showToast("⏸ " + base + (exe ? ` → ${exe}` : ""), "warn");
  } catch (err) {
    dbg("focus-loss-paused handler error:", err?.message || String(err));
  }
});

// ── AUTOMATION TAB ───────────────────────────────────────────────
// v3.2 — Record / Build / My Macros. Two paths to the same Action Engine.
let isRecording = false;
let recordActionCount = 0;

// ── TRANSLUCENT RECORDING OVERLAY ─────────────────────────────────────────
function showRecordingOverlay(on) {
  const ov = document.getElementById("recordingOverlay");
  if (!ov) return;
  ov.style.display = on ? "block" : "none";
}
function setRecordingActionCount(n) {
  const el = document.getElementById("recordingOverlayCount");
  if (el) el.textContent = `${n} actions`;
}
// Poll the macro list count to keep the overlay number roughly synced
// (the backend counts normalized events; this only shows a non-zero visible total).
setInterval(() => {
  if (!isRecording) return;
  setRecordingActionCount(recordActionCount);
}, 500);

// ── EMPTY MACRO TEMPLATE ──────────────────────────────────────────────
function emptyMacro(name) {
  return {
    id: "m_" + Date.now().toString(16) + Math.floor(Math.random() * 0xffff).toString(16),
    name: name || "New macro",
    icon: "✨",
    actions: [],
    // Rust uses an internally tagged enum: { mode: "once" }.
    repeat: { mode: "once" },
    enabled: [],
    created_at: Date.now(),
    updated_at: Date.now(),
  };
}

// Keep the browser/editor representation compatible with Rust's RepeatMode.
// Older UI code used a plain string such as "once", which Tauri rejects.
function normalizeMacroForBackend(macro) {
  if (!macro || typeof macro !== "object") return macro;
  const repeat = macro.repeat;
  if (typeof repeat === "string") {
    const mode = repeat.toLowerCase();
    if (mode === "times" || mode === "repeat") {
      macro.repeat = { mode: "times", count: Math.max(0, Number(macro.repeat_count) || 0) };
    } else if (mode === "forever" || mode === "unlimited" || mode === "until_stopped") {
      macro.repeat = { mode: "until_stopped" };
    } else {
      macro.repeat = { mode: "once" };
    }
  } else if (!repeat || typeof repeat !== "object" || typeof repeat.mode !== "string") {
    macro.repeat = { mode: "once" };
  }
  return macro;
}

async function saveMacro(m) {
  m.updated_at = Date.now();
  normalizeMacroForBackend(m);
  await invoke("save_macro", { m });
}

// ── BUILD MODE ───────────────────────────────────────────────────────
function promptMacroName(defaultVal, onConfirm) {
  const existing = document.getElementById("macroPromptBackdrop");
  if (existing) existing.remove();

  const html = `
    <div class="modal-backdrop" id="macroPromptBackdrop" style="position:fixed;inset:0;background:rgba(0,0,0,0.65);display:flex;align-items:center;justify-content:center;z-index:10000;">
      <div class="modal nano-prompt-modal" style="background:var(--bg-card);border:1px solid var(--border-cyan);border-radius:10px;padding:20px;box-shadow:0 20px 50px rgba(0,0,0,0.6);">
        <div style="display:flex;align-items:center;gap:8px;margin-bottom:12px;">
          <span style="font-size:20px;">✨</span>
          <h3 style="margin:0;font-size:15px;color:var(--text-bright);">Create New Macro</h3>
        </div>
        <p style="color:var(--text-dim);font-size:12px;margin-bottom:14px;">Enter a unique descriptive name for your automation sequence:</p>
        <input type="text" id="macroPromptNameInput" class="text-input" value="${escapeHtml(defaultVal || "Custom macro")}" style="width:100%;margin-bottom:16px;" placeholder="e.g. Loot Farm / Auto Trade" spellcheck="false" autocomplete="off">
        <div style="display:flex;justify-content:flex-end;gap:8px;">
          <button id="macroPromptCancelBtn" class="preset-modal-btn cancel">Cancel</button>
          <button id="macroPromptCreateBtn" class="preset-modal-btn save">Open Visual Editor</button>
        </div>
      </div>
    </div>`;
  document.body.insertAdjacentHTML("beforeend", html);

  const backdrop = document.getElementById("macroPromptBackdrop");
  const input = document.getElementById("macroPromptNameInput");
  const cancelBtn = document.getElementById("macroPromptCancelBtn");
  const createBtn = document.getElementById("macroPromptCreateBtn");

  const cleanup = () => {
    document.removeEventListener("keydown", onKey);
    backdrop?.remove();
  };

  const submit = () => {
    const val = input.value.trim();
    if (!val) {
      input.focus();
      return;
    }
    cleanup();
    onConfirm(val);
  };

  const onKey = (e) => {
    if (e.key === "Enter") submit();
    if (e.key === "Escape") cleanup();
  };

  document.addEventListener("keydown", onKey);
  cancelBtn.addEventListener("click", cleanup);
  createBtn.addEventListener("click", submit);
  backdrop.addEventListener("click", (e) => {
    if (e.target === backdrop) cleanup();
  });

  input.focus();
  input.select();
}

async function openBuildMode() {
  promptMacroName("Custom macro", async (name) => {
    const m = emptyMacro(name);
    await saveMacro(m);
    await renderMacroList();
    const fresh = (await invoke("list_macros")).find(x => x.id === m.id);
    if (fresh) openVisualEditor(fresh, renderMacroList);
  });
}

// ── ⚡ OPTIMIZE TOGGLE & SEGMENTS ──────────────────────────────────────
let optimizeLevel = "balanced"; // subtle | balanced | aggressive

function setOptimizeLevel(level) {
  if (!["subtle", "balanced", "aggressive"].includes(level)) return;
  optimizeLevel = level;
  refreshOptimizeSegments();
}

function refreshOptimizeSegments() {
  const segs = document.querySelectorAll(".macro-opt-seg");
  segs.forEach(seg => {
    if (seg.dataset.level === optimizeLevel) {
      seg.classList.add("active");
    } else {
      seg.classList.remove("active");
    }
  });

  const descEl = document.getElementById("macroOptDesc");
  if (descEl) {
    const descs = {
      subtle: getI18nText("automation_opt_desc_subtle", {}, "High precision: preserves minor cursor twitches and slow movements (10px)."),
      balanced: getI18nText("automation_opt_desc_balanced", {}, "Balanced vector decimation for natural cursor velocity (20px)."),
      aggressive: getI18nText("automation_opt_desc_aggressive", {}, "Aggressive simplification: trims straight trajectories to endpoints (40px).")
    };
    descEl.textContent = descs[optimizeLevel] || descs.balanced;
  }
}

// ── READY-TO-USE MACRO TEMPLATES ──────────────────────────────────────
const MACRO_TEMPLATES = {
  afk: {
    name: "AFK Anti-Kick",
    icon: "🔄",
    repeat: { mode: "until_stopped" },
    actions: [
      { type: "mouse_move", x: 600, y: 400 },
      { type: "wait", ms: 45000 },
      { type: "mouse_move", x: 605, y: 405 },
      { type: "wait", ms: 45000 }
    ]
  },
  doubletap: {
    name: "Rapid Double-Tap",
    icon: "💥",
    repeat: { mode: "once" },
    actions: [
      { type: "mouse_click", button: "left", count: 1 },
      { type: "wait", ms: 35 },
      { type: "mouse_click", button: "left", count: 1 }
    ]
  },
  automine: {
    name: "Auto-Mine & Hold",
    icon: "⛏️",
    repeat: { mode: "times", count: 10 },
    actions: [
      { type: "mouse_down", button: "left" },
      { type: "wait", ms: 3000 },
      { type: "mouse_up", button: "left" },
      { type: "wait", ms: 250 },
      { type: "mouse_move", x: 500, y: 300 }
    ]
  }
};

async function importMacroTemplate(templateKey) {
  const tpl = MACRO_TEMPLATES[templateKey];
  if (!tpl) return;
  const m = {
    ...emptyMacro(tpl.name),
    icon: tpl.icon,
    repeat: tpl.repeat,
    actions: JSON.parse(JSON.stringify(tpl.actions)),
  };
  m.enabled = m.actions.map(() => true);
  try {
    await saveMacro(m);
    await renderMacroList();
    showToast(`✓ Template "${tpl.name}" added to My Macros`, "info");
  } catch (err) {
    console.error("[MacroTemplate] save failed:", err);
    alert("Failed to import template: " + err);
  }
}

async function initAutomationTab() {
  const recordBtn = document.getElementById("automationRecordBtn");
  const stopBtn = document.getElementById("automationStopRecordBtn");
  const buildBtn = document.getElementById("automationBuildBtn");
  const status = document.getElementById("automationRecordingStatus");

  // Wire 3-segment optimize buttons
  document.querySelectorAll(".macro-opt-seg").forEach(btn => {
    btn.addEventListener("click", () => {
      setOptimizeLevel(btn.dataset.level);
    });
  });
  refreshOptimizeSegments();

  // Wire templates
  document.querySelectorAll(".btn-template-use").forEach(btn => {
    btn.addEventListener("click", async (e) => {
      const tplKey = e.currentTarget.dataset.template;
      await importMacroTemplate(tplKey);
    });
  });

  if (recordBtn) {
    recordBtn.addEventListener("click", async () => {
      const op = stage("RecordStart");
      try {
        await op.run("start-recorder", () =>
          invoke("record_start", { mode: optimizeLevel === "aggressive" ? "precise" : "smart", recordHotkey: currentConfig.hotkeys?.record_hotkey || "Ctrl+Shift+R" })
        );
        isRecording = true;
        recordActionCount = 0;
        showRecordingOverlay(true);
        if (status) status.style.display = "block";
        recordBtn.disabled = true;
        op.ok(`mode=${optimizeLevel}`);
      } catch (err) {
        op.fail("record-start", err);
        console.error("[RecordStart] aborted:", err);
      }
    });
  }
  if (stopBtn) {
    stopBtn.addEventListener("click", async () => {
      const op = stage("RecordStop");
      try {
        const macros = await op.run("stop-recorder", () => invoke("record_stop"));
        isRecording = false;
        showRecordingOverlay(false);
        if (status) status.style.display = "none";
        if (recordBtn) recordBtn.disabled = false;

        const arrLength = await op.run("save-macro", () => finishRecordedMacros(macros, "button"));
        op.ok(`${arrLength} macro(s) returned`);
      } catch (err) {
        op.fail("record-stop", err);
        console.error("[RecordStop] aborted:", err);
      }
    });
  }
  if (buildBtn) {
    buildBtn.addEventListener("click", async () => {
      await openBuildMode();
    });
  }
  await renderMacroList();
}

async function renderMacroList() {
  const listEl = document.getElementById("automationMacroList");
  const countEl = document.getElementById("automationMacroCount");
  if (!listEl) return;
  let macros = [];
  try {
    macros = await invoke("list_macros");
  } catch (e) {
    console.error("list_macros failed:", e);
  }
  if (countEl) countEl.textContent = getI18nText("automation_macro_count", { count: macros.length }, `${macros.length} macro${macros.length === 1 ? "" : "s"}`);
  if (macros.length === 0) {
    listEl.innerHTML = `
      <div class="empty-state" style="color:var(--text-dim);padding:20px;text-align:center;border:1px dashed var(--border);border-radius:6px;">
        ${getI18nText("automation_empty", {}, "No macros yet. Hit 🔴 Record or + Build to create your first one.")}
      </div>`;
    return;
  }
  listEl.innerHTML = macros.map(m => {
    const previewActions = m.actions.slice(0, 4).map(formatActionShort).join(" → ");
    const more = m.actions.length > 4 ? ` … +${m.actions.length - 4} more` : "";
    return `
    <div class="macro-card" data-id="${m.id}" style="background:var(--bg-elev);border:1px solid var(--border);border-radius:6px;padding:12px;margin-bottom:8px;">
      <div style="display:flex;justify-content:space-between;align-items:center;">
        <div style="flex:1;">
          <span style="font-size:16px;margin-right:6px;">${m.icon || "🎬"}</span>
          <strong class="macro-name" data-id="${m.id}" tabindex="0" title="Click to rename">${escapeHtml(m.name)}</strong>
          <span style="color:var(--text-dim);font-size:12px;margin-left:8px;">${m.actions.length} action${m.actions.length === 1 ? "" : "s"}</span>
          <div style="color:var(--text-dim);font-size:12px;margin-top:6px;font-family:'Fira Code',monospace;">
            ${escapeHtml(previewActions + more)}
          </div>
        </div>
        <div style="display:flex;gap:6px;">
          <button class="btn-mini btn-play" data-id="${m.id}">${getI18nText("automation_btn_play", {}, "▶ Play")}</button>
          <button class="btn-mini btn-optimize" data-id="${m.id}" title="${getI18nText("automation_btn_optimize_title", {}, "Optimize recorded actions")}">⚡</button>
          <button class="btn-mini btn-edit" data-id="${m.id}" title="${getI18nText("automation_btn_visual_editor_title", {}, "Visual Editor (v3.3)")}">✎</button>
          <button class="btn-mini btn-delete" data-id="${m.id}">🗑</button>
        </div>
      </div>
    </div>`;
  }).join("");
  // Wire up handlers.
  listEl.querySelectorAll(".btn-play").forEach(btn => {
    btn.addEventListener("click", async (ev) => {
      const id = ev.target.dataset.id;
      const m = macros.find(x => x.id === id);
      if (m) await invoke("play_macro", { m });
    });
  });
  listEl.querySelectorAll(".btn-edit").forEach(btn => {
    btn.addEventListener("click", (ev) => {
      const id = ev.target.dataset.id;
      const m = macros.find(x => x.id === id);
      if (m) openVisualEditor(m, renderMacroList);
    });
  });
  listEl.querySelectorAll(".macro-name").forEach(nameEl => {
    const beginRename = () => {
      if (nameEl.querySelector("input")) return;
      const id = nameEl.dataset.id;
      const m = macros.find(x => x.id === id);
      if (!m) return;
      const input = document.createElement("input");
      input.className = "macro-name-input";
      input.value = m.name || "Untitled macro";
      input.setAttribute("aria-label", "Macro name");
      nameEl.replaceChildren(input);
      input.focus();
      input.select();
      let finished = false;
      const finish = async (save) => {
        if (finished) return;
        finished = true;
        const nextName = input.value.trim();
        if (!save || !nextName || nextName === m.name) {
          await renderMacroList();
          return;
        }
        try {
          m.name = nextName;
          await saveMacro(m);
          await renderMacroList();
        } catch (err) {
          console.error("[MacroRename] save failed:", err);
          alert(getI18nText("dialog_alert_macro_rename_fail", {}, "Failed to rename macro."));
          await renderMacroList();
        }
      };
      input.addEventListener("keydown", (event) => {
        if (event.key === "Enter") finish(true);
        if (event.key === "Escape") finish(false);
      });
      input.addEventListener("blur", () => finish(true));
    };
    nameEl.addEventListener("click", beginRename);
    nameEl.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        beginRename();
      }
    });
  });
  listEl.querySelectorAll(".btn-optimize").forEach(btn => {
    btn.addEventListener("click", async (ev) => {
      const id = ev.currentTarget.dataset.id;
      const m = macros.find(x => x.id === id);
      if (!m) return;
      const before = m.actions.length;
      m.actions = optimizeActionList(m.actions, optimizeLevel);
      m.enabled = m.actions.map(() => true);
      const diff = before - m.actions.length;
      try {
        await saveMacro(m);
        console.log(`[MacroOptimize] ${m.name}: ${before} → ${m.actions.length} actions (${diff} cleaned)`);
        await renderMacroList();
        showToast(diff > 0
          ? `⚡ Cleaned ${diff} redundant action(s) (${optimizeLevel})`
          : `⚡ Macro already optimal for ${optimizeLevel} mode`, "info");
      } catch (err) {
        console.error("[MacroOptimize] save failed:", err);
        alert(getI18nText("dialog_alert_macro_optimize_fail", {}, "Failed to save optimized macro."));
      }
    });
  });
  listEl.querySelectorAll(".btn-delete").forEach(btn => {
    btn.addEventListener("click", async (ev) => {
      const id = ev.target.dataset.id;
      if (!confirm(getI18nText("dialog_confirm_delete_macro", {}, "Delete this macro?"))) return;
      await invoke("delete_macro", { id });
      await renderMacroList();
    });
  });
}

/// Compact human label for an Action. Used in the macro list preview.
function formatActionShort(a) {
  if (!a || typeof a !== "object") return "?";
  switch (a.type) {
    case "mouse_move":  return `→(${a.x},${a.y})`;
    case "mouse_click": return `🖱${a.button || "left"}`;
    case "mouse_down":  return `▼${a.button || "left"}`;
    case "mouse_up":    return `▲${a.button || "left"}`;
    case "key_press":   return `⌨${a.key || ""}`;
    case "key_down":    return `⌨↓${a.key || ""}`;
    case "key_up":      return `⌨↑${a.key || ""}`;
    case "scroll":      return `📜${a.delta_y > 0 ? "↓" : "↑"}`;
    case "wait":        return `⏱${a.ms}ms`;
    default:            return a.type;
  }
}

// Post-process an already recorded Action list. The recorder's Rust
// normalizer handles raw events; this pass handles edits and macros loaded
// from storage, where only Actions remain.
// Humanize: add ±jitterPercent variation to every wait interval so the
// macro no longer looks perfectly metronomic. Values are clamped to
// [1 ms, 30 s]; the seed is per-action so the same input produces the
// same output (safe to run multiple times).
function humanizeActionList(actions, jitterPercent = 20) {
  if (!Array.isArray(actions)) return actions;
  const pct = Math.max(0, Math.min(80, Number(jitterPercent) || 0));
  if (pct === 0) return actions;
  return actions.map((a) => {
    if (!a || typeof a !== "object") return a;
    if (a.type !== "wait") return a;
    const original = Number(a.ms);
    if (!Number.isFinite(original) || original < 1) return a;
    const rnd = Math.random() * 2 - 1;
    const jittered = original * (1 + (pct / 100) * rnd);
    const clamped = Math.max(1, Math.min(30000, Math.round(jittered)));
    return { ...a, ms: clamped, _humanized: true };
  });
}

function optimizeActionList(actions, level = optimizeLevel) {
  const moveThreshold = level === "subtle" ? 10 : level === "aggressive" ? 40 : 20;
  const compact = [];

  for (const action of Array.isArray(actions) ? actions : []) {
    if (!action || typeof action !== "object") continue;

    if (action.type === "mouse_move") {
      const previous = compact[compact.length - 1];
      if (previous?.type === "mouse_move") {
        const distance = Math.hypot(
          Number(action.x) - Number(previous.x),
          Number(action.y) - Number(previous.y)
        );
        if (distance < moveThreshold) {
          // Keep the latest coordinate. It is the useful position immediately
          // before the next click/key event, while intermediate samples are noise.
          compact[compact.length - 1] = action;
          continue;
        }
      }
      compact.push(action);
      continue;
    }

    if (action.type === "wait" && Number(action.ms) < 25) continue;
    compact.push(action);
  }

  const paired = [];
  for (let i = 0; i < compact.length; i += 1) {
    const current = compact[i];
    const next = compact[i + 1];
    const afterNext = compact[i + 2];

    if (current.type === "mouse_down") {
      const wait = next?.type === "wait" ? next : null;
      const release = wait ? afterNext : next;
      if (release?.type === "mouse_up" && release.button === current.button) {
        const holdMs = wait ? Number(wait.ms) || 0 : 0;
        if (holdMs <= 200) {
          paired.push({ type: "mouse_click", button: current.button, count: 1 });
          i += wait ? 2 : 1;
          continue;
        }
      }
    }

    if (current.type === "key_down") {
      const wait = next?.type === "wait" ? next : null;
      const release = wait ? afterNext : next;
      if (release?.type === "key_up" && release.key === current.key &&
          JSON.stringify(release.mods || {}) === JSON.stringify(current.mods || {})) {
        const holdMs = wait ? Number(wait.ms) || 0 : 0;
        if (holdMs <= 50) {
          paired.push({ type: "key_press", key: current.key, mods: current.mods || {} });
          i += wait ? 2 : 1;
          continue;
        }
      }
    }

    paired.push(current);
  }

  const merged = [];
  for (let i = 0; i < paired.length; i += 1) {
    const current = paired[i];
    const wait = paired[i + 1];
    const next = paired[i + 2];

    if (current.type === "mouse_click" && wait?.type === "wait" &&
        next?.type === "mouse_click" && current.button === next.button &&
        Number(wait.ms) <= 100) {
      merged.push({
        type: "mouse_click",
        button: current.button,
        count: Math.min(255, (Number(current.count) || 1) + (Number(next.count) || 1))
      });
      i += 2;
      continue;
    }
    if (current.type === "wait" && merged[merged.length - 1]?.type === "wait") {
      merged[merged.length - 1].ms += Number(current.ms) || 0;
      continue;
    }
    merged.push({ ...current });
  }

  const result = [];
  for (const action of merged) {
    if (action.type !== "wait") {
      result.push(action);
      continue;
    }
    let remaining = Math.max(0, Number(action.ms) || 0);
    while (remaining >= 25) {
      const chunk = Math.min(60000, remaining);
      result.push({ type: "wait", ms: chunk });
      remaining -= chunk;
    }
  }
  while (result[result.length - 1]?.type === "wait") result.pop();
  return result;
}

/// Visual Editor — per-action edit modal with:
///  - drag-reorder (↑/↓ buttons + native HTML5 drag)
///  - inline edit values (ms / x,y / key)
///  - enable/disable toggle per action
///  - add new action (Wait, Click, Move, Key)
///  - delete action
///  - live preview of the macro before save
async function openVisualEditor(macro, onChange) {
  const html = `
    <div class="modal-backdrop" id="visualEditorBackdrop" style="position:fixed;inset:0;background:rgba(0,0,0,0.65);display:flex;align-items:center;justify-content:center;z-index:1000;">
      <div class="modal ve-modal" style="background:var(--bg-card);border:1px solid var(--border);border-radius:8px;padding:20px;width:780px;max-width:94vw;max-height:88vh;display:flex;flex-direction:column;">
        <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:14px;">
          <div>
            <div style="display:flex;align-items:center;gap:8px;">
              <span style="font-size:18px;">🧩</span>
              <input id="veMacroNameInput" class="ve-name-input" value="${escapeHtml(macro.name || "Untitled macro")}" aria-label="Macro name" spellcheck="false">
            </div>
            <div class="ve-subtitle">${getI18nText("ve_subtitle", {}, "Arrange action blocks into a visual sequence")}</div>
          </div>
          <button id="veCloseBtn" style="background:none;border:none;color:var(--text-bright);font-size:18px;cursor:pointer;">✕</button>
        </div>
        <div class="ve-palette">
          <span class="ve-palette-label">${getI18nText("ve_palette_label", {}, "ADD BLOCK")}</span>
          <button class="ve-add ve-add-wait" data-type="wait">${getI18nText("ve_btn_wait", {}, "＋ Wait")}</button>
          <button class="ve-add ve-add-click" data-type="click">${getI18nText("ve_btn_click", {}, "＋ Click")}</button>
          <button class="ve-add ve-add-move" data-type="move">${getI18nText("ve_btn_move", {}, "＋ Move")}</button>
          <button class="ve-add ve-add-key" data-type="key">${getI18nText("ve_btn_key", {}, "＋ Key")}</button>
          <button id="veOptimizeBtn" class="btn-mini" title="${getI18nText("ve_btn_optimize_title", {}, "Remove noise and simplify recorded actions")}">${getI18nText("ve_btn_optimize", {}, "⚡ Optimize")}</button>
          <button id="veHumanizeBtn" class="btn-mini" title="${getI18nText("ve_btn_humanize_title", {}, "Add natural timing jitter (±20%) to wait intervals")}">${getI18nText("ve_btn_humanize", {}, "🧬 Humanize")}</button>
        </div>
        <div id="veActionsList" class="ve-canvas"></div>
        <div style="margin-top:14px;display:flex;justify-content:space-between;align-items:center;gap:8px;">
          <span id="veSummary" style="color:var(--text-dim);font-size:12px;"></span>
          <div style="display:flex;gap:8px;">
            <button id="veReRecordBtn" class="btn-secondary" title="${getI18nText("ve_btn_rerecord_title", {}, "Re-record this macro")}">${getI18nText("ve_btn_rerecord", {}, "⏺ Re-record")}</button>
            <button id="veSaveBtn" class="btn-primary">${getI18nText("ve_btn_save", {}, "💾 Save")}</button>
            <button id="veCancelBtn" class="btn-secondary">${getI18nText("modal_cancel", {}, "Cancel")}</button>
          </div>
        </div>
      </div>
    </div>`;
  document.body.insertAdjacentHTML("beforeend", html);

  // Initialize `enabled` array so individual toggles work even when missing.
  if (!Array.isArray(macro.enabled)) {
    macro.enabled = macro.actions.map(() => true);
  }
  // Resize enabled array to match actions array length (handle add/delete).
  while (macro.enabled.length < macro.actions.length) macro.enabled.push(true);
  while (macro.enabled.length > macro.actions.length) macro.enabled.pop();

  const listEl = document.getElementById("veActionsList");
  const summaryEl = document.getElementById("veSummary");
  const nameInput = document.getElementById("veMacroNameInput");
  if (nameInput) {
    nameInput.addEventListener("input", () => {
      macro.name = nameInput.value;
    });
  }

  const refresh = () => {
    listEl.innerHTML = macro.actions
      .map((a, idx) => renderEditorRow(a, idx, macro.enabled[idx] !== false))
      .join("");
    updateSummary();
    wireRowHandlers();
  };

  const updateSummary = () => {
    const total = macro.actions.length;
    const enabled = macro.enabled.filter(Boolean).length;
    const controlFlow = macro.actions.filter(a =>
      a.type === "repeat" || a.type === "if" || a.type === "call"
    ).length;
    const sets = macro.actions.filter(a => a.type === "set_var").length;
    const lines = [
      `${enabled} / ${total} actions enabled · ${total} total`,
    ];
    if (controlFlow > 0) lines.push(`${controlFlow} control-flow`);
    if (sets > 0) lines.push(`${sets} vars`);
    summaryEl.textContent = lines.join(" · ");
  };

  document.getElementById("veOptimizeBtn").addEventListener("click", () => {
    const before = macro.actions.length;
    macro.actions = optimizeActionList(macro.actions, optimizeLevel);
    macro.enabled = macro.actions.map(() => true);
    refresh();
    const removed = before - macro.actions.length;
    console.log(`[MacroOptimize] ${macro.name}: ${before} → ${macro.actions.length} actions (${removed} removed)`);
  });

  document.getElementById("veHumanizeBtn").addEventListener("click", () => {
    macro.actions = humanizeActionList(macro.actions, 20);
    refresh();
    console.log(`[MacroHumanize] ${macro.name}: applied ±20% jitter to wait intervals`);
  });

  const wireRowHandlers = () => {
    listEl.querySelectorAll(".ve-del").forEach(b => {
      b.addEventListener("click", () => {
        const i = parseInt(b.dataset.idx);
        macro.actions.splice(i, 1);
        macro.enabled.splice(i, 1);
        refresh();
      });
    });

    // ── DRAG-TO-REORDER (HTML5 DnD) ──────────────────────────────────────
    listEl.querySelectorAll(".ve-row").forEach(row => {
      row.setAttribute("draggable", "true");
      row.addEventListener("dragstart", (ev) => {
        ev.dataTransfer.setData("text/plain", row.dataset.idx);
        ev.dataTransfer.effectAllowed = "move";
        row.style.opacity = "0.5";
      });
      row.addEventListener("dragend", () => { row.style.opacity = "1"; });
      row.addEventListener("dragover", (ev) => {
        ev.preventDefault();
        ev.dataTransfer.dropEffect = "move";
        row.style.borderTop = "2px solid var(--accent)";
      });
      row.addEventListener("dragleave", () => { row.style.borderTop = ""; });
      row.addEventListener("drop", (ev) => {
        ev.preventDefault();
        row.style.borderTop = "";
        const from = parseInt(ev.dataTransfer.getData("text/plain"));
        const to = parseInt(row.dataset.idx);
        if (Number.isNaN(from) || Number.isNaN(to) || from === to) return;
        const [item] = macro.actions.splice(from, 1);
        const [enItem] = macro.enabled.splice(from, 1);
        macro.actions.splice(to, 0, item);
        macro.enabled.splice(to, 0, enItem);
        refresh();
      });
    });

    // ── CONTEXT MENU ⋮ (Run from here / Step / Duplicate) ────────────────
    listEl.querySelectorAll(".ve-more").forEach(b => {
      b.addEventListener("click", (e) => {
        e.stopPropagation();
        const i = parseInt(b.dataset.idx);
        showRowContextMenu(b, i, macro, refresh, close);
      });
    });
    listEl.querySelectorAll(".ve-up").forEach(b => {
      b.addEventListener("click", () => {
        const i = parseInt(b.dataset.idx);
        if (i > 0) {
          [macro.actions[i - 1], macro.actions[i]] = [macro.actions[i], macro.actions[i - 1]];
          [macro.enabled[i - 1], macro.enabled[i]] = [macro.enabled[i], macro.enabled[i - 1]];
          refresh();
        }
      });
    });
    listEl.querySelectorAll(".ve-down").forEach(b => {
      b.addEventListener("click", () => {
        const i = parseInt(b.dataset.idx);
        if (i < macro.actions.length - 1) {
          [macro.actions[i], macro.actions[i + 1]] = [macro.actions[i + 1], macro.actions[i]];
          [macro.enabled[i], macro.enabled[i + 1]] = [macro.enabled[i + 1], macro.enabled[i]];
          refresh();
        }
      });
    });
    listEl.querySelectorAll(".ve-toggle").forEach(b => {
      b.addEventListener("click", () => {
        const i = parseInt(b.dataset.idx);
        macro.enabled[i] = !macro.enabled[i];
        refresh();
      });
    });
    // Inline edit: type="number" inputs for ms / x / y.
    listEl.querySelectorAll(".ve-input").forEach(inp => {
      inp.addEventListener("change", () => {
        const i = parseInt(inp.dataset.idx);
        const field = inp.dataset.field;
        const a = macro.actions[i];
        const v = parseInt(inp.value);
        if (!isNaN(v)) a[field] = v;
        refresh();
      });
    });
    listEl.querySelectorAll(".ve-input-str").forEach(inp => {
      inp.addEventListener("change", () => {
        const i = parseInt(inp.dataset.idx);
        const field = inp.dataset.field;
        macro.actions[i][field] = inp.value;
        refresh();
      });
    });
  };

  // Wire up "+ add" buttons.
  listEl.parentElement.querySelectorAll(".ve-add").forEach(b => {
    b.addEventListener("click", () => {
      const t = b.dataset.type;
      macro.actions.push(makeAction(t));
      macro.enabled.push(true);
      refresh();
    });
  });

  refresh();

  const close = () => {
    document.removeEventListener("keydown", onVeKeydown);
    document.getElementById("visualEditorBackdrop")?.remove();
  };
  const onVeKeydown = (e) => {
    if (e.key === "Escape") close();
  };
  document.addEventListener("keydown", onVeKeydown);
  document.getElementById("veCloseBtn")?.addEventListener("click", close);
  document.getElementById("veCancelBtn")?.addEventListener("click", close);
  const veBackdrop = document.getElementById("visualEditorBackdrop");
  if (veBackdrop) {
    veBackdrop.addEventListener("click", (e) => {
      if (e.target === veBackdrop) close();
    });
  }
  const reRec = document.getElementById("veReRecordBtn");
  if (reRec) reRec.addEventListener("click", () => {
    reRecordMacro(macro, onChange);
  });
  document.getElementById("veSaveBtn").addEventListener("click", async () => {
    try {
      const nextName = nameInput?.value.trim();
      if (!nextName) {
        nameInput?.focus();
        return;
      }
      macro.name = nextName;
      macro.updated_at = Date.now();
      // Filter disabled actions out for serialization integrity while
      // keeping indices intuitive in the editor.
      const all = macro.actions.map((a, i) => ({
        action: a,
        enabled: macro.enabled[i] !== false,
      }));
      macro.actions = all.filter(x => x.enabled).map(x => x.action);
      macro.enabled = macro.actions.map(() => true);
      await saveMacro(macro);
      close();
      if (onChange) await onChange();
    } catch (e) {
      alert(getI18nText("dialog_alert_save_fail", { err: e }, "Save failed: " + e));
    }
  });
}

/// Render a single editor row. Returns HTML.
function renderEditorRow(a, idx, enabled) {
  const safeType = String(a?.type || "unknown").replace(/[^a-z0-9_-]/gi, "-");
  const actionMeta = {
    mouse_move:  ["🖱", "MOVE",       "Position cursor"],
    mouse_click: ["◉", "CLICK",      "Mouse action"],
    mouse_down:  ["▼", "MOUSE DOWN", "Hold button"],
    mouse_up:    ["▲", "MOUSE UP",   "Release button"],
    key_press:   ["⌨", "KEY PRESS",  "Keyboard action"],
    key_down:    ["⌨", "KEY DOWN",   "Hold key"],
    key_up:      ["⌨", "KEY UP",     "Release key"],
    scroll:      ["↕", "SCROLL",     "Wheel action"],
    wait:        ["◷", "WAIT",       "Pause sequence"],
  }[a?.type] || ["◆", String(a?.type || "ACTION").toUpperCase(), "Action block"];
  const dimStyle = enabled ? "" : "opacity:0.45;filter:saturate(0.35);";
  const headerRow = `
    <div class="ve-row ve-type-${safeType}" data-idx="${idx}" style="${dimStyle}">
      <div class="ve-connector" aria-hidden="true"></div>
      <div class="ve-block">
        <div class="ve-block-face">
          <span class="ve-drag-handle" title="Drag to reorder">⋮⋮</span>
          <span class="ve-step">${String(idx + 1).padStart(2, "0")}</span>
          <span class="ve-block-icon">${actionMeta[0]}</span>
          <div class="ve-block-heading">
            <strong>${actionMeta[1]}</strong>
            <span>${actionMeta[2]}</span>
          </div>
          <code class="ve-block-preview">${escapeHtml(formatActionShort(a))}</code>
          <button class="ve-toggle" data-idx="${idx}" title="Enable/disable this action">${enabled ? "●" : "○"}</button>
          <button class="ve-more" data-idx="${idx}" title="More actions">⋯</button>
          <button class="ve-up" data-idx="${idx}" title="Move up">↑</button>
          <button class="ve-down" data-idx="${idx}" title="Move down">↓</button>
          <button class="ve-del" data-idx="${idx}" title="Delete">×</button>
        </div>
      </div>
    </div>`;
  // Inline editors per action type.
  let bodyRow = "";
  if (!a || typeof a !== "object") return headerRow;
  switch (a.type) {
    case "wait":
      bodyRow = `
        <div class="ve-field-row">
          <label style="font-size:11px;color:var(--text-dim);">ms</label>
          <input class="ve-input" data-idx="${idx}" data-field="ms" type="number" min="0" step="10"
                 value="${a.ms ?? 100}">
        </div>`;
      break;
    case "mouse_move":
      bodyRow = `
        <div class="ve-field-row">
          <label style="font-size:11px;color:var(--text-dim);">x</label>
          <input class="ve-input" data-idx="${idx}" data-field="x" type="number" value="${a.x ?? 0}"
                 >
          <label style="font-size:11px;color:var(--text-dim);">y</label>
          <input class="ve-input" data-idx="${idx}" data-field="y" type="number" value="${a.y ?? 0}"
                 >
        </div>`;
      break;
    case "key_press":
    case "key_down":
    case "key_up":
      bodyRow = `
        <div class="ve-field-row">
          <label style="font-size:11px;color:var(--text-dim);">vk</label>
          <input class="ve-input" data-idx="${idx}" data-field="key" type="number" min="0" max="255"
                 value="${(a.key ?? 0x41) & 0xff}">
          <span style="font-size:11px;color:var(--text-dim);align-self:center;">VK code (decimal)</span>
        </div>`;
      break;
    case "scroll":
      bodyRow = `
        <div class="ve-field-row">
          <label style="font-size:11px;color:var(--text-dim);">Δy</label>
          <input class="ve-input" data-idx="${idx}" data-field="delta_y" type="number"
                 value="${a.delta_y ?? 120}">
        </div>`;
      break;
  }
  return `<div data-idx="${idx}">${headerRow}${bodyRow}</div>`;
}

/// Build a fresh Action object with default values.
function makeAction(type) {
  switch (type) {
    case "wait":  return { type: "wait",   ms: 200 };
    case "click": return { type: "mouse_click", button: "left",  count: 1 };
    case "move":  return { type: "mouse_move",  x: 500, y: 300 };
    case "key":   return { type: "key_press",   key: 0x41, mods: { ctrl: false, shift: false, alt: false, win: false } };
    default:      return { type: type };
  }
}

// ── INIT ────────────────────────────────────────────────────
//
// **Defensive wrap**: the init() flow used to throw uncaught exceptions
// when `window.__TAURI__.core.invoke` was briefly unavailable during
// WebView2 startup, which (combined with the global console interceptor)
// flooded the logBuffer and triggered "Out of Memory" in the WebView2
// child processes. Now each step is independently guarded so a single
// failed step cannot take down the whole UI.
onDomReady(() => {
  bootCanary("dom-ready");
  // Honour the persisted debug flag instead of forcing it on every launch.
  invoke("get_debug_mode")
    .then((d) => {
      DEBUG_UI = !!d;
      setDebugMode(DEBUG_UI);
      bootCanary(`debug-mode=${DEBUG_UI}`);
    })
    .catch(() => {
      // Backend not ready yet — keep DEBUG_UI at its module-load default
      // (false for release builds) instead of forcing it on.
    });

  const safeStep = (name, fn) => {
    try {
      fn();
      bootCanary(`step ok: ${name}`);
    } catch (e) {
      // ALWAYS visible: a failed init step is the difference between a working
      // UI and "the interface does nothing", so it must not depend on the
      // debug flag.
      bootCanary(`step FAILED: ${name} — ${e && e.message ? e.message : String(e)}`);
      try { console.error(`[init] step '${name}' failed:`, e); } catch (_) { /* never throw from a logger */ }
    }
  };

  // Phased startup sequence to prevent CPU spikes and IPC queue congestion
  // Phase 1 (Immediate): Core UI & app config loading
  safeStep("loadConfig", () => loadConfig());
  safeStep("initAutomationTab", () => { void initAutomationTab(); });

  // === BOOT HANDSHAKE (v1.1.0) ===
  // boot_guard.js (classic script, loaded before this module) starts a ~3 s
  // watchdog: if this flag is still unset it writes "MODULE DID NOT EXECUTE
  // (SyntaxError class)" into %TEMP%\nanoclick_web.log and shows a visible
  // banner. It is the only detection that survives a module-level SyntaxError,
  // because in that case nothing inside this file — including this line — runs.
  window.__nanoclick_boot_ok__ = true;

  // Phase 2 (Deferred 150ms): Non-critical diagnostic & version checks
  setTimeout(() => {
    safeStep("syncVersionDisplay", () => { void syncVersionDisplay(); });
    safeStep("checkPlatformCapabilities", () => { void checkPlatformCapabilities(); });
  }, 150);

  // Phase 3 (Background 3000ms): Auto-updater check
  setTimeout(() => {
    safeStep("startUpdateChecker", () => startUpdateChecker());
  }, 3000);

  try { console.log("[nanoclick] init complete (phased)"); } catch (_) { /* ignore */ }
});

// ── Toggle Response Time (debounce) ──────────────────────────
function debounceZoneLabel(ms) {
  if (ms <= 35) return "Instant";
  if (ms <= 75) return "Fast";
  return "Relaxed";
}
function applyDebounceFromConfig(ms) {
  const slider = document.getElementById("debounceSlider");
  const label = document.getElementById("debounceLabel");
  if (!slider || !label) return;
  const clamped = Math.max(5, Math.min(250, parseInt(ms) || 80));
  slider.value = clamped;
  label.textContent = `${clamped} ms · ${debounceZoneLabel(clamped)}`;
  document.querySelectorAll(".debounce-zone").forEach(z => {
    const zm = parseInt(z.dataset.ms);
    z.classList.toggle("zone-active",
      (zm === 5 && clamped <= 35) ||
      (zm === 60 && clamped > 35 && clamped <= 75) ||
      (zm === 150 && clamped > 75));
  });
}
onDomReady(() => {
  // Engine inputs that need explicit auto-save (debounce/start-delay/stop-time).
  // Note: most engine inputs already wire saveConfig via the L760-L800 change
  // listeners; these three were missing — without explicit input/change
  // listeners they never persisted across restarts.
  const startDelayInputEl = document.getElementById("startDelayInput");
  const stopDurationInputEl = document.getElementById("stopDurationInput");
  const stopTimeInputEl = document.getElementById("stopTimeInput");
  if (startDelayInputEl) startDelayInputEl.addEventListener("input", () => { if (typeof saveConfig === "function") saveConfigThrottled(); });
  if (stopDurationInputEl) stopDurationInputEl.addEventListener("input", () => { if (typeof saveConfig === "function") saveConfigThrottled(); });
  if (stopTimeInputEl) stopTimeInputEl.addEventListener("input", () => { if (typeof saveConfig === "function") saveConfigThrottled(); });

  const slider = document.getElementById("debounceSlider");
  if (slider) slider.addEventListener("input", () => {
    applyDebounceFromConfig(slider.value);
    if (typeof currentConfig !== "undefined" && currentConfig && currentConfig.engine) {
      currentConfig.engine.hotkey_debounce_ms = Math.max(5, Math.min(250, parseInt(slider.value) || 80));
    }
    if (typeof saveConfig === "function") saveConfigThrottled();
  });
  document.querySelectorAll(".debounce-zone").forEach(z => {
    z.addEventListener("click", () => {
      applyDebounceFromConfig(parseInt(z.dataset.ms));
      if (typeof currentConfig !== "undefined" && currentConfig && currentConfig.engine) {
        currentConfig.engine.hotkey_debounce_ms = Math.max(5, Math.min(250, parseInt(z.dataset.ms) || 80));
      }
      if (typeof saveConfig === "function") saveConfig();
    });
  });
});

// ── STATISTICS VIEW & ANALYTICS ENGINE ─────────────────────────
// The live engine is stats.js (window.StatsEngine). main.js used to carry a
// parallel copy of the same bookkeeping — its own `_stats` object plus
// recordCpsHistoryPoint/renderStats-DOM-writer/drawStatsChart. Nothing ever fed
// that copy on the recording path (the status tick delegates to StatsEngine with
// no `else` branch), so its CPS history stayed empty and its chart could only
// ever paint the "start autoclicker" placeholder. Removed; ensureStatsConfig()
// stays because the preset flow calls it directly.

function ensureStatsConfig() {
  if (!currentConfig.stats || typeof currentConfig.stats !== "object") {
    currentConfig.stats = {
      total_clicks: 0,
      total_active_ms: 0,
      total_sessions: 0,
      presets_applied: 0,
      max_cps: 0.0,
      history: []
    };
  }
  if (typeof currentConfig.stats.total_clicks !== "number") currentConfig.stats.total_clicks = 0;
  if (typeof currentConfig.stats.total_active_ms !== "number") currentConfig.stats.total_active_ms = 0;
  if (typeof currentConfig.stats.total_sessions !== "number") currentConfig.stats.total_sessions = 0;
  if (typeof currentConfig.stats.presets_applied !== "number") currentConfig.stats.presets_applied = 0;
  if (typeof currentConfig.stats.max_cps !== "number") currentConfig.stats.max_cps = 0;
  if (!Array.isArray(currentConfig.stats.history)) currentConfig.stats.history = [];
  return currentConfig.stats;
}

/**
 * Thin delegator. StatsEngine (stats.js) is the only stats writer in this build;
 * keeping the signature means the two call sites (navigation + the 1 Hz
 * everyVisible tick below) stay untouched.
 */
function renderStats() {
  window.StatsEngine?.renderStats(currentConfig);
}

// The legacy canvas painter that used to live here read `_stats.liveCpsHistory`,
// which the recording path never filled — see the note above ensureStatsConfig().
// StatsEngine.drawStatsChart() (stats.js) owns the chart now.

onDomReady(() => {
  const resetBtn = document.getElementById("statsResetBtn");
  if (resetBtn) resetBtn.addEventListener("click", () => {
    if (confirm(getI18nText("dialog_confirm_reset_stats", {}, "Are you sure you want to reset all saved statistics?"))) {
      window.StatsEngine.resetStats(currentConfig, saveConfig);
    }
  });
  everyVisible(1000, renderStats);
});


// ── FULL BACKUP EXPORT / IMPORT ─────────────────────────────
onDomReady(() => {
  const exportBtn = document.getElementById("exportBackupBtn");
  const importBtn = document.getElementById("importBackupBtn");
  if (exportBtn) exportBtn.addEventListener("click", async () => {
    try {
      const json = await invoke("export_full_backup");
      const blob = new Blob([json], { type: "application/json" });
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = `nanoclick_backup_${new Date().toISOString().slice(0, 10)}.json`;
      anchor.style.display = "none";
      document.body.appendChild(anchor);
      anchor.click();
      anchor.remove();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    } catch (err) {
      console.error("[Backup] export failed:", err);
      alert(getI18nText("dialog_alert_export_backup_fail", {}, "Failed to export backup."));
    }
  });
  if (importBtn) importBtn.addEventListener("click", () => {
    const fileInput = document.createElement("input");
    fileInput.type = "file";
    fileInput.accept = ".json,application/json";
    fileInput.addEventListener("change", async (event) => {
      const file = event.target.files?.[0];
      if (!file) return;
      const restoreConfig = confirm(getI18nText("dialog_confirm_restore_config", {}, "Restore CONFIG + PRESETS from the backup?\n\nOK = yes, Cancel = keep current config."));
      const restoreMacros = restoreConfig && confirm(getI18nText("dialog_confirm_restore_macros", {}, "Also restore MACROS?\n\nOK = yes (replaces current macros), Cancel = keep current macros."));
      if (!restoreConfig && !restoreMacros) return;
      try {
        const backupJson = await file.text();
        const msg = await invoke("import_full_backup", {
          backupJson,
          restoreConfig,
          restoreMacros,
        });
        alert(getI18nText("dialog_alert_restore_backup_success", { msg }, `Backup restored successfully (${msg}). Restart the app to see all changes.`));
      } catch (err) {
        console.error("[Backup] import failed:", err);
        alert(getI18nText("dialog_alert_restore_backup_fail", { err }, `Import failed: ${err}`));
      }
    });
    fileInput.click();
  });
});


// ── SMART GUARD (Typing Guard + App filter) ─────────────────
// All markup wiring lives in settings_guard.js; main.js only hands over
// the save pipeline so guard edits persist like every other setting.
onDomReady(() => {
  if (window.SmartGuard) window.SmartGuard.init(saveConfig);
});

// ── IMAGE TRIGGER (F8) ────────────────────────────────────────
function renderImageTrigger() {
  const status = document.getElementById("imgTrigStatus");
  const xIn = document.getElementById("imgTrigX");
  const yIn = document.getElementById("imgTrigY");
  const cIn = document.getElementById("imgTrigColor");
  const tIn = document.getElementById("imgTrigTol");
  const pIn = document.getElementById("imgTrigPoll");
  const trig = currentConfig.image_trigger;
  if (!status) return;
  if (!trig) {
    status.textContent = getI18nText("settings_trigger_status_none", {}, "No image trigger set.");
    status.style.color = "";
    return;
  }
  if (xIn && !xIn.value) xIn.value = trig.x;
  if (yIn && !yIn.value) yIn.value = trig.y;
  if (cIn && !cIn.value) {
    const hex = (trig.color_rgba >>> 8).toString(16).padStart(6, "0").toUpperCase();
    cIn.value = "#" + hex;
  }
  if (tIn && !tIn.value) tIn.value = trig.tolerance;
  if (pIn && !pIn.value) pIn.value = trig.poll_ms;
  status.textContent = `Active: (${trig.x}, ${trig.y}) #${(trig.color_rgba >>> 8).toString(16).padStart(6, "0").toUpperCase()} ±${trig.tolerance} / ${trig.poll_ms}ms`;
  status.style.color = "var(--accent)";
}

onDomReady(() => {
  renderImageTrigger();
  const pickBtn = document.getElementById("pickPixelBtn");
  const saveBtn = document.getElementById("setImgTrigBtn");
  const clearBtn = document.getElementById("clearImgTrigBtn");
  if (pickBtn) pickBtn.addEventListener("click", async () => {
    try {
      const [x, y] = await invoke("get_cursor_pos_now");
      document.getElementById("imgTrigX").value = x;
      document.getElementById("imgTrigY").value = y;
      const rgba = await invoke("pick_screen_pixel", { x, y });
      if (rgba != null) {
        const hex = (rgba >>> 8).toString(16).padStart(6, "0").toUpperCase();
        document.getElementById("imgTrigColor").value = "#" + hex;
      }
    } catch (e) { console.error("pickPixel failed:", e); }
  });
  if (saveBtn) saveBtn.addEventListener("click", async () => {
    const x = parseInt(document.getElementById("imgTrigX").value, 10);
    const y = parseInt(document.getElementById("imgTrigY").value, 10);
    let hex = (document.getElementById("imgTrigColor").value || "").replace("#", "").trim();
    if (hex.length !== 6 || !/^[0-9A-Fa-f]{6}$/.test(hex)) {
      alert(getI18nText("dialog_alert_color_hex_invalid", {}, "Color must be 6 hex digits (RRGGBB)."));
      return;
    }
    const colorRgba = (parseInt(hex, 16) << 8) | 0xFF;
    const tol = Math.max(0, Math.min(255, parseInt(document.getElementById("imgTrigTol").value, 10) || 12));
    const poll = Math.max(50, Math.min(2000, parseInt(document.getElementById("imgTrigPoll").value, 10) || 120));
    if (!Number.isFinite(x) || !Number.isFinite(y)) { alert(getI18nText("dialog_alert_xy_required", {}, "X and Y required.")); return; }
    const trigger = { x, y, color_rgba: colorRgba, tolerance: tol, poll_ms: poll, label: `pixel ${x},${y}` };
    currentConfig.image_trigger = trigger;
    try {
      await invoke("set_image_trigger", { trigger });
      renderImageTrigger();
    } catch (e) { alert(getI18nText("dialog_alert_save_fail", { err: e }, `Save failed: ${e}`)); }
  });
  if (clearBtn) clearBtn.addEventListener("click", async () => {
    currentConfig.image_trigger = null;
    try {
      await invoke("set_image_trigger", { trigger: null });
      const status = document.getElementById("imgTrigStatus");
      if (status) { status.textContent = getI18nText("settings_trigger_status_none", {}, "No image trigger set."); status.style.color = ""; }
    } catch (e) { alert(getI18nText("dialog_alert_clear_fail", { err: e }, `Clear failed: ${e}`)); }
  });
  listen("image-trigger-match", (e) => {
    const status = document.getElementById("imgTrigStatus");
    if (status) { status.textContent = `Matched: ${e.payload}`; status.style.color = "var(--accent)"; }
  });
});

// ── PORTABLE MODE BADGE (F10) ───────────────────────────────
onDomReady(() => {
  if (document.getElementById("portableBadge")) return;
  const anchor = document.getElementById("saveConfigBtn")
    || document.querySelector(".settings-row");
  if (!anchor) return;
  const badge = document.createElement("div");
  badge.id = "portableBadge";
  badge.style.cssText = "font-size:11px;color:var(--text-dim);margin-top:6px;display:none;";
  if (document.body && window.__TAURI__ && window.__TAURI__.path) {
    // We can detect portable mode by asking Rust through the config path.
    invoke("get_config_path").then((p) => {
      const looksPortable = /\\\\nanoclick_data\\\\config\\.json$/i.test(p)
        || /\/nanoclick_data\/config\.json$/i.test(p);
      if (looksPortable) {
        badge.textContent = "Portable mode: config and macros stored next to nanoclick.exe";
        badge.style.display = "block";
        badge.style.color = "var(--accent)";
      }
    }).catch(() => {});
  }
});

// Clean process termination hook on REAL app close.
// v1.1.0 fix: was a bare `beforeunload` — which fires on EVERY page reload,
// including the benign WebView2 reload triggered by win.set_position() at
// boot (remember-window-position restore). Result: the restore moved the
// window -> page reloaded -> JS called exit_app -> full backend shutdown ->
// "frontend alive, backend dead". Now the kill only happens when the window
// is actually being torn down for good, not on internal reloads.
window.addEventListener("beforeunload", () => {
  // Webview2 fires beforeunload for internal reloads with no trusted user
  // intent; the app's own CloseRequested path in Rust (RunEvent::WindowEvent)
  // already performs the full shutdown, so a JS-initiated exit here is only
  // needed as a last-resort fallback. Gate it behind the same flag the
  // updater relaunch uses: a page that is being swapped out must NOT kill us.
  if (window.__NANOCLICK_RELOADING__) return;
  if (typeof invoke === "function") {
    invoke("exit_app").catch(() => {});
  }
});

window.addEventListener("nanoclick-language-changed", () => {
  setModeDisplay(currentConfig.active_mode || "autoclicker", true);
  updateSubSettingsVisibility();
  updateConfigFolderTooltip();
  if (currentConfig) { renderImageTrigger(); if (window.SmartGuard) window.SmartGuard.render(); }
  if (window.UipiManager?.updateHeaderBadge) window.UipiManager.updateHeaderBadge();
});
