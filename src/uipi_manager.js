// ── nanoclick UIPI & Elevation Manager ─────────────────────────
// Independent module for handling Windows UIPI (User Interface Privilege Isolation),
// UAC elevation checks, audio warning chimes, window focusing, and admin restart.

// Locale lookup with an English fallback. Module scope on purpose: it captures
// nothing (window.I18nEngine is read lazily at call time), so rebuilding it on
// every badge update only allocated garbage — oxlint: consistent-function-scoping.
const tI18n = (k, fb) => (window.I18nEngine ? window.I18nEngine.t(k, {}, fb) : fb);

  const UipiManager = {
  isElevated: false,
  alwaysAdmin: false,
  scheduledTask: false,
  _audioCtx: null,

  async init(config) {
    this.setupListeners();
    this.setupModalControls();
    await this.refreshElevationStatus();
    this.syncSettingCheckbox(config);
  },

  async refreshElevationStatus() {
    try {
      if (window.__TAURI__?.core?.invoke) {
        const info = await window.__TAURI__.core.invoke("check_elevation");
        if (info && typeof info === "object") {
          this.isElevated = !!info.is_elevated;
          this.alwaysAdmin = !!info.always_run_as_admin;
          this.scheduledTask = !!info.scheduled_elevated_task;
          this.updateHeaderBadge();
          this.syncScheduledTaskCheckbox();
        }
      }
    } catch (e) {
      console.warn("[UIPI] Failed to check elevation:", e);
    }
  },

  updateHeaderBadge() {
    const badge = document.getElementById("adminStatusBadge");
    const text = document.getElementById("adminBadgeText");
    if (!badge || !text) return;

    if (this.isElevated) {
      badge.className = "admin-status-badge elevated";
      badge.title = tI18n("admin_badge_title_elevated", "nanoclick is running as Administrator (no UIPI restrictions)");
      text.textContent = tI18n("admin_badge_admin", "Admin 🛡️");
    } else {
      badge.className = "admin-status-badge standard";
      badge.title = tI18n("admin_badge_title_standard", "Standard mode. Click to restart as Administrator");
      text.textContent = tI18n("admin_badge_standard", "Standard");
    }
  },

  syncSettingCheckbox(config) {
    const cb = document.getElementById("alwaysRunAsAdminCheckbox");
    if (cb) {
      cb.checked = (config?.ui?.always_run_as_admin ?? false) || this.alwaysAdmin;
      cb.addEventListener("change", async () => {
        const enabled = cb.checked;
        if (config?.ui) config.ui.always_run_as_admin = enabled;
        try {
          if (window.__TAURI__?.core?.invoke) {
            await window.__TAURI__.core.invoke("set_always_run_as_admin", { enabled });
          }
        } catch (err) {
          console.error("[UIPI] Failed to update always_run_as_admin:", err);
        }
        if (window.saveConfig) window.saveConfig();
      });
    }

    const modalCb = document.getElementById("uipiAlwaysAdminCheckbox");
    if (modalCb) {
      modalCb.checked = cb ? cb.checked : this.alwaysAdmin;
      modalCb.addEventListener("change", async () => {
        const enabled = modalCb.checked;
        if (cb) cb.checked = enabled;
        if (config?.ui) config.ui.always_run_as_admin = enabled;
        try {
          if (window.__TAURI__?.core?.invoke) {
            await window.__TAURI__.core.invoke("set_always_run_as_admin", { enabled });
          }
        } catch (err) {
          console.error("[UIPI] Failed to update always_run_as_admin:", err);
        }
        if (window.saveConfig) window.saveConfig();
      });
    }

    const schedCb = document.getElementById("uipiScheduledTaskCheckbox");
    if (schedCb) {
      schedCb.checked = this.scheduledTask;
      schedCb.addEventListener("change", async () => {
        const enabled = schedCb.checked;
        try {
          if (window.__TAURI__?.core?.invoke) {
            if (enabled) {
              await window.__TAURI__.core.invoke("register_elevated_logon_task");
            } else {
              await window.__TAURI__.core.invoke("unregister_elevated_logon_task");
            }
          }
          this.scheduledTask = enabled;
        } catch (err) {
          // A failed schtasks run (no consent, no rights) must not leave the
          // box checked: the UI reports the state that EXISTS, not the one
          // the user requested (same rule as the tray toggle).
          schedCb.checked = false;
          this.scheduledTask = false;
          // NOTE: `showToast` from main.js is NOT reachable here — main.js is
          // an ES module, its top-level functions are module-scoped and
          // invisible to this classic script. Same failure path as
          // `restartAsAdmin` below: console + alert.
          const msg = String(err && err.message ? err.message : err);
          console.error("[UIPI] Elevated logon task failed:", err);
          alert(msg);
        }
        this.syncScheduledTaskCheckbox();
      });
      this.syncScheduledTaskCheckbox();
    }
  },

  syncScheduledTaskCheckbox() {
    const schedCb = document.getElementById("uipiScheduledTaskCheckbox");
    if (schedCb) schedCb.checked = !!this.scheduledTask;
    const hint = document.getElementById("uipiScheduledTaskHint");
    if (hint) hint.classList.toggle("hidden", !this.scheduledTask);
  },

  setupListeners() {
    if (window.__TAURI__?.event?.listen) {
      window.__TAURI__.event.listen("uipi-blocked", (event) => {
        this.handleUipiBlocked(event.payload);
      });
    }

    const badge = document.getElementById("adminStatusBadge");
    if (badge) {
      badge.addEventListener("click", () => {
        if (!this.isElevated) {
          this.showModal();
        }
      });
    }

    window.addEventListener("nanoclick-language-changed", () => {
      this.updateHeaderBadge();
    });
  },

  setupModalControls() {
    const modal = document.getElementById("uipiModal");
    const closeBtn = document.getElementById("uipiCancelBtn");
    const closeX = document.getElementById("uipiCloseXBtn");
    const restartBtn = document.getElementById("uipiRestartAdminBtn");

    if (closeBtn) closeBtn.addEventListener("click", () => this.hideModal());
    if (closeX) closeX.addEventListener("click", () => this.hideModal());
    if (restartBtn) restartBtn.addEventListener("click", () => this.restartAsAdmin());

    if (modal) {
      modal.addEventListener("click", (e) => {
        if (e.target === modal) this.hideModal();
      });
    }

    window.addEventListener("keydown", (e) => {
      if (e.key === "Escape" && modal && !modal.classList.contains("hidden")) {
        this.hideModal();
      }
    });
  },

  // The payload carries the blocked window's details; today the chime + modal is
  // the whole UX, hence the underscore on the otherwise unused parameter.
  handleUipiBlocked(_payload) {
    this.playAlertChime();
    this.focusWindow();
    this.showModal();
  },

  playAlertChime() {
    if (window.SoundManager) {
      window.SoundManager.play("warning");
      return;
    }
    try {
      const AudioContext = window.AudioContext || window.webkitAudioContext;
      if (!AudioContext) return;
      if (!this._audioCtx) this._audioCtx = new AudioContext();
      if (this._audioCtx.state === "suspended") this._audioCtx.resume();

      const now = this._audioCtx.currentTime;
      const osc1 = this._audioCtx.createOscillator();
      const gain1 = this._audioCtx.createGain();

      osc1.type = "sine";
      osc1.frequency.setValueAtTime(587.33, now); // D5
      osc1.frequency.exponentialRampToValueAtTime(880, now + 0.12); // A5

      gain1.gain.setValueAtTime(0.3, now);
      gain1.gain.exponentialRampToValueAtTime(0.01, now + 0.35);

      osc1.connect(gain1);
      gain1.connect(this._audioCtx.destination);

      osc1.start(now);
      osc1.stop(now + 0.35);

      // Second harmonic chime
      const osc2 = this._audioCtx.createOscillator();
      const gain2 = this._audioCtx.createGain();
      osc2.type = "triangle";
      osc2.frequency.setValueAtTime(440, now + 0.15);
      osc2.frequency.exponentialRampToValueAtTime(659.25, now + 0.3);

      gain2.gain.setValueAtTime(0.2, now + 0.15);
      gain2.gain.exponentialRampToValueAtTime(0.01, now + 0.45);

      osc2.connect(gain2);
      gain2.connect(this._audioCtx.destination);

      osc2.start(now + 0.15);
      osc2.stop(now + 0.45);
    } catch (_) {}
  },

  // Bring the window forward so the alert below is readable — but NEVER resurrect
  // one that the app deliberately put in the tray. Closing to the tray is a
  // decision the user made, and a blocked click (the clicker running while the
  // window is hidden is the normal tray case) must not undo it: this used to call
  // show() unconditionally, which is why the window kept coming back on its own.
  // When the window is hidden the backend raises a tray balloon instead, so the
  // alert still reaches the user; the modal stays open, ready for their return.
  async focusWindow() {
    try {
      const win = window.__TAURI__?.window?.getCurrentWindow?.();
      if (!win) return;
      if (typeof win.isVisible === "function" && !(await win.isVisible())) {
        if (window.__TAURI__?.core?.invoke) {
          await window.__TAURI__.core.invoke("uipi_blocked_in_tray");
        }
        return;
      }
      win.unminimize();
      win.show();
      win.setFocus();
    } catch (_) {}
  },

  showModal() {
    const modal = document.getElementById("uipiModal");
    if (modal) modal.classList.remove("hidden");
  },

  hideModal() {
    const modal = document.getElementById("uipiModal");
    if (modal) modal.classList.add("hidden");
  },

  async restartAsAdmin() {
    try {
      if (window.__TAURI__?.core?.invoke) {
        await window.__TAURI__.core.invoke("restart_as_admin");
      }
    } catch (e) {
      const msg = window.I18nEngine ? window.I18nEngine.t("dialog_alert_admin_restart_fail", { err: e }, "Failed to run as administrator: " + e) : ("Failed to run as administrator: " + e);
      alert(msg);
    }
  }
};

if (typeof window !== "undefined") {
  window.UipiManager = UipiManager;
}
