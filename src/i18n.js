// ── nanoclick Lightweight i18n Engine ───────────────────────────
// Zero-dependency, minimal memory footprint internationalization module.
// Only keeps the active language dictionary in memory (lazy loading).

const I18nEngine = {
  currentLang: "ua",
  dict: null,
  _initialized: false,

  /**
   * Initializes i18n engine with the specified language.
   * @param {string} initialLang - "ua" or "en"
   */
  async init(initialLang = "ua") {
    const lang = ["ua", "en", "ru"].includes(initialLang) ? initialLang : "ua";
    await this.setLanguage(lang);
    this._initialized = true;
  },

  /**
   * Loads the language dictionary and applies it to the DOM.
   * Replaces any previous dictionary so old data is immediately garbage-collected.
   * @param {string} lang - "ua", "en", or "ru"
   */
  async setLanguage(lang) {
    const targetLang = ["ua", "en", "ru"].includes(lang) ? lang : "ua";
    try {
      const resp = await fetch(`locales/${targetLang}.json`);
      if (!resp.ok) {
        throw new Error(`HTTP error ${resp.status} fetching locales/${targetLang}.json`);
      }
      // Overwrite dictionary: previous dictionary has no references left and will be GC-ed
      this.dict = await resp.json();
      this.currentLang = targetLang;
      document.documentElement.lang = targetLang;
      this.applyTranslations();

      // Dispatch events so any custom JS components can react immediately
      const evt = { detail: { lang: targetLang } };
      window.dispatchEvent(new CustomEvent("nanoclick-language-changed", evt));
      document.dispatchEvent(new CustomEvent("languageChanged", evt));
    } catch (err) {
      console.warn(`[i18n] Failed to load locale "${targetLang}":`, err);
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
