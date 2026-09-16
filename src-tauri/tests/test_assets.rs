#[test]
fn test_index_html_modals_and_structure() {
    let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();
    let key = tauri::utils::assets::AssetKey::from("index.html");
    let bytes = ctx.assets().get(&key).expect("index.html not embedded");
    let html = String::from_utf8_lossy(&bytes);

    // 1. Preset edit modal has required id and scrollable body class
    assert!(html.contains("id=\"presetEditModal\""), "missing #presetEditModal");
    assert!(html.contains("preset-modal-body"), "missing .preset-modal-body class");
    assert!(html.contains("id=\"presetCancelBtn\""), "missing #presetCancelBtn");
    assert!(html.contains("id=\"presetSaveBtn\""), "missing #presetSaveBtn");

    // 2. Preset inspect modal has required id and buttons
    assert!(html.contains("id=\"presetInspectModal\""), "missing #presetInspectModal");
    assert!(html.contains("id=\"inspectCloseBtn\""), "missing #inspectCloseBtn");

    // 3. Onboarding modal has close button so user is never trapped
    assert!(html.contains("id=\"onboardingModal\""), "missing #onboardingModal");
    assert!(html.contains("id=\"onboardingCloseBtn\""), "missing #onboardingCloseBtn");

    // 4. Multi-point sequence has full-width class so it spans grid properly
    assert!(html.contains("preset-form-group full-width"), "multi-point sequence must be full-width");
    assert!(html.contains("id=\"prPointsCanvas\""), "missing #prPointsCanvas");

    // 5. Recording overlay has pointer-events:none so it doesn't intercept clicks
    assert!(html.contains("pointer-events:none"), "recordingOverlay must have pointer-events:none");
}

#[test]
fn test_style_css_modal_scrollability_and_overflow() {
    let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();
    let key = tauri::utils::assets::AssetKey::from("style.css");
    let bytes = ctx.assets().get(&key).expect("style.css not embedded");
    let css = String::from_utf8_lossy(&bytes);

    // 1. .preset-modal-body must have overflow-y: auto and max-height so tall forms scroll
    assert!(
        css.contains(".preset-modal-body"),
        "missing .preset-modal-body rule in style.css"
    );
    assert!(
        css.contains("max-height: 65vh") && css.contains("overflow-y: auto"),
        ".preset-modal-body must have max-height and overflow-y: auto"
    );

    // 2. .preset-modal-card must have max-height: 92vh and flexbox so card stays in viewport
    assert!(
        css.contains("max-height: 92vh"),
        ".preset-modal-card must have max-height: 92vh to prevent overflowing screen"
    );

    // 3. .modal-overlay must have overflow-y: auto as a universal safety net
    assert!(
        css.contains(".modal-overlay") && css.contains("overflow-y: auto"),
        ".modal-overlay must have overflow-y: auto"
    );

    // 4. .modal-body must have overflow-y: auto
    assert!(
        css.contains(".modal-body") && css.contains("overflow-y: auto"),
        ".modal-body must have overflow-y: auto"
    );

    // 5. .full-width must span both grid columns
    assert!(
        css.contains(".preset-form-group.full-width"),
        "missing .preset-form-group.full-width rule in style.css"
    );
}

#[test]
fn test_main_js_escape_and_backdrop_handlers() {
    let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();
    let key = tauri::utils::assets::AssetKey::from("main.js");
    let bytes = ctx.assets().get(&key).expect("main.js not embedded");
    let js = String::from_utf8_lossy(&bytes);

    // 1. Escape key handler must check Escape and hide modals
    assert!(js.contains("e.key !== \"Escape\""), "missing Escape key handler check");
    assert!(js.contains("presetEditModal"), "Escape key handler must handle presetEditModal");
    assert!(js.contains("presetInspectModal"), "Escape key handler must handle presetInspectModal");
    assert!(js.contains("onboardingModal"), "Escape key handler must handle onboardingModal");

    // 2. Backdrop click handler must bind to modals
    assert!(js.contains("bindBackdropClose"), "missing bindBackdropClose helper");
    assert!(js.contains("bindBackdropClose(\"presetEditModal\")"), "presetEditModal must have backdrop close");
    assert!(js.contains("bindBackdropClose(\"presetInspectModal\")"), "presetInspectModal must have backdrop close");

    // 3. Visual editor must also have Escape handler and backdrop click
    assert!(js.contains("onVeKeydown"), "visual editor must have Escape key listener");
    assert!(js.contains("visualEditorBackdrop"), "visual editor backdrop must be dismissible");
}

#[test]
fn test_stats_js_and_immediate_updates() {
    let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();

    // 1. stats.js must be present in embedded assets
    let stats_key = tauri::utils::assets::AssetKey::from("stats.js");
    let stats_bytes = ctx.assets().get(&stats_key).expect("stats.js must be embedded");
    let stats_code = String::from_utf8_lossy(&stats_bytes);
    assert!(stats_code.contains("StatsEngine"), "stats.js must export StatsEngine");
    assert!(stats_code.contains("recordSessionTick"), "stats.js must have recordSessionTick");
    assert!(stats_code.contains("drawStatsChart"), "stats.js must have drawStatsChart");
    assert!(stats_code.contains("saveToLocalStorage"), "stats.js must have localStorage backup");

    // 2. index.html must load stats.js
    let html_key = tauri::utils::assets::AssetKey::from("index.html");
    let html_bytes = ctx.assets().get(&html_key).expect("index.html not embedded");
    let html = String::from_utf8_lossy(&html_bytes);
    assert!(html.contains("<script src=\"stats.js\"></script>"), "index.html must include stats.js");

    // 3. main.js must update mode display immediately without restart
    let js_key = tauri::utils::assets::AssetKey::from("main.js");
    let js_bytes = ctx.assets().get(&js_key).expect("main.js not embedded");
    let js = String::from_utf8_lossy(&js_bytes);
    assert!(
        js.contains("setModeDisplay(currentConfig.active_mode"),
        "main.js must call setModeDisplay immediately on mode_switch update"
    );
    assert!(
        js.contains("StatsEngine.recordSessionTick"),
        "main.js must delegate stats recording to StatsEngine"
    );
    assert!(
        js.contains("key_ttl_ms"),
        "main.js must track and save key_ttl_ms"
    );
}

#[test]
fn test_uipi_manager_and_elevation_assets() {
    let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();

    // 1. uipi_manager.js must be present in embedded assets
    let uipi_key = tauri::utils::assets::AssetKey::from("uipi_manager.js");
    let uipi_bytes = ctx.assets().get(&uipi_key).expect("uipi_manager.js must be embedded");
    let uipi_code = String::from_utf8_lossy(&uipi_bytes);
    assert!(uipi_code.contains("UipiManager"), "uipi_manager.js must export UipiManager");
    assert!(uipi_code.contains("check_elevation"), "uipi_manager.js must invoke check_elevation");
    assert!(uipi_code.contains("restart_as_admin"), "uipi_manager.js must invoke restart_as_admin");
    assert!(uipi_code.contains("set_always_run_as_admin"), "uipi_manager.js must invoke set_always_run_as_admin");

    // 2. index.html must load uipi_manager.js and have UI elements
    let html_key = tauri::utils::assets::AssetKey::from("index.html");
    let html_bytes = ctx.assets().get(&html_key).expect("index.html not embedded");
    let html = String::from_utf8_lossy(&html_bytes);
    assert!(html.contains("<script src=\"uipi_manager.js\"></script>"), "index.html must include uipi_manager.js");
    assert!(html.contains("id=\"adminStatusBadge\""), "index.html must have #adminStatusBadge");
    assert!(html.contains("id=\"alwaysRunAsAdminCheckbox\""), "index.html must have #alwaysRunAsAdminCheckbox");
    assert!(html.contains("id=\"uipiModal\""), "index.html must have #uipiModal");
    assert!(html.contains("id=\"uipiRestartAdminBtn\""), "index.html must have #uipiRestartAdminBtn");

    // 3. style.css must style UIPI components
    let css_key = tauri::utils::assets::AssetKey::from("style.css");
    let css_bytes = ctx.assets().get(&css_key).expect("style.css not embedded");
    let css = String::from_utf8_lossy(&css_bytes);
    assert!(css.contains(".admin-status-badge"), "style.css must have .admin-status-badge");
    assert!(css.contains(".uipi-modal-card"), "style.css must have .uipi-modal-card");
}

#[test]
fn test_i18n_assets_and_dictionary_keys() {
    let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();

    // 1. i18n.js must be embedded and export I18nEngine
    let i18n_key = tauri::utils::assets::AssetKey::from("i18n.js");
    let i18n_bytes = ctx.assets().get(&i18n_key).expect("i18n.js must be embedded");
    let i18n_code = String::from_utf8_lossy(&i18n_bytes);
    assert!(i18n_code.contains("I18nEngine"), "i18n.js must export I18nEngine");
    assert!(i18n_code.contains("applyTranslations"), "i18n.js must have applyTranslations");
    assert!(i18n_code.contains("setLanguage"), "i18n.js must have setLanguage");

    // 2. ua.json, en.json, and ru.json must be embedded
    let ua_key = tauri::utils::assets::AssetKey::from("locales/ua.json");
    let ua_bytes = ctx.assets().get(&ua_key).expect("locales/ua.json must be embedded");
    let ua_val: serde_json::Value = serde_json::from_slice(&ua_bytes).expect("ua.json must be valid JSON");

    let en_key = tauri::utils::assets::AssetKey::from("locales/en.json");
    let en_bytes = ctx.assets().get(&en_key).expect("locales/en.json must be embedded");
    let en_val: serde_json::Value = serde_json::from_slice(&en_bytes).expect("en.json must be valid JSON");

    let ru_key = tauri::utils::assets::AssetKey::from("locales/ru.json");
    let ru_bytes = ctx.assets().get(&ru_key).expect("locales/ru.json must be embedded");
    let ru_val: serde_json::Value = serde_json::from_slice(&ru_bytes).expect("ru.json must be valid JSON");

    let ua_map = ua_val.as_object().expect("ua.json must be an object");
    let en_map = en_val.as_object().expect("en.json must be an object");
    let ru_map = ru_val.as_object().expect("ru.json must be an object");

    // 3. Perfect dictionary key symmetry across all three languages (UA, EN, RU)
    for k in ua_map.keys() {
        assert!(en_map.contains_key(k), "en.json is missing key '{}' present in ua.json", k);
        assert!(ru_map.contains_key(k), "ru.json is missing key '{}' present in ua.json", k);
    }
    for k in en_map.keys() {
        assert!(ua_map.contains_key(k), "ua.json is missing key '{}' present in en.json", k);
        assert!(ru_map.contains_key(k), "ru.json is missing key '{}' present in en.json", k);
    }
    for k in ru_map.keys() {
        assert!(ua_map.contains_key(k), "ua.json is missing key '{}' present in ru.json", k);
        assert!(en_map.contains_key(k), "en.json is missing key '{}' present in ru.json", k);
    }

    // 4. Dictionary volume must exceed 150 keys (comprehensive coverage)
    assert!(ua_map.len() >= 150, "Dictionaries must contain at least 150 keys, got {}", ua_map.len());

    // 5. index.html must load i18n.js and include data-i18n across all sections
    let html_key = tauri::utils::assets::AssetKey::from("index.html");
    let html_bytes = ctx.assets().get(&html_key).expect("index.html not embedded");
    let html = String::from_utf8_lossy(&html_bytes);
    assert!(html.contains("<script src=\"i18n.js\"></script>"), "index.html must include i18n.js");
    assert!(html.contains("id=\"languageSelect\""), "index.html must have #languageSelect");
    assert!(html.contains("data-i18n=\"settings_lang_label\""), "index.html must have data-i18n for language label");
    assert!(html.contains("data-i18n=\"dash_click_rate\""), "index.html must have dash_click_rate");
    assert!(html.contains("data-i18n=\"presets_header_title\""), "index.html must have presets_header_title");
    assert!(html.contains("data-i18n=\"automation_header\""), "index.html must have automation_header");
    assert!(html.contains("data-i18n=\"hotkeys_header\""), "index.html must have hotkeys_header");
    assert!(html.contains("data-i18n=\"stats_header\""), "index.html must have stats_header");
    assert!(html.contains("data-i18n=\"about_header\""), "index.html must have about_header");
    assert!(html.contains("data-i18n=\"uipi_modal_title\""), "index.html must have uipi_modal_title");
}

#[test]
fn test_overlay_assets_and_capabilities() {
    let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();

    // 1. overlay.html must be present and load overlay.css & overlay.js
    let html_key = tauri::utils::assets::AssetKey::from("overlay.html");
    let html_bytes = ctx.assets().get(&html_key).expect("overlay.html must be embedded");
    let html = String::from_utf8_lossy(&html_bytes);
    assert!(html.contains("overlay.css"), "overlay.html must link overlay.css");
    assert!(html.contains("overlay.js"), "overlay.html must link overlay.js");
    assert!(html.contains("rippleContainer"), "overlay.html must have #rippleContainer");

    // 2. overlay.css must define transparent background and ripple animation
    let css_key = tauri::utils::assets::AssetKey::from("overlay.css");
    let css_bytes = ctx.assets().get(&css_key).expect("overlay.css must be embedded");
    let css = String::from_utf8_lossy(&css_bytes);
    assert!(css.contains("background: transparent !important"), "overlay.css must enforce transparency");
    assert!(css.contains("pointer-events: none"), "overlay.css must be pointer-events: none");
    assert!(css.contains("ripple-expand"), "overlay.css must have ripple-expand animation");

    // 3. overlay.js must handle DPI scaling and overlay_ready
    let js_key = tauri::utils::assets::AssetKey::from("overlay.js");
    let js_bytes = ctx.assets().get(&js_key).expect("overlay.js must be embedded");
    let js = String::from_utf8_lossy(&js_bytes);
    assert!(js.contains("spawn-ripple"), "overlay.js must listen for spawn-ripple event");
    assert!(js.contains("devicePixelRatio"), "overlay.js must correct for DPI scaling");
    assert!(js.contains("overlay_ready"), "overlay.js must invoke overlay_ready signal");

    // 4. main.js must invoke toggle_overlay and must not have legacy mousemove listener
    let main_js_key = tauri::utils::assets::AssetKey::from("main.js");
    let main_js_bytes = ctx.assets().get(&main_js_key).expect("main.js must be embedded");
    let main_js = String::from_utf8_lossy(&main_js_bytes);
    assert!(main_js.contains("toggle_overlay"), "main.js must invoke toggle_overlay");
    assert!(!main_js.contains("document.addEventListener(\"mousemove\""), "main.js must not contain obsolete mousemove listener");

    // 5. index.html must no longer have embedded rippleContainer
    let index_key = tauri::utils::assets::AssetKey::from("index.html");
    let index_bytes = ctx.assets().get(&index_key).expect("index.html must be embedded");
    let index_html = String::from_utf8_lossy(&index_bytes);
    assert!(!index_html.contains("<div id=\"rippleContainer\""), "index.html must not contain legacy rippleContainer");

    // 6. tauri.conf.json & capabilities must declare overlay window
    let tauri_conf = include_str!("../tauri.conf.json");
    assert!(tauri_conf.contains("\"label\": \"overlay\""), "tauri.conf.json must declare overlay window");
    assert!(tauri_conf.contains("\"url\": \"overlay.html\""), "tauri.conf.json must point overlay to overlay.html");
    assert!(tauri_conf.contains("\"fullscreen\": true"), "tauri.conf.json must set overlay fullscreen");

    let cap_default = include_str!("../capabilities/default.json");
    assert!(cap_default.contains("\"overlay\""), "capabilities/default.json must include overlay window");
}

