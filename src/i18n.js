// ── nanoclick Lightweight i18n Engine ───────────────────────────
// Zero-dependency, minimal memory footprint internationalization module.
// Only keeps the active language dictionary in memory (lazy loading).

const I18nEngine = {
  currentLang: "ua",
  dict: null,
  _initialized: false,
  // One in-flight fetch per language: rapid EN↔UA clicks share the pending
  // promise instead of stacking duplicate downloads (the JS "memory leak").
  _pending: {},
  // Loaded dictionaries stay cached: a language visited once never
  // re-downloads (~12 KB saved per revisit, zero stale-promises pileup).
  _cache: {},

  /**
   * Initializes i18n engine with the specified language.
   * @param {string} initialLang - "ua" or "en"
   */
  async init(initialLang = "ua") {
    const lang = ["ua", "en"].includes(initialLang) ? initialLang : "ua";
    await this.setLanguage(lang);
    this._initialized = true;
  },

  /**
   * Loads the language dictionary and applies it to the DOM.
   * Replaces any previous dictionary so old data is immediately garbage-collected.
   * @param {string} lang - "ua" or "en"
   */
  async setLanguage(lang) {
    const targetLang = ["ua", "en"].includes(lang) ? lang : "ua";
    // Same language, dictionary already live: repaint is a no-op, skip fetch.
    // (The segment paint is owned by main.js paintLanguageSegment.)
    if (targetLang === this.currentLang && this.dict) return;
    // A fetch for this language is already in flight: piggyback on it
    // instead of firing a duplicate download.
    if (this._pending[targetLang]) {
      try { await this._pending[targetLang]; } catch (_) { /* reported below */ }
      if (this.currentLang === targetLang && this.dict) return;
    }
    // Smooth swap: fade the settings view out, swap strings, fade back in.
    // One rAF-driven class — no timers to leak, no layout thrash.
    const view = typeof document !== "undefined"
      ? document.getElementById("viewSettings")
      : null;
    if (view) view.classList.add("lang-swapping");
    const job = (async () => {
      // Cache hit: no network at all.
      if (this._cache[targetLang]) {
        this.dict = this._cache[targetLang];
      } else {
        const resp = await fetch(`locales/${targetLang}.json`);
        if (!resp.ok) {
          throw new Error(`HTTP error ${resp.status} fetching locales/${targetLang}.json`);
        }
        // Cache BEFORE publish: concurrent waiters read the same object.
        this._cache[targetLang] = await resp.json();
        this.dict = this._cache[targetLang];
      }
      // Overwrite dictionary: the previous dict object stays cached under its
      // own key (revisit = free), nothing dangles for the GC to chase.
      this.currentLang = targetLang;
      document.documentElement.lang = targetLang;
      this.applyTranslations();

      // Dispatch events so any custom JS components can react immediately
      const evt = { detail: { lang: targetLang } };
      window.dispatchEvent(new CustomEvent("nanoclick-language-changed", evt));
      document.dispatchEvent(new CustomEvent("languageChanged", evt));
    })();
    this._pending[targetLang] = job;
    try {
      await job;
    } catch (err) {
      console.warn(`[i18n] Failed to load locale "${targetLang}":`, err);
    } finally {
      delete this._pending[targetLang];
      // Paint the swap on the next frame so the fade-out is actually visible,
      // then release the class — a second rapid switch re-arms cleanly.
      if (view) {
        const release = () => view.classList.remove("lang-swapping");
        if (typeof requestAnimationFrame !== "undefined") {
          requestAnimationFrame(() => requestAnimationFrame(release));
        } else {
          release();
        }
      }
    }
  },

  /**
   * Translates a key with optional dynamic parameter interpolation.
   * Example: t("clicker_cps_hint", { cps: 50 }, "Fallback")
   * @param {string} key
   * @param {Object} params
   * @param {string} fallback
   * @returns {string}
   */
  t(key, params = {}, fallback = "") {
    let text = (this.dict && this.dict[key]) ? this.dict[key] : (fallback || key);
    if (params && typeof params === "object") {
      for (const [k, v] of Object.entries(params)) {
        text = text.replace(new RegExp(`\\{${k}\\}`, "g"), String(v));
      }
    }
    return text;
  },

  /**
   * Scans the document and applies translations to all marked elements.
   * - [data-i18n]: textContent
   * - [data-i18n-html]: innerHTML (for elements with tags like <strong>, <code>)
   * - [data-i18n-title]: title attribute (tooltips)
   * - [data-i18n-placeholder]: placeholder attribute (inputs)
   */
  applyTranslations(container = document) {
    if (!this.dict) return;

    // 1. Text content
    const textEls = container.querySelectorAll("[data-i18n]");
    for (let i = 0; i < textEls.length; i++) {
      const el = textEls[i];
      const key = el.getAttribute("data-i18n");
      if (this.dict[key]) {
        el.textContent = this.dict[key];
      }
    }

    // 2. HTML content (for rich formatted blocks)
    const htmlEls = container.querySelectorAll("[data-i18n-html]");
    for (let i = 0; i < htmlEls.length; i++) {
      const el = htmlEls[i];
      const key = el.getAttribute("data-i18n-html");
      if (this.dict[key]) {
        el.innerHTML = this.dict[key];
      }
    }

    // 3. Tooltip / Title attributes
    const titleEls = container.querySelectorAll("[data-i18n-title]");
    for (let i = 0; i < titleEls.length; i++) {
      const el = titleEls[i];
      const key = el.getAttribute("data-i18n-title");
      if (this.dict[key]) {
        el.title = this.dict[key];
      }
    }

    // 4. Placeholder attributes
    const placeholderEls = container.querySelectorAll("[data-i18n-placeholder]");
    for (let i = 0; i < placeholderEls.length; i++) {
      const el = placeholderEls[i];
      const key = el.getAttribute("data-i18n-placeholder");
      if (this.dict[key]) {
        el.placeholder = this.dict[key];
      }
    }
  }
};

window.I18nEngine = I18nEngine;
