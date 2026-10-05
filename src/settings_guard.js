// ── nanoclick Smart Guard — Typing Guard & App Window Filter ───────
// Independent module for the two guard blocks on the Settings tab:
//   * Typing Guard — short click freeze after real text input. Gameplay
//     keys (WASD, hotbar digits, arrows, modifiers) are filtered out in
//     Rust, so gaming never pauses the clicker.
//   * App filter   — everywhere | whitelist | blacklist, matched against
//     the foreground process image name (e.g. "discord.exe").
//
// State lives in the app config (ui.typing_pause_ms, ui.app_filter_mode,
// ui.app_filter_list). main.js only calls hydrate / collect / render.

const SmartGuard = {
  // Freeze window the checkbox applies. Overwritten from Rust so the
  // value has a single source of truth (guard::TYPING_FREEZE_MS).
  typingFreezeMs: 600,
  _initialized: false,
  _onChange: null,
  _onChangeThrottled: null,
  _list: [],
  /// User-configured ignored keys (layer B of docs/KEY_POLICY.md). Keys bound
  /// to a hotkey are exempt automatically and are NOT stored here — the backend
  /// owns that layer and the user cannot remove it.
  _ignoreKeys: [],
  _ignoreCapturing: false,
  _captureTimer: null,
  _capturing: false,
  /// Picker catalogue: { running: [...], installed: [...] } (lazy, cached).
  _catalog: null,
  _catalogLoading: false,
  _suggestItems: [],
  _suggestIndex: -1,
  _statusTimer: null,
  _statusBusy: false,
  _isTypingGuardEnabled: true,
  _canConfirmUncheck: false,
  _uncheckTimer: null,
  _uncheckInterval: null,
  _uncheckExpireTimer: null,

  /* ── helpers ───────────────────────────────────────────────── */

  _t(key, fallback, params) {
    return window.I18nEngine
      ? window.I18nEngine.t(key, params || {}, fallback)
      : fallback;
  },

  _esc(value) {
    return String(value ?? "").replace(/[&<>"']/g, (c) => ({
      "&": "&amp;",
      "<": "&lt;",
      ">": "&gt;",
      '"': "&quot;",
      "'": "&#39;",
    })[c]);
  },

  _el(id) {
    return document.getElementById(id);
  },

  _mode() {
    const checked = document.querySelector('input[name="appFilterMode"]:checked');
    return checked ? checked.value : "everywhere";
  },

  _setMode(mode) {
    const target = ["whitelist", "blacklist"].includes(mode) ? mode : "everywhere";
    document.querySelectorAll('input[name="appFilterMode"]').forEach((radio) => {
      radio.checked = radio.value === target;
    });
    this._paintMode();
  },

  /* ── lifecycle ─────────────────────────────────────────────── */

  /**
   * Wire the guard controls once. `onChange` is main.js's saveConfig so
   * every guard edit persists through the existing config pipeline;
   * `onChangeThrottled` is the 250 ms-coalesced variant for `input` bursts.
   */
  async init(onChange, onChangeThrottled) {
    if (this._initialized) {
      this._onChange = onChange || this._onChange;
      this._onChangeThrottled = onChangeThrottled || this._onChangeThrottled;
      return;
    }
    this._initialized = true;
    this._onChange = onChange || null;
    this._onChangeThrottled = onChangeThrottled || null;

    // Pull the freeze window from Rust instead of hardcoding it twice.
    try {
      if (window.__TAURI__?.core?.invoke) {
        const defaults = await window.__TAURI__.core.invoke("get_smart_guard_defaults");
        const ms = Number(defaults?.typing_freeze_ms);
        if (Number.isFinite(ms) && ms > 0) this.typingFreezeMs = ms;
      }
    } catch (err) {
      console.warn("[SmartGuard] defaults fetch failed:", err);
    }

    this._wire();
  },

  _paintMode() {
    const current = this._mode();
    document.querySelectorAll('input[name="appFilterMode"]').forEach((radio) => {
      const label = radio.closest(".radio-item");
      if (label) label.classList.toggle("selected", radio.value === current);
    });
  },
  _wire() {
    const typingCb = this._el("typingGuardCheckbox");
    if (typingCb) {
      typingCb.addEventListener("click", (e) => {
        this._handleTypingCbClick(e);
      });
      const popover = this._el("typingGuardPopover");
      if (popover) {
        popover.addEventListener("click", (e) => e.stopPropagation());
      }
    }

    const freezeInput = this._el("typingFreezeInput");
    if (freezeInput) {
      // `change` alone misses spinner clicks (they fire `input` per tick and
      // `change` only on blur/Enter) — the value must reach the config even
      // if the user hits Start right after the spinner. `input` is throttled
      // inside saveConfigThrottled (250 ms), so dragging is still one write.
      freezeInput.addEventListener("input", () => this._changedThrottled());
      freezeInput.addEventListener("change", () => this._changed());
    }

    document.querySelectorAll('input[name="appFilterMode"]').forEach((radio) => {
      radio.addEventListener("change", () => {
        this._setMode(radio.value);
        this._changed();
      });
    });

    const addBtn = this._el("addAppFilterBtn");
    if (addBtn) addBtn.addEventListener("click", () => this._addFromInput());

    this._wireSuggest();


    const modeGroup = this._el("appFilterModeGroup");
    if (modeGroup) {
      modeGroup.addEventListener("change", () => {
        this._paintMode();
        this._changed();
      });
    }
    const captureBtn = this._el("captureAppBtn");
    if (captureBtn) captureBtn.addEventListener("click", () => this.startCapture());

    // Row delete buttons are re-created on every render -> delegate.
    const list = this._el("appFilterList");
    if (list) {
      list.addEventListener("click", (e) => {
        const btn = e.target.closest(".app-filter-del");
        if (!btn) return;
        this.removeEntry(Number(btn.dataset.idx));
      });
    }

    // ── Ignored keys (layer B) ──
    // Names are deliberately distinct from the app-filter bindings above: a
    // duplicate `const` in an ES module is a SyntaxError that kills the WHOLE
    // file — no handler attaches, the UI stays painted and every Rust test
    // still passes (see AGENTS.md §0).
    const ignoreAddBtn = this._el("typingKeyAddBtn");
    if (ignoreAddBtn) {
      ignoreAddBtn.addEventListener("click", async () => {
        const input = this._el("typingKeyEntry");
        const raw = input?.value || "";
        if (input) input.value = "";
        // A comma-separated list is accepted for users who know the keys.
        for (const part of String(raw).split(",")) {
          if (part.trim()) await this.addIgnoredKey(part);
        }
      });
    }
    const ignoreEntry = this._el("typingKeyEntry");
    if (ignoreEntry) {
      ignoreEntry.addEventListener("keydown", (e) => {
        if (e.key !== "Enter") return;
        e.preventDefault();
        const raw = ignoreEntry.value || "";
        ignoreEntry.value = "";
        for (const part of String(raw).split(",")) {
          if (part.trim()) this.addIgnoredKey(part);
        }
      });
    }
    const captureKeyBtn = this._el("typingKeyCaptureBtn");
    if (captureKeyBtn) {
      captureKeyBtn.addEventListener("click", () => this._startIgnoreCapture());
    }
    const ignoreKeyList = this._el("typingKeyList");
    if (ignoreKeyList) {
      ignoreKeyList.addEventListener("click", (e) => {
        const btn = e.target.closest(".typing-key-del");
        if (!btn) return;
        this.removeIgnoredKey(btn.dataset.key);
      });
    }
  },

  /**
   * One-shot key capture. Deliberately NOT a copy of the hotkey recorder: no
   * modifier collection (a modifier is never a gameplay letter) and no TTL
   * timer — one key, one add.
   */
  _startIgnoreCapture() {
    if (this._ignoreCapturing) return;
    const btn = this._el("typingKeyCaptureBtn");
    this._ignoreCapturing = true;
    const restore = btn ? btn.innerHTML : "";
    if (btn) {
      btn.innerHTML = '<span data-i18n="settings_typing_ignore_capture_waiting">Press any key…</span>';
    }
    const finish = () => {
      document.removeEventListener("keydown", onKey, true);
      this._ignoreCapturing = false;
      if (btn) btn.innerHTML = restore;
    };
    const onKey = (e) => {
      e.preventDefault();
      e.stopPropagation();
      // Escape cancels: without this the capture would keep swallowing every
      // key on the page until one is finally pressed.
      if (e.key === "Escape") {
        finish();
        return;
      }
      const label = e.key && e.key.length === 1 ? e.key.toUpperCase() : e.key;
      finish();
      if (label && !["Shift", "Control", "Alt", "Meta"].includes(label)) {
        this.addIgnoredKey(label);
      }
    };
    document.addEventListener("keydown", onKey, true);
  },

  _handleTypingCbClick(e) {
    const typingCb = this._el("typingGuardCheckbox");
    if (!typingCb) return;

    // Case 1: Currently ENABLED -> User wants to DISABLE
    if (this._isTypingGuardEnabled) {
      if (this._canConfirmUncheck) {
        // SECOND CLICK: Confirmed! Allow disabling guard now.
        this._resetUncheckState();
        this._isTypingGuardEnabled = false;
        typingCb.checked = false;
        this._syncStatusPolling();
        this._changed();
        return;
      }

      // FIRST CLICK (or clicking while 2-sec warning is active):
      // Prevent unchecking, keep checkbox checked
      e.preventDefault();
      typingCb.checked = true;

      // Show 2-second safety warning popover
      this._showTypingGuardWarnPopover();
      return;
    }

    // Case 2: Currently DISABLED -> User wants to ENABLE
    this._resetUncheckState();
    this._isTypingGuardEnabled = true;
    typingCb.checked = true;
    this._syncStatusPolling();
    this._changed();
  },

  _showTypingGuardWarnPopover() {
    const popover = this._el("typingGuardPopover");
    const cdText = this._el("typingGuardCdText");
    const progressBar = this._el("typingGuardProgressBar");
    const row = this._el("typingGuardRow");
    if (!popover) return;

    // If already counting down, shake popover to highlight attention
    if (popover.classList.contains("active-countdown")) {
      popover.classList.remove("popover-shake");
      void popover.offsetWidth; // trigger reflow
      popover.classList.add("popover-shake");
      return;
    }

    this._resetUncheckState();

    popover.classList.remove("hidden");
    popover.classList.add("active-countdown");
    if (progressBar) {
      progressBar.style.transition = "none";
      progressBar.style.width = "0%";
      void progressBar.offsetWidth;
      progressBar.style.transition = "width 2s linear";
      progressBar.style.width = "100%";
    }

    let secondsLeft = 2;
    if (cdText) {
      cdText.textContent = this._t("typing_guard_warn_wait", `Review requirements: ${secondsLeft}s...`, { s: secondsLeft });
    }

    this._uncheckInterval = setInterval(() => {
      secondsLeft--;
      if (secondsLeft > 0 && cdText) {
        cdText.textContent = this._t("typing_guard_warn_wait", `Review requirements: ${secondsLeft}s...`, { s: secondsLeft });
      }
    }, 1000);

    // 2-Second timer: disappears after 2s and unlocks confirmation
    this._uncheckTimer = setTimeout(() => {
      if (this._uncheckInterval) clearInterval(this._uncheckInterval);
      popover.classList.remove("active-countdown");
      popover.classList.add("hidden");

      // Unlock second click confirmation
      this._canConfirmUncheck = true;
      if (row) row.classList.add("confirm-ready");

      // Safety expiration: revert to protected state after 7 seconds of inactivity
      this._uncheckExpireTimer = setTimeout(() => {
        this._resetUncheckState();
      }, 7000);
    }, 2000);
  },

  _resetUncheckState() {
    if (this._uncheckTimer) {
      clearTimeout(this._uncheckTimer);
      this._uncheckTimer = null;
    }
    if (this._uncheckInterval) {
      clearInterval(this._uncheckInterval);
      this._uncheckInterval = null;
    }
    if (this._uncheckExpireTimer) {
      clearTimeout(this._uncheckExpireTimer);
      this._uncheckExpireTimer = null;
    }
    this._canConfirmUncheck = false;

    const popover = this._el("typingGuardPopover");
    if (popover) {
      popover.classList.remove("active-countdown", "popover-shake");
      popover.classList.add("hidden");
    }
    const progressBar = this._el("typingGuardProgressBar");
    if (progressBar) {
      progressBar.style.transition = "none";
      progressBar.style.width = "0%";
    }
    const row = this._el("typingGuardRow");
    if (row) row.classList.remove("confirm-ready");
  },

  _changed() {
    if (typeof this._onChange === "function") this._onChange();
    else this.render();
  },

  // Throttled variant for high-frequency `input` events (number spinners).
  // main.js passes saveConfig as onChange, but saveConfig() is immediate —
  // bursts of `input` ticks must go through the throttled wrapper, which
  // coalesces to one disk write per 250 ms with a trailing call, so the
  // FINAL spinner value always lands on disk even without blur/Enter.
  _changedThrottled() {
    if (typeof this._onChangeThrottled === "function") {
      this._onChangeThrottled();
      return;
    }
    this._changed();
  },

  /* ── app picker (running + installed) ──────────────────────── */

  /**
   * Attach autocomplete to the filter input: focusing or typing opens a
   * dropdown of running/installed apps, so the user never has to know a
   * process name by heart.
   */
  _wireSuggest() {
    const entry = this._el("appFilterEntry");
    if (!entry) return;

    entry.addEventListener("focus", () => this._showSuggest());
    entry.addEventListener("input", () => this._showSuggest());

    entry.addEventListener("keydown", (e) => {
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        this._moveSuggest(e.key === "ArrowDown" ? 1 : -1);
      } else if (e.key === "Enter") {
        e.preventDefault();
        if (this._suggestIndex >= 0) this._pickSuggest(this._suggestIndex);
        else this._addFromInput();
      } else if (e.key === "Escape") {
        this._hideSuggest();
      }
    });

    // mousedown (not click) so the row is handled before the input blurs.
    const box = this._el("appFilterSuggest");
    if (box) {
      box.addEventListener("mousedown", (e) => {
        e.preventDefault();
        const row = e.target.closest(".app-suggest-item");
        if (row) this._pickSuggest(Number(row.dataset.idx));
      });
    }

    entry.addEventListener("blur", () => setTimeout(() => this._hideSuggest(), 120));
  },

  /** Fetch the catalogue once, then reuse it for the session. */
  async _loadCatalog() {
    if (this._catalog) return this._catalog;
    if (this._catalogLoading) return { running: [], installed: [] };
    this._catalogLoading = true;

    let catalog = { running: [], installed: [] };
    try {
      if (window.__TAURI__?.core?.invoke) {
        const res = await window.__TAURI__.core.invoke("list_installed_and_running_apps");
        catalog = {
          running: Array.isArray(res?.running) ? res.running : [],
          installed: Array.isArray(res?.installed) ? res.installed : [],
        };
      }
    } catch (err) {
      console.warn("[SmartGuard] app catalogue failed:", err);
    }

    this._catalogLoading = false;
    this._catalog = catalog;
    return catalog;
  },

  /**
   * Ranked matches: running processes first (guaranteed to match the
   * foreground filter), then installed apps that expose a real executable.
   */
  async _computeSuggest() {
    const query = (this._el("appFilterEntry")?.value || "").trim().toLowerCase();
    const { running, installed } = await this._loadCatalog();
    const out = [];
    const seen = new Set();
    const LIMIT = 80;

    for (const item of running) {
      const exe = String(item?.exe || "").toLowerCase();
      if (!exe || (query && !exe.includes(query)) || seen.has(exe)) continue;
      seen.add(exe);
      out.push({ label: exe, exe, source: "running" });
      if (out.length >= LIMIT) break;
    }

    if (out.length < LIMIT) {
      for (const item of installed) {
        const exe = String(item?.exe || "").toLowerCase();
        if (!exe) continue;
        const label = String(item?.label || exe);
        if (query && !label.toLowerCase().includes(query) && !exe.includes(query)) continue;
        if (seen.has(exe)) continue;
        seen.add(exe);
        out.push({ label, exe, source: "installed" });
        if (out.length >= LIMIT) break;
      }
    }
    return out;
  },

  async _showSuggest() {
    const box = this._el("appFilterSuggest");
    if (!box) return;

    this._suggestItems = await this._computeSuggest();
    this._suggestIndex = -1;

    if (this._suggestItems.length === 0) {
      box.innerHTML = `<div class="app-suggest-empty">${this._esc(
        this._t("settings_app_picker_empty", "Nothing found — keep typing the process name"),
      )}</div>`;
      box.style.display = "block";
      return;
    }

    box.innerHTML = this._suggestItems
      .map((item, idx) => {
        const running = item.source === "running";
        const tag = this._t(
          running ? "settings_app_source_running" : "settings_app_source_installed",
          running ? "running" : "installed",
        );
        return `<div class="app-suggest-item" data-idx="${idx}">
          <span class="app-suggest-name">${this._esc(item.label)}</span>
          <span class="app-suggest-tag">${this._esc(tag)}</span>
        </div>`;
      })
      .join("");
    box.style.display = "block";
  },

  _hideSuggest() {
    const box = this._el("appFilterSuggest");
    if (box) box.style.display = "none";
    this._suggestIndex = -1;
  },

  _moveSuggest(delta) {
    const box = this._el("appFilterSuggest");
    if (!box || box.style.display === "none" || this._suggestItems.length === 0) return;
    const next = this._suggestIndex + delta;
    this._suggestIndex = Math.max(0, Math.min(this._suggestItems.length - 1, next));
    box.querySelectorAll(".app-suggest-item").forEach((row, idx) => {
      row.classList.toggle("active", idx === this._suggestIndex);
    });
    const active = box.querySelector(".app-suggest-item.active");
    if (active?.scrollIntoView) active.scrollIntoView({ block: "nearest" });
  },

  _pickSuggest(index) {
    const item = this._suggestItems[index];
    if (!item) return;
    const input = this._el("appFilterEntry");
    if (input) input.value = "";
    this._hideSuggest();
    this.addEntry(item.exe);
  },

  /* ── live guard status ─────────────────────────────────────── */

  /**
   * Start/stop the status poller so the user can *see* the guard working.
   * `text_events` is the decisive diagnostic: if it stays at 0 while typing,
   * either the checkbox is off or the hook is not receiving keys.
   */
  _syncStatusPolling() {
    const enabled = !!this._el("typingGuardCheckbox")?.checked;
    if (enabled && !this._statusTimer) {
      this._renderStatus();
      this._statusTimer = setInterval(() => this._renderStatus(), 400);
    } else if (!enabled && this._statusTimer) {
      clearInterval(this._statusTimer);
      this._statusTimer = null;
      const el = this._el("guardStatus");
      if (el) {
        el.className = "app-guard-status";
        el.textContent = this._t("settings_guard_status_off", "Typing Guard is off");
      }
    }
  },

  async _renderStatus() {
    const el = this._el("guardStatus");
    if (!el || this._statusBusy) return;
    if (!this._el("typingGuardCheckbox")?.checked) return;

    this._statusBusy = true;
    try {
      if (!window.__TAURI__?.core?.invoke) return;
      const st = await window.__TAURI__.core.invoke("get_smart_guard_status");
      const remaining = Number(st?.lock_remaining_ms) || 0;
      const locked = remaining > 0;
      const events = Number(st?.text_events) || 0;
      const windowMs = Number(st?.typing_pause_ms) || 0;

      if (locked) {
        el.className = "app-guard-status frozen";
        el.textContent = this._t(
          "settings_guard_status_frozen",
          "🔒 Clicking stopped — you are typing · {ms} ms left",
          { ms: remaining },
        );
      } else {
        el.className = "app-guard-status";
        el.textContent = events === 0
          ? this._t(
              "settings_guard_status_idle",
              "Stops clicking for {ms} ms after you type (WASD and hotbar ignored)",
              { ms: windowMs },
            )
          : this._t(
              "settings_guard_status_on",
              "Typing Guard active · text keys seen: {n}",
              { n: events },
            );
      }
    } catch (err) {
      console.warn("[SmartGuard] status poll failed:", err);
    } finally {
      this._statusBusy = false;
    }
  },

  /* ── config bridge ─────────────────────────────────────────── */

  /** Freeze window from the input, clamped to the range Rust accepts. */
  _clampFreezeMs() {
    const raw = Number(this._el("typingFreezeInput")?.value);
    if (!Number.isFinite(raw)) return this.typingFreezeMs;
    return Math.max(100, Math.min(5000, Math.round(raw)));
  },

  /** Config -> UI. Called from main.js updateUiFromConfig(). */
  hydrate(config) {
    const ui = config?.ui || {};

    const delayMs = Number(ui.typing_pause_ms);
    const enabled = Number.isFinite(delayMs) ? delayMs > 0 : true;
    this._isTypingGuardEnabled = enabled;
    const typingCb = this._el("typingGuardCheckbox");
    if (typingCb) typingCb.checked = enabled;
    this._resetUncheckState();

    const freezeInput = this._el("typingFreezeInput");
    if (freezeInput) {
      freezeInput.value = String(enabled ? (delayMs > 0 ? delayMs : this.typingFreezeMs) : this.typingFreezeMs);
    }
    this._syncStatusPolling();

    this._setMode(ui.app_filter_mode);
    this._list = Array.isArray(ui.app_filter_list)
      ? ui.app_filter_list.map((s) => String(s).trim().toLowerCase()).filter(Boolean)
      : [];
    this._ignoreKeys = Array.isArray(ui.typing_ignore_keys)
      ? ui.typing_ignore_keys.map((s) => String(s).trim()).filter(Boolean)
      : [];
    this.render();
    this._renderIgnoreKeys();
    this._refreshKeyPolicyReport();
  },

  /** UI -> Config. Called from main.js saveConfig(). */
  collect(config) {
    if (!config) return;
    if (!config.ui) config.ui = {};

    const typingCb = this._el("typingGuardCheckbox");
    if (typingCb) {
      config.ui.typing_pause_ms = typingCb.checked ? this._clampFreezeMs() : 0;
    }

    config.ui.app_filter_mode = this._mode();
    config.ui.app_filter_list = this._list.slice();
    config.ui.typing_ignore_keys = this._ignoreKeys.slice();
  },

  /* ── ignored keys (layer B) ───────────────────────────────────── */

  /** Ask the backend what is in force, so the list can show locked keys. */
  async _refreshKeyPolicyReport() {
    const host = this._el("typingKeyReport");
    if (!host || !window.__TAURI__?.core?.invoke) return;
    try {
      const inv = window.__TAURI__.core.invoke;
      const report = await inv("get_key_policy");
      const extra = (report?.non_seed_keys || []).length;
      host.textContent = this._t(
        "settings_typing_ignore_report",
        { total: report?.exempt_total ?? 0, extra },
        `${report?.exempt_total ?? 0} keys ignored (${extra} added by you or bound to hotkeys)`
      );
    } catch (_) {
      // Diagnostics only — the list itself renders without it.
    }
  },

  /** Render the ignored-key chips. Bound keys appear locked (no delete). */
  _renderIgnoreKeys(locked) {
    const list = this._el("typingKeyList");
    if (!list) return;
    const lockedSet = new Set((locked || []).map((k) => String(k)));
    const all = this._ignoreKeys.concat([...lockedSet].filter((k) => !this._ignoreKeys.includes(k)));
    if (all.length === 0) {
      list.innerHTML = `<div style="color:var(--text-dim);font-size:12px;">${this._esc(
        this._t("settings_typing_ignore_empty", "No extra keys — the built-in set applies.")
      )}</div>`;
      return;
    }
    list.innerHTML = all.map((key) => {
      const isLocked = lockedSet.has(key);
      return `
        <div style="display:flex;align-items:center;gap:6px;font-size:12px;">
          <span style="flex:1;background:var(--bg-elev);border:1px solid var(--border);border-radius:5px;padding:4px 8px;font-family:monospace;">${this._esc(key)}</span>
          ${isLocked
            ? `<span title="${this._esc(this._t("settings_typing_ignore_locked", "Bound to a hotkey — always ignored"))}">🔒</span>`
            : `<button class="typing-key-del preset-modal-btn cancel" data-key="${this._esc(key)}" style="flex:0 0 auto;padding:3px 8px;" title="${this._esc(this._t("settings_typing_ignore_remove", "Remove"))}">✕</button>`}
        </div>`;
    }).join("");
  },

  /**
   * Add one key after asking the backend why it cannot be added.
   * `validate_ignore_key` is what teaches the two-layer model: it reports
   * "already ignored" for the seed and "already bound" for a hotkey key.
   */
  async addIgnoredKey(raw) {
    const label = String(raw || "").trim();
    if (!label) return;
    const inv = window.__TAURI__?.core?.invoke;
    let alreadyBound = false;
    if (inv) {
      try {
        const rep = await inv("validate_ignore_key", {
          label,
          alreadyInList: this._ignoreKeys.some((k) => k.toLowerCase() === label.toLowerCase()),
        });
        if (!rep?.ok) {
          const status = this._el("typingKeyReport");
          if (status) {
            const msg = rep?.already_bound
              ? this._t("settings_typing_ignore_already_bound", "Already ignored automatically (bound to a hotkey).")
              : rep?.reason === "already_ignored"
                ? this._t("settings_typing_ignore_protected", "Already ignored by default.")
                : rep?.reason === "duplicate"
                  ? this._t("settings_typing_ignore_duplicate", "Already in your list.")
                  : this._t("settings_typing_ignore_unknown", "Unknown key.");
            status.textContent = label + ": " + msg;
          }
          if (rep?.already_bound) {
            await this._refreshKeyPolicyReport();
            this._renderIgnoreKeys(await this._boundKeyLabels());
          }
          return;
        }
        alreadyBound = !!rep?.already_bound;
      } catch (_) {
        // If validation is unavailable, fall through and let the config round-trip decide.
      }
    }
    const normalized = alreadyBound ? label : label.charAt(0).toUpperCase() + label.slice(1);
    if (!this._ignoreKeys.some((k) => k.toLowerCase() === normalized.toLowerCase())) {
      this._ignoreKeys.push(normalized);
      this._ignoreKeys.sort();
      this._changed();
    }
    this._renderIgnoreKeys(await this._boundKeyLabels());
  },

  async removeIgnoredKey(key) {
    this._ignoreKeys = this._ignoreKeys.filter((k) => k !== key);
    this._changed();
    this._renderIgnoreKeys(await this._boundKeyLabels());
  },

  /** Labels currently exempt ONLY because they are bound to a hotkey. */
  async _boundKeyLabels() {
    const inv = window.__TAURI__?.core?.invoke;
    if (!inv) return [];
    try {
      const report = await inv("get_key_policy");
      return report?.non_seed_keys || [];
    } catch (_) {
      return [];
    }
  },

  /* ── list editing ──────────────────────────────────────────── */

  _addFromInput() {
    const input = this._el("appFilterEntry");
    const value = (input?.value || "").trim().toLowerCase();
    if (!value) return;
    if (input) input.value = "";
    this.addEntry(value);
  },

  addEntry(entry) {
    const normalized = String(entry || "").trim().toLowerCase();
    if (!normalized) return;
    if (this._list.includes(normalized)) {
      const status = this._el("captureAppStatus");
      if (status) {
        status.textContent = `${this._t("settings_app_filter_duplicate", "Already in the list:")} ${normalized}`;
      }
      return;
    }
    this._list.push(normalized);
    this._list.sort();
    this.render();
    this._changed();
  },

  removeEntry(index) {
    if (!Number.isInteger(index) || index < 0 || index >= this._list.length) return;
    this._list.splice(index, 1);
    this.render();
    this._changed();
  },

  /* ── rendering ─────────────────────────────────────────────── */

  render() {
    const list = this._el("appFilterList");
    if (!list) return;

    list.innerHTML = this._list.length === 0
      ? `<div style="color:var(--text-dim);font-size:12px;">${this._esc(this._t("settings_app_filter_empty", "No apps listed yet."))}</div>`
      : this._list.map((exe, idx) => `
        <div style="display:flex;align-items:center;gap:6px;font-size:12px;">
          <span style="flex:1;background:var(--bg-elev);border:1px solid var(--border);border-radius:5px;padding:4px 8px;font-family:monospace;">${this._esc(exe)}</span>
          <button class="app-filter-del preset-modal-btn cancel" data-idx="${idx}" style="flex:0 0 auto;padding:3px 8px;" title="${this._esc(this._t("settings_app_filter_remove", "Remove"))}">✕</button>
        </div>`).join("");
  },

  /* ── active-window capture ─────────────────────────────────── */

  /**
   * 3-second countdown, then ask Rust what the user focused. The user
   * never has to type a path or hunt the process list by hand.
   */
  startCapture() {
    if (this._capturing) return;
    this._capturing = true;

    const btn = this._el("captureAppBtn");
    if (btn) btn.disabled = true;

    let remaining = 3;
    const tick = () => {
      if (remaining > 0) {
        const status = this._el("captureAppStatus");
        if (status) {
          status.textContent = this._t(
            "settings_capture_countdown",
            "Switch to the target app… {n}",
            { n: remaining },
          );
        }
        remaining -= 1;
        this._captureTimer = setTimeout(tick, 1000);
        return;
      }
      this._finishCapture();
    };
    tick();
  },

  async _finishCapture() {
    const status = this._el("captureAppStatus");
    const btn = this._el("captureAppBtn");
    this._captureTimer = null;
    this._capturing = false;
    if (btn) btn.disabled = false;

    let exe = "";
    try {
      if (window.__TAURI__?.core?.invoke) {
        const res = await window.__TAURI__.core.invoke("capture_foreground_app");
        exe = String(res?.exe || "").trim().toLowerCase();
      }
    } catch (err) {
      console.warn("[SmartGuard] capture failed:", err);
    }

    if (!exe) {
      // Most common cause: nanoclick itself was still focused, and the
      // backend deliberately refuses to report its own process.
      if (status) {
        status.textContent = this._t(
          "settings_capture_failed",
          "Could not detect the active app — switch to it during the countdown.",
        );
      }
      return;
    }

    if (this._list.includes(exe)) {
      if (status) {
        status.textContent = `${this._t("settings_app_filter_duplicate", "Already in the list:")} ${exe}`;
      }
      return;
    }

    this.addEntry(exe);
    if (status) {
      status.textContent = `${this._t("settings_capture_captured", "Captured:")} ${exe}`;
    }
  },
};

if (typeof window !== "undefined") {
  window.SmartGuard = SmartGuard;
}
