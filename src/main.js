// ── Tauri 2.x API accessors ─────────────────────────────────────
// `withGlobalTauri: true` in tauri.conf.json exposes `window.__TAURI__`
// with all sub-modules: app, core (invoke), dpi, event, image, menu,
// mocks, path, tray, webview, webviewWindow, window.
//
// We use the public `core.invoke` for command calls and `event.listen`
// for event subscriptions — both are synchronous wrappers around the
// Tauri 2.x internals.
const TAURI = (typeof window !== "undefined" && window.__TAURI__) || null;

// ── LOGGING INFRASTRUCTURE ──────────────────────────────────────
// Per-call wrappers for `invoke` and `listen` so every IPC round-trip
// shows up in the dev log with timing + success/failure status.
// Without this, an "operation didn't happen" symptom had no trail to
// diagnose — the log was a wall of silence.
function ts() {
  const d = new Date();
  return d.toTimeString().slice(0, 8) + "." + String(d.getMilliseconds()).padStart(3, "0");
}
function logCall(direction, label, extra) {
  try { console.log(`[${ts()}] [${direction}] ${label}`, extra ?? ""); } catch (_) { /* never throw out of a logger */ }
}

// ── invoke wrapper ──────────────────────────────────────
// Logs:  cmd name, args summary, elapsed ms, success value or error message.
const rawInvoke = TAURI?.core?.invoke
  ? TAURI.core.invoke
  : async () => { throw new Error("Tauri invoke not available"); };
const invoke = async function(cmd, args) {
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
};

// ── listen wrapper ──────────────────────────────────────
// Logs: subscription start + every payload received. For high-frequency
// events (~30/s status-updates during clicking), use `listenSilent` to
// skip the per-event log and let the handler log only what matters.
const rawListen = TAURI?.event?.listen
  ? TAURI.event.listen
  : async () => () => {};
const listen = async function(eventName, handler) {
  logCall("→SUB", `event="${eventName}"`);
  try {
    const unlisten = await rawListen(eventName, (event) => {
      const payloadKeys = event?.payload && typeof event.payload === "object"
        ? Object.keys(event.payload).join(",") : typeof event?.payload;
      logCall("←EVT", `event="${eventName}"`, payloadKeys);
      try {
        return handler(event);
      } catch (err) {
        logCall("✗EVT", `event="${eventName}" handler threw — ${err?.message ?? err}`);
        throw err;
      }
    });
    logCall("✓SUB", `event="${eventName}" registered (unlisten fn: ${typeof unlisten})`);
    return unlisten;
  } catch (err) {
    logCall("✗SUB", `event="${eventName}" subscribe failed — ${err?.message ?? err}`);
    return () => {}; // never throw — silent noop is the original behavior
  }
};

// Silent variant — no per-event payload log, only sub start/result.
// Use for noisy events where the handler does its own throttled logging.
const listenSilent = async function(eventName, handler) {
  logCall("→SUB•", `event="${eventName}" (silent — handler will log)`);
  try {
    const unlisten = await rawListen(eventName, (event) => {
      try {
        return handler(event);
      } catch (err) {
        logCall("✗EVT", `event="${eventName}" handler threw — ${err?.message ?? err}`);
        throw err;
      }
    });
    logCall("✓SUB", `event="${eventName}" registered (silent, unlisten fn: ${typeof unlisten})`);
    return unlisten;
  } catch (err) {
    logCall("✗SUB", `event="${eventName}" subscribe failed — ${err?.message ?? err}`);
    return () => {};
  }
};

// ── GLOBAL ERROR TRAPS ──────────────────────────────────────
// Capture synchronous throws (window.onerror) and async promise rejections
// (window.onunhandledrejection) so they land in the dev log instead of
// disappearing into the WebView console. Both are no-ops if already installed.
if (typeof window !== "undefined" && !window.__nanoclick_log_installed__) {
  window.__nanoclick_log_installed__ = true;
  window.addEventListener("error", (e) => {
    logCall("✗ERR", `${e.filename}:${e.lineno}:${e.colno}`, e.message);
    // Don't preventDefault — let Tauri/devtools see it too.
  });
  window.addEventListener("unhandledrejection", (e) => {
    const reason = e.reason?.message ?? e.reason;
    logCall("✗REJ", `unhandled promise rejection`, reason);
  });
}

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
    const ms = (performance.now() - start).toFixed(1);
    const safeDetail = String(detail || "").slice(0, 80);
    const tag = icon === "✓" ? "→STAGE✓" : icon === "✗" ? "→STAGE✗" : "→STAGE•";
    logCall(tag, `[${name}] #${++stepNum} ${label}`, safeDetail);
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

// Diagnostic banner: if Tauri globals are missing, dump the page state so
// we can see *why* the UI is dead instead of staring at a blank window.
if (typeof window === "undefined" || !TAURI || !TAURI.core || typeof TAURI.core.invoke !== "function") {
  document.addEventListener("DOMContentLoaded", () => {
    const err = document.createElement("div");
    err.style.cssText = "position:fixed;top:0;left:0;right:0;padding:24px;background:#7f1d1d;color:#fff;font:14px monospace;z-index:99999;white-space:pre-wrap";
    err.textContent = "NanoClick WebView error: window.__TAURI__ is " + (TAURI ? "missing .core.invoke" : "undefined") + ".\\n\\nThis usually means Tauri did not inject the global, or this page is loaded outside Tauri.\\n\\nKeys present: " + (TAURI ? Object.keys(TAURI).join(", ") : "<none>");
    document.body.appendChild(err);
  });
}

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
    jitter_radius_px: 3,
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

// DOM Elements
const statusBadge = document.getElementById("statusBadge");
const modeToggleBtn = document.getElementById("modeToggleBtn");
const clickCounter = document.getElementById("clickCounter");
const displayCps = document.getElementById("displayCps");
const toggleBtn = document.getElementById("toggleBtn");
const toggleBtnText = document.getElementById("toggleBtnText");

const cpsRange = document.getElementById("cpsRange");
const cpsInput = document.getElementById("cpsInput");
const randomRange = document.getElementById("randomRange");
const randomInput = document.getElementById("randomInput");
const limitInput = document.getElementById("limitInput");
const limitRange = document.getElementById("limitRange");
const clickTypeSelect = document.getElementById("clickTypeSelect");
const posXInput = document.getElementById("posXInput");
const posYInput = document.getElementById("posYInput");
const repeatCountInput = document.getElementById("repeatCountInput");
const hotkeyRecordBtn = document.getElementById("hotkeyRecordBtn");
const modeSwitchRecordBtn = document.getElementById("modeSwitchRecordBtn");
const recordMacroHotkeyBtn = document.getElementById("recordMacroHotkeyBtn");
const hotkeyRecordLabel = document.getElementById("hotkeyRecordLabel");
const modeSwitchRecordLabel = document.getElementById("modeSwitchRecordLabel");
const recordMacroHotkeyLabel = document.getElementById("recordMacroHotkeyLabel");

const configPathDisplay = document.getElementById("configPathDisplay");
const guiLockDelayInput = document.getElementById("guiLockDelayInput");
const jitterRadiusInput = document.getElementById("jitterRadiusInput");
const rippleCheckbox = document.getElementById("rippleCheckbox");
const footerModeShortcut = document.getElementById("footerModeShortcut");

// Modal
const onboardingModal = document.getElementById("onboardingModal");
const onboardingBtn = document.getElementById("onboardingBtn");

// ── SIDEBAR NAVIGATION ──────────────────────────────────────
document.querySelectorAll(".nav-item").forEach(navBtn => {
  navBtn.addEventListener("click", async () => {
    const viewId = navBtn.getAttribute("data-view");
    if (!viewId) return;

    // Auto-pause autoclicker when navigating away from Dashboard
    if (viewId !== "viewDashboard" && isRunning) {
      try {
        const active = await invoke("toggle_autoclicker");
        setRunningState(active);
      } catch (err) {
        console.error("Auto-pause on navigation failed:", err);
      }
    }

    // Update nav active state
    document.querySelectorAll(".nav-item").forEach(b => b.classList.remove("active"));
    navBtn.classList.add("active");

    // Show the target view
    document.querySelectorAll(".view").forEach(v => v.classList.remove("active"));
    const target = document.getElementById(viewId);
    if (target) target.classList.add("active");
  });
});

// ── MODE DISPLAY ────────────────────────────────────────────
function setModeDisplay(mode) {
  currentConfig.active_mode = mode;
  const switchKey = currentConfig.hotkeys ? currentConfig.hotkeys.mode_switch : "Ctrl+Alt+M";

  if (modeToggleBtn) {
    const modeIcon = modeToggleBtn.querySelector(".mode-icon");
    const modeText = modeToggleBtn.querySelector(".mode-text");
    const modeHint = modeToggleBtn.querySelector(".mode-shortcut-hint");

    if (mode === "autoclicker") {
      if (modeIcon) modeIcon.textContent = "⚡";
      if (modeText) modeText.textContent = "AUTOCLICKER MODE";
      if (modeHint) modeHint.textContent = `[ ${switchKey} ]`;
      modeToggleBtn.className = "prominent-mode-btn autoclicker";
      modeToggleBtn.title = `Click or press ${switchKey} to switch to WORK mode`;
    } else {
      if (modeIcon) modeIcon.textContent = "⌨️";
      if (modeText) modeText.textContent = "WORK MODE (PAUSED)";
      if (modeHint) modeHint.textContent = `[ ${switchKey} ]`;
      modeToggleBtn.className = "prominent-mode-btn work";
      modeToggleBtn.title = `Click or press ${switchKey} to enable AUTOCLICKER mode`;
    }
  }

  // Lock/Unlock Start Button depending on mode
  if (toggleBtn) {
    const toggleBtnHint = toggleBtn.querySelector(".start-btn-hint");
    if (mode === "work") {
      toggleBtn.disabled = true;
      toggleBtn.classList.add("disabled-mode");
      if (toggleBtnText) toggleBtnText.textContent = "DISABLED IN WORK MODE";
      if (toggleBtnHint) toggleBtnHint.textContent = "Switch to Autoclicker Mode to Start";
    } else {
      toggleBtn.disabled = false;
      toggleBtn.classList.remove("disabled-mode");
      if (!isRunning) {
        if (toggleBtnText) toggleBtnText.textContent = "START AUTOMATION";
        if (toggleBtnHint) toggleBtnHint.textContent = currentConfig.hotkeys?.toggle || "R / K";
      }
    }
  }
}

const limitBadge = document.getElementById("limitBadge");
const holdSubSettings = document.getElementById("holdSubSettings");
const holdDurationInput = document.getElementById("holdDurationInput");
const holdIntervalInput = document.getElementById("holdIntervalInput");
const repeatIntervalInput = document.getElementById("repeatIntervalInput");
const pickPosBtn = document.getElementById("pickPosBtn");
const pickPosStatus = document.getElementById("pickPosStatus");
const openConfigFolderBtn = document.getElementById("openConfigFolderBtn");

const startMinimizedCheckbox = document.getElementById("startMinimizedCheckbox");
const autostartCheckbox = document.getElementById("autostartCheckbox");
const minimizeToTrayCheckbox = document.getElementById("minimizeToTrayCheckbox");
const notificationsCheckbox = document.getElementById("notificationsCheckbox");
const pauseFocusLossCheckbox = document.getElementById("pauseFocusLossCheckbox");

const themeSelect = document.getElementById("themeSelect");
const accentSwatches = document.querySelectorAll("#accentSwatches .swatch");

const emergencyRecordBtn = document.getElementById("emergencyRecordBtn");
const speedUpRecordBtn = document.getElementById("speedUpRecordBtn");
const slowDownRecordBtn = document.getElementById("slowDownRecordBtn");
const pickPosRecordBtn = document.getElementById("pickPosRecordBtn");

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
}

// ── CONFIG SYNC ─────────────────────────────────────────────
function updateUiFromConfig(config) {
  currentConfig = config;

  if (config.first_run) {
    onboardingModal.classList.remove("hidden");
  } else {
    onboardingModal.classList.add("hidden");
  }

  setModeDisplay(config.active_mode || "autoclicker");

  if (cpsRange) cpsRange.value = config.engine.target_cps;
  if (cpsInput) cpsInput.value = config.engine.target_cps;
  if (displayCps) displayCps.textContent = Number(config.engine.target_cps).toFixed(1);

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

  const startDelayInput = document.getElementById("startDelayInput");
  const stopDurationInput = document.getElementById("stopDurationInput");
  const stopTimeInput = document.getElementById("stopTimeInput");

  if (startDelayInput) startDelayInput.value = Math.round((config.engine.start_delay_ms || 0) / 1000);
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
  }

  if (jitterRadiusInput) jitterRadiusInput.value = config.engine.jitter_radius_px;
  if (rippleCheckbox) rippleCheckbox.checked = config.ui.visual_ripple;

  if (startMinimizedCheckbox) startMinimizedCheckbox.checked = !!config.ui.start_minimized;
  if (autostartCheckbox) autostartCheckbox.checked = !!config.ui.autostart;
  if (minimizeToTrayCheckbox) minimizeToTrayCheckbox.checked = config.ui.minimize_to_tray !== false;
  if (notificationsCheckbox) notificationsCheckbox.checked = config.ui.show_notifications !== false;
  if (pauseFocusLossCheckbox) pauseFocusLossCheckbox.checked = !!config.ui.pause_on_focus_loss;

  if (themeSelect && config.ui.theme) {
    themeSelect.value = config.ui.theme;
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

// ===== Version display sync (v1.1+) =====
// The header/about version labels always mirror the real backend version
// (Cargo.toml / tauri.conf.json). No more hardcoded "vX.Y" in the HTML.
async async function syncVersionDisplay() {
  try {
    const v = await invoke("get_app_version");
    // Show FULL version (e.g. "v1.1.2") instead of truncated "v1.1".
    // Strip any pre-release suffix from the runtime string for display only.
    const display = "v" + String(v).split(/[-+]/)[0];
    document.querySelectorAll(".ver").forEach((el) => {
      el.textContent = el.id === "aboutVersion" ? display + " PRO" : display;
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
  } catch (err) {
    console.warn("[NanoClick] version sync failed:", err);
  }
}

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
    console.warn("[NanoClick] capabilities check failed:", err);
  }
}

async function loadConfig() {
  try {
    const config = await invoke("get_app_config");
    updateUiFromConfig(config);
    const path = await invoke("get_config_path");
    if (configPathDisplay) configPathDisplay.textContent = path;
  } catch (err) {
    console.error("Failed to load app config:", err);
  }
}

// Helper: safely parse int preserving 0 as a valid value
function safeInt(val, fallback) {
  const n = parseInt(val, 10);
  return isNaN(n) ? fallback : n;
}

async function saveConfig() {
  const op = stage("SaveConfig");
  try {
    await op.run("collect-input", () => {
      currentConfig.engine.target_cps = parseFloat(cpsInput?.value) || 29.0;
      currentConfig.engine.jitter_percent = parseFloat(randomInput?.value) || 0.0;
      currentConfig.engine.click_limit = safeInt(limitInput?.value, 0);
      currentConfig.engine.gui_lock_ms = safeInt(guiLockDelayInput?.value, 1500) || 1500;
      currentConfig.engine.hotkey_debounce_ms = safeInt(debounceSlider?.value, 80);
      currentConfig.engine.jitter_radius_px = safeInt(jitterRadiusInput?.value, 3) || 3;

      const startDelayInput = document.getElementById("startDelayInput");
      const stopDurationInput = document.getElementById("stopDurationInput");
      const stopTimeInput = document.getElementById("stopTimeInput");

      if (startDelayInput) currentConfig.engine.start_delay_ms = Math.max(0, safeInt(startDelayInput.value, 0)) * 1000;
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
        currentConfig.hotkeys = { toggle: "R / K", mode_switch: "Ctrl+Alt+M", emergency_stop: "Escape", speed_up: "Ctrl+=", slow_down: "Ctrl+-", capture_pos: "Ctrl+P", record_hotkey: "Ctrl+Shift+R" };
      }
      if (rippleCheckbox) currentConfig.ui.visual_ripple = rippleCheckbox.checked;

      if (!currentConfig.ui) currentConfig.ui = {};
      if (startMinimizedCheckbox) currentConfig.ui.start_minimized = startMinimizedCheckbox.checked;
      if (autostartCheckbox) currentConfig.ui.autostart = autostartCheckbox.checked;
      if (minimizeToTrayCheckbox) currentConfig.ui.minimize_to_tray = minimizeToTrayCheckbox.checked;
      if (notificationsCheckbox) currentConfig.ui.show_notifications = notificationsCheckbox.checked;
      if (pauseFocusLossCheckbox) currentConfig.ui.pause_on_focus_loss = pauseFocusLossCheckbox.checked;
      if (themeSelect) currentConfig.ui.theme = themeSelect.value;

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
  saveConfig();
});

// ── CPS slider + number input (Dashboard) ────────────────────
// Mirrors the slider into the number input and vice versa, updates the
// live counter, then persists via saveConfig(). Mirrors limitRange pattern.
if (cpsRange) cpsRange.addEventListener("input", (e) => {
  const val = parseFloat(e.target.value);
  const safeVal = isNaN(val) ? 29 : Math.max(1, Math.min(100, val));
  if (cpsInput) cpsInput.value = safeVal;
  if (displayCps) displayCps.textContent = safeVal.toFixed(1);
  if (currentConfig?.engine) currentConfig.engine.target_cps = safeVal;
  saveConfig();
});
if (cpsInput) cpsInput.addEventListener("input", (e) => {
  const val = parseFloat(e.target.value);
  const safeVal = isNaN(val) ? 29 : Math.max(1, Math.min(100, val));
  if (cpsRange) {
    // expand slider max if user types a value above current max
    const currentMax = parseFloat(cpsRange.max) || 100;
    if (safeVal > currentMax) cpsRange.max = safeVal;
    cpsRange.value = safeVal;
  }
  if (displayCps) displayCps.textContent = safeVal.toFixed(1);
  if (currentConfig?.engine) currentConfig.engine.target_cps = safeVal;
  saveConfig();
});

// ── Jitter slider + number input (Dashboard) ─────────────────
if (randomRange) randomRange.addEventListener("input", (e) => {
  const val = parseFloat(e.target.value);
  const safeVal = isNaN(val) ? 0 : Math.max(0, Math.min(30, val));
  if (randomInput) randomInput.value = safeVal;
  if (currentConfig?.engine) currentConfig.engine.jitter_percent = safeVal;
  saveConfig();
});
if (randomInput) randomInput.addEventListener("input", (e) => {
  const val = parseFloat(e.target.value);
  const safeVal = isNaN(val) ? 0 : Math.max(0, Math.min(30, val));
  if (randomRange) randomRange.value = safeVal;
  if (currentConfig?.engine) currentConfig.engine.jitter_percent = safeVal;
  saveConfig();
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
if (jitterRadiusInput) jitterRadiusInput.addEventListener("change", saveConfig);
if (rippleCheckbox) rippleCheckbox.addEventListener("change", saveConfig);

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
    activeRecordingBtn = btn;
    btn.classList.add("recording");
    const labelEl = btn.querySelector("span:last-child");
    labelEl.textContent = "Press key...";

    const pressedKeys = [];
    let finishTimeout = null;

    const handleKeyDown = (e) => {
      e.preventDefault();
      e.stopPropagation();

      const physicalName = codeToPhysicalKey(e.code, e.key);
      if (!pressedKeys.includes(physicalName)) {
        pressedKeys.push(physicalName);
      }

      const modifiers = [];
      const regularKeys = [];
      for (const k of pressedKeys) {
        if (["Ctrl", "Alt", "Shift"].includes(k)) {
          if (!modifiers.includes(k)) modifiers.push(k);
        } else {
          if (!regularKeys.includes(k)) regularKeys.push(k);
        }
      }

      const bindingStr = [...modifiers, ...regularKeys].join("+");
      labelEl.textContent = bindingStr || "Press key...";

      if (finishTimeout) clearTimeout(finishTimeout);
      finishTimeout = setTimeout(() => {
        finalizeRecording(bindingStr);
      }, 400);
    };

    const handleKeyUp = () => {
      if (pressedKeys.length > 0) {
        if (finishTimeout) clearTimeout(finishTimeout);
        finishTimeout = setTimeout(() => {
          const modifiers = [];
          const regularKeys = [];
          for (const k of pressedKeys) {
            if (["Ctrl", "Alt", "Shift"].includes(k)) {
              if (!modifiers.includes(k)) modifiers.push(k);
            } else {
              if (!regularKeys.includes(k)) regularKeys.push(k);
            }
          }
          const bindingStr = [...modifiers, ...regularKeys].join("+");
          finalizeRecording(bindingStr);
        }, 150);
      }
    };

    function finalizeRecording(bindingStr) {
      if (!bindingStr) return;
      if (targetKey === "toggle") {
        currentConfig.hotkeys.toggle = bindingStr;
      } else if (targetKey === "mode_switch") {
        currentConfig.hotkeys.mode_switch = bindingStr;
        if (footerModeShortcut) footerModeShortcut.textContent = bindingStr;
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
setupHotkeyRecorder(emergencyRecordBtn, "emergency_stop");
setupHotkeyRecorder(speedUpRecordBtn, "speed_up");
setupHotkeyRecorder(slowDownRecordBtn, "slow_down");
setupHotkeyRecorder(pickPosRecordBtn, "capture_pos");

// ── OPEN CONFIG FOLDER BUTTON ────────────────────────────────
if (openConfigFolderBtn) {
  openConfigFolderBtn.addEventListener("click", async () => {
    try {
      await invoke("open_config_folder");
    } catch (err) {
      console.error("Failed to open config folder:", err);
    }
  });
}

// ── BEHAVIOR AUTOSTART LISTENER ──────────────────────────────
if (autostartCheckbox) {
  autostartCheckbox.addEventListener("change", async () => {
    saveConfig();
    try {
      await invoke("set_windows_autostart", { enable: autostartCheckbox.checked });
    } catch (err) {
      console.error("Failed to set Windows autostart:", err);
    }
  });
}
if (startMinimizedCheckbox) startMinimizedCheckbox.addEventListener("change", saveConfig);
if (minimizeToTrayCheckbox) minimizeToTrayCheckbox.addEventListener("change", saveConfig);
if (notificationsCheckbox) notificationsCheckbox.addEventListener("change", saveConfig);
if (pauseFocusLossCheckbox) pauseFocusLossCheckbox.addEventListener("change", saveConfig);

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
  accentSwatches.forEach(swatch => {
    if (swatch.getAttribute("data-accent").toLowerCase() === accentHex.toLowerCase()) {
      swatch.classList.add("active");
    } else {
      swatch.classList.remove("active");
    }
  });
}

if (themeSelect) {
  themeSelect.addEventListener("change", () => {
    const theme = themeSelect.value;
    applyTheme(theme, currentConfig.ui?.accent_color);
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
  modeToggleBtn.addEventListener("click", async () => {
    try {
      const newMode = await invoke("toggle_mode");
      setModeDisplay(newMode);
    } catch (err) {
      console.error("Failed to toggle mode:", err);
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
    const posStr = p.position_mode === "fixed" ? `Fixed (${p.fixed_x},${p.fixed_y})` : "Cursor";
    const clickTypeStr = p.click_type ? (p.click_type.charAt(0).toUpperCase() + p.click_type.slice(1)) : "Single";
    const btnStr = p.button ? (p.button.charAt(0).toUpperCase() + p.button.slice(1)) : "Left";
    const limitStr = p.click_limit > 0 ? `Limit: ${p.click_limit}` : null;
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
            <span class="preset-tag">±${p.jitter_percent}% Jitter</span>
            <span class="preset-tag">${clickTypeStr}</span>
            <span class="preset-tag">${btnStr} Btn</span>
            <span class="preset-tag">${posStr}</span>
            ${limitStr ? `<span class="preset-tag">${limitStr}</span>` : ''}
          </div>
        </div>
        <div class="preset-card-footer">
          <button type="button" class="preset-run-btn" data-action="run" data-id="${escapeHtml(p.id)}" title="Apply and run now">
              ▶ Run
          </button>
          <button type="button" class="preset-apply-btn" data-action="apply" data-id="${escapeHtml(p.id)}" title="Apply preset">
              ⚡ Apply
          </button>
          <button type="button" class="preset-icon-btn" data-action="inspect" data-id="${escapeHtml(p.id)}" title="View details">👁️</button>
          <button type="button" class="preset-icon-btn edit" data-action="edit" data-id="${escapeHtml(p.id)}" title="Edit">✏️</button>
          <button type="button" class="preset-icon-btn danger" data-action="delete" data-id="${escapeHtml(p.id)}" title="Delete">🗑️</button>
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

// Minimal HTML escaper to prevent XSS when preset names contain <, >, &.
function escapeHtml(s) {
  return String(s)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
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
  // Remove any existing context menu.
  document.querySelectorAll(".ve-context-menu").forEach(m => m.remove());

  const rect = targetBtn.getBoundingClientRect();
  const menu = document.createElement("div");
  menu.className = "ve-context-menu";
  menu.style.cssText = `
    position:fixed;left:${rect.right + 4}px;top:${rect.top}px;z-index:100000;
    background:var(--bg-elev);border:1px solid var(--border);border-radius:6px;
    box-shadow:0 8px 24px rgba(0,0,0,0.5);min-width:180px;padding:4px 0;
  `;
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
    a.addEventListener("click", () => { menu.remove(); it.action(); });
    menu.appendChild(a);
  }
  document.body.appendChild(menu);
  // Close on click-away.
  setTimeout(() => {
    const onDoc = (ev) => {
      if (!menu.contains(ev.target)) {
        menu.remove();
        document.removeEventListener("click", onDoc, true);
      }
    };
    document.addEventListener("click", onDoc, true);
  }, 0);
}

async function reRecordMacro(macro, onChange) {
  if (!confirm(`Re-record "${macro.name}"? The existing actions will be replaced once you stop recording.`)) {
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
      currentConfig.engine.jitter_radius_px = Number(p.jitter_radius_px) || 3;
      currentConfig.engine.repeat_mode = p.repeat_mode || "unlimited";
      currentConfig.engine.repeat_count = Number(p.repeat_count) || 0;
      currentConfig.engine.repeat_interval_ms = p.repeat_interval_ms == null ? 1000 : Number(p.repeat_interval_ms);
      currentConfig.engine.start_delay_ms = Number(p.start_delay_ms) || 0;
      currentConfig.engine.stop_duration_min = Number(p.stop_duration_min) || 0;
      currentConfig.engine.stop_time_str = p.stop_time_str || "";
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
      showToast("⚠️ Switch to Autoclicker mode to start", "warn");
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

  if (title) title.textContent = `🔍 Preset details: ${p.icon || ''} ${p.name}`;

  const intervalMs = (1000 / p.target_cps).toFixed(2);

  if (body) {
    body.innerHTML = `
      <div class="inspect-row">
        <span class="inspect-label">Name:</span>
        <span class="inspect-value">${p.name}</span>
      </div>
      <div class="inspect-row">
        <span class="inspect-label">Click speed (CPS):</span>
        <span class="inspect-value">${p.target_cps} CPS (${intervalMs} ms)</span>
      </div>
      <div class="inspect-row">
        <span class="inspect-label">Randomization (Jitter):</span>
        <span class="inspect-value">±${p.jitter_percent}%</span>
      </div>
      <div class="inspect-row">
        <span class="inspect-label">Click type:</span>
        <span class="inspect-value">${p.click_type || 'single'}</span>
      </div>
      <div class="inspect-row">
        <span class="inspect-label">Mouse button:</span>
        <span class="inspect-value">${p.button || 'left'}</span>
      </div>
      <div class="inspect-row">
        <span class="inspect-label">Position mode:</span>
        <span class="inspect-value">${p.position_mode === 'fixed' ? `Fixed (X: ${p.fixed_x}, Y: ${p.fixed_y})` : 'Follow cursor'}</span>
      </div>
      <div class="inspect-row">
        <span class="inspect-label">Click limit:</span>
        <span class="inspect-value">${p.click_limit > 0 ? `${p.click_limit} clicks` : 'Unlimited'}</span>
      </div>
    `;
  }

  if (applyBtn) {
    applyBtn.onclick = () => {
      modal.classList.add("hidden");
      void applyPreset(p.id);
    };
  }

  modal.classList.remove("hidden");
}

function openPresetEditModal(p = null) {
  const modal = document.getElementById("presetEditModal");
  const editIdInput = document.getElementById("presetEditId");
  const nameInput = document.getElementById("presetNameInput");
  const iconSelect = document.getElementById("presetIconSelect");
  const cpsRange = document.getElementById("presetCpsRange");
  const cpsVal = document.getElementById("presetCpsVal");
  const jitterRange = document.getElementById("presetJitterRange");
  const jitterVal = document.getElementById("presetJitterVal");
  const clickTypeSelect = document.getElementById("presetClickTypeSelect");
  const holdDurationInput = document.getElementById("presetHoldDuration");
  const holdIntervalInput = document.getElementById("presetHoldInterval");
  const buttonSelect = document.getElementById("presetButtonSelect");
  const positionSelect = document.getElementById("presetPositionSelect");
  const fixedXInput = document.getElementById("presetFixedX");
  const fixedYInput = document.getElementById("presetFixedY");
  const clickLimitInput = document.getElementById("presetClickLimit");
  const startDelayInput = document.getElementById("presetStartDelaySec");
  const stopDurationInput = document.getElementById("presetStopDurationMin");
  const stopTimeInput = document.getElementById("presetStopTime");
  const repeatModeSelect = document.getElementById("presetRepeatModeSelect");
  const repeatCountInput = document.getElementById("presetRepeatCount");
  const repeatIntervalInput = document.getElementById("presetRepeatInterval");
  const coordRow = document.getElementById("presetFixedCoordRow");
  const modalTitle = document.getElementById("presetModalTitle");

  if (p) {
    if (modalTitle) modalTitle.textContent = "✏️ Edit Preset";
    if (editIdInput) editIdInput.value = p.id;
    if (nameInput) nameInput.value = p.name || "";
    if (iconSelect) iconSelect.value = p.icon || "⚡";
    if (cpsRange) {
      cpsRange.value = p.target_cps || 29;
      if (cpsVal) cpsVal.textContent = p.target_cps || 29;
    }
    if (jitterRange) {
      jitterRange.value = p.jitter_percent || 7.5;
      if (jitterVal) jitterVal.textContent = p.jitter_percent || 7.5;
    }
    if (clickTypeSelect) clickTypeSelect.value = p.click_type || "single";
    if (holdDurationInput) holdDurationInput.value = p.hold_duration_ms ?? 500;
    if (holdIntervalInput) holdIntervalInput.value = p.hold_interval_ms ?? 1000;
    if (buttonSelect) buttonSelect.value = p.button || "left";
    if (positionSelect) positionSelect.value = p.position_mode || "cursor";
    if (fixedXInput) fixedXInput.value = p.fixed_x ?? 100;
    if (fixedYInput) fixedYInput.value = p.fixed_y ?? 100;
    if (clickLimitInput) clickLimitInput.value = p.click_limit || 0;
    if (startDelayInput) startDelayInput.value = Math.round((p.start_delay_ms || 0) / 1000);
    if (stopDurationInput) stopDurationInput.value = p.stop_duration_min || 0;
    if (stopTimeInput) stopTimeInput.value = p.stop_time_str || "";
    if (repeatModeSelect) repeatModeSelect.value = p.repeat_mode || "unlimited";
    if (repeatCountInput) repeatCountInput.value = p.repeat_count || 0;
    if (repeatIntervalInput) repeatIntervalInput.value = p.repeat_interval_ms ?? 1000;
  } else {
    if (modalTitle) modalTitle.textContent = "✨ New Preset";
    if (editIdInput) editIdInput.value = "";
    if (nameInput) nameInput.value = "New Preset";
    if (iconSelect) iconSelect.value = "⚡";
    if (cpsRange) {
      cpsRange.value = currentConfig.engine.target_cps || 29;
      if (cpsVal) cpsVal.textContent = currentConfig.engine.target_cps || 29;
    }
    if (jitterRange) {
      jitterRange.value = currentConfig.engine.jitter_percent || 7.5;
      if (jitterVal) jitterVal.textContent = currentConfig.engine.jitter_percent || 7.5;
    }
    if (clickTypeSelect) clickTypeSelect.value = currentConfig.engine.click_type || "single";
    if (holdDurationInput) holdDurationInput.value = currentConfig.engine.hold_duration_ms ?? 500;
    if (holdIntervalInput) holdIntervalInput.value = currentConfig.engine.hold_interval_ms ?? 1000;
    if (buttonSelect) buttonSelect.value = currentConfig.engine.button || "left";
    if (positionSelect) positionSelect.value = currentConfig.engine.position_mode || "cursor";
    if (fixedXInput) fixedXInput.value = currentConfig.engine.fixed_x ?? 100;
    if (fixedYInput) fixedYInput.value = currentConfig.engine.fixed_y ?? 100;
    if (clickLimitInput) clickLimitInput.value = currentConfig.engine.click_limit || 0;
    if (startDelayInput) startDelayInput.value = Math.round((currentConfig.engine.start_delay_ms || 0) / 1000);
    if (stopDurationInput) stopDurationInput.value = currentConfig.engine.stop_duration_min || 0;
    if (stopTimeInput) stopTimeInput.value = currentConfig.engine.stop_time_str || "";
    if (repeatModeSelect) repeatModeSelect.value = currentConfig.engine.repeat_mode || "unlimited";
    if (repeatCountInput) repeatCountInput.value = currentConfig.engine.repeat_count || 0;
    if (repeatIntervalInput) repeatIntervalInput.value = currentConfig.engine.repeat_interval_ms ?? 1000;
  }

  if (positionSelect && coordRow) {
    coordRow.classList.toggle("hidden", positionSelect.value !== "fixed");
  }

  modal.classList.remove("hidden");
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
        stop_time_str: stopTimeStr
      };
    }
  } else {
    const newId = "preset_" + Date.now();
    currentConfig.presets.push({
      id: newId,
      name,
      description: `${cps} CPS | ${clickType}`,
      icon,
      target_cps: cps,
      jitter_percent: jitter,
      jitter_radius_px: 3,
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
      is_default: false
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

// Preset listeners setup. Keep all controls in one idempotent setup so a
// rerender or a delayed WebView DOM cannot leave only some buttons active.
function setupPresetListeners() {
  const on = (id, event, handler) => {
    const element = document.getElementById(id);
    if (!element || element.dataset.presetBound === "1") return;
    element.addEventListener(event, handler);
    element.dataset.presetBound = "1";
  };

  on("createNewPresetBtn", "click", () => openPresetEditModal(null));
  on("saveCurrentAsPresetBtn", "click", () => {
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
      stop_time_str: currentConfig.engine.stop_time_str
    });
  });
  on("presetSaveBtn", "click", () => void savePresetFromModal());
  on("presetCancelBtn", "click", () => {
    document.getElementById("presetEditModal")?.classList.add("hidden");
  });
  on("inspectCloseBtn", "click", () => {
    document.getElementById("presetInspectModal")?.classList.add("hidden");
  });

  on("exportPresetsBtn", "click", () => {
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
      alert("Failed to export presets.");
    }
  });

  on("importPresetsBtn", "click", () => {
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
            target_cps: Math.max(1, Math.min(100, Number(p.target_cps))),
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
        alert(`Successfully imported ${imported.length} presets`);
      } catch (err) {
        console.error("[Presets] import failed:", err);
        alert("Invalid format or empty presets file.");
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
    if (toggleBtnText) toggleBtnText.textContent = "START AUTOMATION";
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
        if (toggleBtnText) toggleBtnText.textContent = "RUNNING (CLICK LOCKED)";
        return `pointerEvents=none for ${parseInt(guiLockDelayInput?.value, 10) || 1500}ms`;
      });

      await op.run("start-gui-lock-timer", () => {
        const lockDuration = parseInt(guiLockDelayInput?.value, 10) || 1500;
        if (guiLockTimer) clearTimeout(guiLockTimer);
        guiLockTimer = setTimeout(() => {
          isButtonLocked = false;
          if (toggleBtn) toggleBtn.style.pointerEvents = "auto";
          if (isRunning && toggleBtnText) toggleBtnText.textContent = "STOP AUTOMATION";
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
  // Busy flag prevents double-fire while a previous toggle is mid-flight.
  let toggleBusy = false;

  toggleBtn.addEventListener("click", async () => {
    const clickOp = stage("Click");
    let clickTime;
    try {
      clickTime = performance.now();

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

      const startDelaySec = parseInt(document.getElementById("startDelayInput")?.value, 10) || 0;

      // ── STAGE 5: Was a start-delay configured? ──────────────────────
      if (startDelaySec > 0 && !isRunning) {
        await clickOp.run("begin-countdown", () => {
          let remaining = startDelaySec;
          if (toggleBtnText) toggleBtnText.textContent = `STARTING IN ${remaining}s...`;
          if (toggleBtn) toggleBtn.style.pointerEvents = "none";

          startDelayTimer = setInterval(async () => {
            remaining--;
            if (remaining > 0) {
              if (toggleBtnText) toggleBtnText.textContent = `STARTING IN ${remaining}s...`;
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
if (onboardingBtn) {
  onboardingBtn.addEventListener("click", async () => {
    try {
      const config = await invoke("complete_onboarding");
      onboardingModal.classList.add("hidden");
      updateUiFromConfig(config);
    } catch (err) {
      console.error("Failed to complete onboarding:", err);
    }
  });
}

// ── STATE DISPLAY ───────────────────────────────────────────
function setRunningState(active, statusText = "") {
  isRunning = active;
  if (!active) {
    clearAutoTimers();
  }
  if (currentConfig.active_mode === "work") {
    if (toggleBtn) {
      toggleBtn.disabled = true;
      toggleBtn.classList.add("disabled-mode");
      if (toggleBtnText) toggleBtnText.textContent = "DISABLED IN WORK MODE";
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
      if (toggleBtnText) toggleBtnText.textContent = "RUNNING (CLICK LOCKED)";
      if (toggleBtn) toggleBtn.style.pointerEvents = "none";
    } else {
      // Lock already released (or never set). Make sure pointerEvents is "auto"
      // — it could be stuck on "none" from a previous run if the lock timer
      // fired during a `setRunningState(false)` window.
      if (toggleBtn) toggleBtn.style.pointerEvents = "auto";
      if (toggleBtnText) toggleBtnText.textContent = "STOP AUTOMATION";
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
    if (toggleBtnText) toggleBtnText.textContent = "START AUTOMATION";
  }
}

// ── IPC LISTENER ────────────────────────────────────────────
// Rust emits status-update on every click (~29/s at default CPS). That's
// useful for the live click counter but produces ~30 log lines per second.
// Use listenSilent (no auto per-event log) and throttle manually — UI
// updates on every tick, log only at meaningful boundaries.
let _lastStatusLogAt = 0;
let _lastStatusLogActive = null;
let _lastStatusLogClicks = -1;

listenSilent("status-update", (event) => {
  const { active, mode, clicks_done, cps, status_text } = event.payload;

  // Always update UI on every tick — no throttling here.
  if (mode) setModeDisplay(mode);
  setRunningState(active, status_text);
  if (clickCounter) {
    const n = clicks_done || 0;
    clickCounter.textContent = n.toLocaleString().padStart(11, "0").replace(/,/g, ",");
  }

  // Throttled log: skip unless (a) state changed, (b) click count crossed a
  // 50 boundary, or (c) ≥500ms since last log.
  const now = performance.now();
  const stateChanged = active !== _lastStatusLogActive;
  const milestoneCrossed = Math.floor((clicks_done || 0) / 50) !==
                           Math.floor(_lastStatusLogClicks / 50);
  const intervalElapsed = now - _lastStatusLogAt >= 500;
  if (stateChanged || milestoneCrossed || intervalElapsed) {
    logCall("←EVT•",
      `status-update [active=${active} mode=${mode} clicks=${clicks_done} cps=${cps?.toFixed?.(1) ?? cps} text="${status_text}"]`, "");
    _lastStatusLogAt = now;
    _lastStatusLogActive = active;
    _lastStatusLogClicks = clicks_done || 0;
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
async function openBuildMode() {
  const name = prompt("Name your macro:", "Custom macro");
  if (!name || !name.trim()) return;
  const m = emptyMacro(name.trim());
  await saveMacro(m);
  await renderMacroList();
  const fresh = (await invoke("list_macros")).find(x => x.id === m.id);
  if (fresh) openVisualEditor(fresh, renderMacroList);
}

// ── ⚡ OPTIMIZE TOGGLE ────────────────────────────────────────────────
let optimizeLevel = "balanced"; // subtle | balanced | aggressive
function cycleOptimizeLevel() {
  optimizeLevel = optimizeLevel === "subtle"
    ? "balanced"
    : optimizeLevel === "balanced"
    ? "aggressive"
    : "subtle";
  refreshOptimizeBtn();
}
function refreshOptimizeBtn() {
  const btn = document.getElementById("automationOptimizeBtn");
  if (!btn) return;
  const icons = { subtle: "🌙", balanced: "⚡", aggressive: "🔥" };
  btn.textContent = `${icons[optimizeLevel]} ${optimizeLevel[0].toUpperCase() + optimizeLevel.slice(1)}`;
  btn.title = `Optimize level: ${optimizeLevel}`;
}

async function initAutomationTab() {
  const recordBtn = document.getElementById("automationRecordBtn");
  const stopBtn = document.getElementById("automationStopRecordBtn");
  const buildBtn = document.getElementById("automationBuildBtn");
  const optBtn = document.getElementById("automationOptimizeBtn");
  const status = document.getElementById("automationRecordingStatus");
  refreshOptimizeBtn();
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
  if (optBtn) {
    optBtn.addEventListener("click", () => cycleOptimizeLevel());
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
  if (countEl) countEl.textContent = `${macros.length} macro${macros.length === 1 ? "" : "s"}`;
  if (macros.length === 0) {
    listEl.innerHTML = `
      <div class="empty-state" style="color:var(--text-dim);padding:20px;text-align:center;border:1px dashed var(--border);border-radius:6px;">
        No macros yet. Hit 🔴 Record or + Build to create your first one.
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
          <button class="btn-mini btn-play" data-id="${m.id}">▶ Play</button>
          <button class="btn-mini btn-optimize" data-id="${m.id}" title="Optimize recorded actions">⚡</button>
          <button class="btn-mini btn-edit" data-id="${m.id}" title="Visual Editor (v3.3)">✎</button>
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
          alert("Failed to rename macro.");
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
      try {
        await saveMacro(m);
        console.log(`[MacroOptimize] ${m.name}: ${before} → ${m.actions.length} actions`);
        await renderMacroList();
      } catch (err) {
        console.error("[MacroOptimize] save failed:", err);
        alert("Failed to save optimized macro.");
      }
    });
  });
  listEl.querySelectorAll(".btn-delete").forEach(btn => {
    btn.addEventListener("click", async (ev) => {
      const id = ev.target.dataset.id;
      if (!confirm("Delete this macro?")) return;
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
            <div class="ve-subtitle">Arrange action blocks into a visual sequence</div>
          </div>
          <button id="veCloseBtn" style="background:none;border:none;color:var(--text-bright);font-size:18px;cursor:pointer;">✕</button>
        </div>
        <div class="ve-palette">
          <span class="ve-palette-label">ADD BLOCK</span>
          <button class="ve-add ve-add-wait" data-type="wait">＋ Wait</button>
          <button class="ve-add ve-add-click" data-type="click">＋ Click</button>
          <button class="ve-add ve-add-move" data-type="move">＋ Move</button>
          <button class="ve-add ve-add-key" data-type="key">＋ Key</button>
          <button id="veOptimizeBtn" class="btn-mini" title="Remove noise and simplify recorded actions">⚡ Optimize</button>
        </div>
        <div id="veActionsList" class="ve-canvas"></div>
        <div style="margin-top:14px;display:flex;justify-content:space-between;align-items:center;gap:8px;">
          <span id="veSummary" style="color:var(--text-dim);font-size:12px;"></span>
          <div style="display:flex;gap:8px;">
            <button id="veReRecordBtn" class="btn-secondary" title="Re-record this macro">⏺ Re-record</button>
            <button id="veSaveBtn" class="btn-primary">💾 Save</button>
            <button id="veCancelBtn" class="btn-secondary">Cancel</button>
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
        showRowContextMenu(b, i, macro, refresh, closeEditor);
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

  const close = () => document.getElementById("visualEditorBackdrop")?.remove();
  document.getElementById("veCloseBtn").addEventListener("click", close);
  document.getElementById("veCancelBtn").addEventListener("click", close);
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
      alert("Save failed: " + e);
    }
  });
}

/// Render a single editor row. Returns HTML.
function renderEditorRow(a, idx, enabled) {
  const safeType = String(a?.type || "unknown").replace(/[^a-z0-9_-]/gi, "-");
  const actionMeta = {
    mouse_move: ["🖱", "MOVE", "Position cursor"],
    mouse_click: ["◉", "CLICK", "Mouse action"],
    mouse_down: ["▼", "MOUSE DOWN", "Hold button"],
    mouse_up: ["▲", "MOUSE UP", "Release button"],
    key_press: ["⌨", "KEY PRESS", "Keyboard action"],
    key_down: ["⌨", "KEY DOWN", "Hold key"],
    key_up: ["⌨", "KEY UP", "Release key"],
    scroll: ["↕", "SCROLL", "Wheel action"],
    wait: ["◷", "WAIT", "Pause sequence"],
    mouse_click: ["◉", "CLICK", "Mouse action"],
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
window.addEventListener("DOMContentLoaded", () => {
  // Set up the console → Tauri debug_log pipe FIRST so every subsequent
  // console.log/error/warn (including the "DOM ready" message below) is
  // captured. Without this order, the first few logs use the original
  // console.log and never reach nanoclick_web.log.
  const origLog = console.log;
  const origError = console.error;
  const origWarn = console.warn;
  const sendToBackend = async (level, args) => {
    try {
      const msg = args.map(a => {
        try { return typeof a === 'string' ? a : JSON.stringify(a); }
        catch { return String(a); }
      }).join(' ');
      // Use the Tauri 2.x internals (or legacy `__TAURI__.core.invoke` for old builds).
      const inv = (typeof window !== "undefined" && window.__TAURI_INTERNALS__?.invoke)
        || window.__TAURI__?.core?.invoke;
      if (inv) {
        await inv("debug_log", { level, message: msg });
      }
    } catch (_) {}
  };
  console.log = (...args) => { origLog.apply(console, args); sendToBackend("info", args); };
  console.error = (...args) => { origError.apply(console, args); sendToBackend("error", args); };
  console.warn = (...args) => { origWarn.apply(console, args); sendToBackend("warn", args); };
  window.addEventListener("error", (e) => {
    console.error("[uncaught]", e.message, "at", e.filename + ":" + e.lineno);
  });
  window.addEventListener("unhandledrejection", (e) => {
    console.error("[unhandled-rejection]", e.reason);
  });

  loadConfig();
  syncVersionDisplay();
  checkPlatformCapabilities();
  initAutomationTab();
  startUpdateChecker();

  console.log("[NanoClick] window.__TAURI__:", typeof window.__TAURI__, window.__TAURI__ ? Object.keys(window.__TAURI__) : "");
  console.log("[NanoClick] TAURI.core.invoke type:", typeof TAURI?.core?.invoke);
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
document.addEventListener("DOMContentLoaded", () => {
  // Engine inputs that need explicit auto-save (debounce/start-delay/stop-time).
  // Note: most engine inputs already wire saveConfig via the L760-L800 change
  // listeners; these three were missing — without explicit input/change
  // listeners they never persisted across restarts.
  const startDelayInputEl = document.getElementById("startDelayInput");
  const stopDurationInputEl = document.getElementById("stopDurationInput");
  const stopTimeInputEl = document.getElementById("stopTimeInput");
  if (startDelayInputEl) startDelayInputEl.addEventListener("input", () => { if (typeof saveConfig === "function") saveConfig(); });
  if (stopDurationInputEl) stopDurationInputEl.addEventListener("input", () => { if (typeof saveConfig === "function") saveConfig(); });
  if (stopTimeInputEl) stopTimeInputEl.addEventListener("input", () => { if (typeof saveConfig === "function") saveConfig(); });

  const slider = document.getElementById("debounceSlider");
  if (slider) slider.addEventListener("input", () => {
    applyDebounceFromConfig(slider.value);
    if (typeof currentConfig !== "undefined" && currentConfig && currentConfig.engine) {
      currentConfig.engine.hotkey_debounce_ms = Math.max(5, Math.min(250, parseInt(slider.value) || 80));
    }
    if (typeof saveConfig === "function") saveConfig();
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
