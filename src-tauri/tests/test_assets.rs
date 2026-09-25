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
    // Session lifecycle invariants: exactly-once finalize, junk filter, dirty flag.
    // Finalize runs ONLY on the true active->idle transition (early return on
    // !activeNow swallows late 66ms worker echoes); belt-and-suspenders
    // monotonic guard survives even a missed flag reset.
    assert!(stats_code.contains("if (!this.state.activeNow)"), "stats.js finalize must no-op on late idle echoes");
    assert!(stats_code.contains("lastFinalizedClicks"), "stats.js must guard double-flush with lastFinalizedClicks");
    assert!(stats_code.contains("isJunk"), "stats.js must filter junk runs from history");
    assert!(stats_code.contains("statsDirty"), "stats.js must gate disk writes with a dirty flag");
    assert!(stats_code.contains("st.history.length > 50"), "stats.js history must stay ring-capped at 50");

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
    assert!(html.contains("rippleCanvas"), "overlay.html must have #rippleCanvas");

    // 2. overlay.css must define transparent background and canvas layout
    // (ripple is a Canvas 2D renderer — no DOM keyframes anymore)
    let css_key = tauri::utils::assets::AssetKey::from("overlay.css");
    let css_bytes = ctx.assets().get(&css_key).expect("overlay.css must be embedded");
    let css = String::from_utf8_lossy(&css_bytes);
    assert!(css.contains("background: transparent"), "overlay.css must enforce transparency");
    assert!(css.contains("pointer-events: none"), "overlay.css must be pointer-events: none");
    assert!(css.contains("rippleCanvas"), "overlay.css must style #rippleCanvas");

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

    // 6. tauri.conf.json must declare ONLY the main window at boot (lazy overlay/hud)
    let tauri_conf = include_str!("../tauri.conf.json");
    assert!(tauri_conf.contains("NanoClick"), "tauri.conf.json must declare main window");
    assert!(tauri_conf.contains("\"windows\""), "tauri.conf.json must declare windows");
    assert!(!tauri_conf.contains("overlay.html"), "tauri.conf.json must NOT pre-create overlay (lazy via ensure_overlay_window)");
    assert!(!tauri_conf.contains("hud.html"), "tauri.conf.json must NOT pre-create hud (lazy via ensure_hud_window)");

    let cap_default = include_str!("../capabilities/default.json");
    assert!(cap_default.contains("\"overlay\""), "capabilities/default.json must include overlay window");
    assert!(cap_default.contains("\"hud\""), "capabilities/default.json must include hud window");
}

/// Lazy-creation wiring: secondary WebViews must be built at runtime, never
/// pre-declared in tauri.conf.json. Cold boot = main window only.
#[test]
fn test_lazy_windows_built_at_runtime() {
    let overlay_src = include_str!("../src/overlay.rs");
    assert!(overlay_src.contains("ensure_overlay_window"), "overlay.rs must expose ensure_overlay_window");
    assert!(overlay_src.contains("WebviewWindowBuilder::new"), "overlay.rs must build overlay via WebviewWindowBuilder");
    assert!(overlay_src.contains("overlay.html"), "overlay.rs must load overlay.html at runtime");
    assert!(overlay_src.contains("win.destroy()") || overlay_src.contains("win.destroy"), "toggle_overlay(false) must destroy the WebView to free RAM");

    let lib_src = include_str!("../src/lib.rs");
    assert!(lib_src.contains("ensure_hud_window"), "lib.rs must expose ensure_hud_window");
    assert!(lib_src.contains("hud.html"), "lib.rs must load hud.html at runtime");
    assert!(lib_src.contains("no WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"), "lib.rs must document why no extra browser flags are set");

    let sched_src = include_str!("../src/scheduler.rs");
    assert!(sched_src.contains("ensure_overlay_window"), "scheduler must pre-create overlay on click-start (background thread)");
}

/// Last-Known-Good wiring: golden snapshots + quarantine + notice queues.
#[test]
fn test_last_good_black_box_wiring() {
    let defaults_src = include_str!("../src/defaults/mod.rs");
    assert!(defaults_src.contains("config.last_good.json"), "must define LKG snapshot file name");
    assert!(defaults_src.contains("refresh_last_good_snapshot"), "save path must refresh snapshot");
    assert!(defaults_src.contains("load_last_good_snapshot"), "must restore LKG on corruption");
    assert!(defaults_src.contains(".broken-"), "must quarantine broken file with timestamp");
    assert!(defaults_src.contains("restored_from_last_good"), "must report LKG vs factory source");

    let lib_src = include_str!("../src/lib.rs");
    assert!(lib_src.contains("get_startup_notices"), "lib.rs must expose queue drain command");
    assert!(lib_src.contains("poll_file_toasts"), "lib.rs must expose toast drain command");
    assert!(lib_src.contains("startup_notices"), "AppState must carry notice queue");
    assert!(lib_src.contains("file_toasts"), "AppState must carry toast queue");
    assert!(lib_src.contains("WatcherState"), "save commands must mark own writes");
    assert!(lib_src.contains("load_with_action"), "boot must capture healing action");
    assert!(lib_src.contains("NoticeLevel"), "must define granular notice levels");
    assert!(lib_src.contains("AppNotice"), "must define AppNotice payload");
    assert!(lib_src.contains("Critical"), "critical level must exist");
    assert!(lib_src.contains("macros_healing_notice"), "macros healing must feed the queue");
    assert!(lib_src.contains("file_health_notice"), "watcher verdicts must feed toasts");

    let cm_src = include_str!("../src/config_manager.rs");
    assert!(cm_src.contains("load_with_action"), "ConfigManager must expose load_with_action");

    let main_js = {
        let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();
        let key = tauri::utils::assets::AssetKey::from("main.js");
        let bytes = ctx.assets().get(&key).expect("main.js must be embedded");
        String::from_utf8_lossy(&bytes).into_owned()
    };
    assert!(main_js.contains("get_startup_notices"), "main.js must drain notice queue");
    assert!(main_js.contains("poll_file_toasts"), "main.js must poll toast queue");
    assert!(main_js.contains("showDeadboltQueue"), "main.js must chain queue in order");
    assert!(main_js.contains("deadboltModal"), "main.js must build deadbolt modal");
    assert!(main_js.contains("deadboltOk"), "modal must have mandatory OK button");
    assert!(main_js.contains("resolveNoticeText"), "notices must resolve i18n keys");
    assert!(main_js.contains("showFileToast"), "watcher verdicts must toast non-blocking");
}

/// Remember-window-position wiring: BEHAVIOR checkbox -> config fields ->
/// save capture -> boot restore with virtual-screen sanitizer.
#[test]
fn test_remember_window_position_wiring() {
    let cm_src = include_str!("../src/config_manager.rs");
    assert!(cm_src.contains("remember_window_position"), "UiSettings must carry remember flag");
    assert!(cm_src.contains("window_x"), "UiSettings must carry window_x");
    assert!(cm_src.contains("window_y"), "UiSettings must carry window_y");

    let lib_src = include_str!("../src/lib.rs");
    assert!(lib_src.contains("outer_position"), "save_app_config must capture outer_position");
    assert!(lib_src.contains("remember_window_position"), "save/restore must respect the flag");
    assert!(lib_src.contains("get_screen_size"), "boot restore must sanitize against virtual screen");
    assert!(lib_src.contains("set_position"), "boot restore must set_position when inside screen");
    assert!(lib_src.contains("win.center()") || lib_src.contains(".center()"), "off-screen save must fall back to center");

    let defaults_src = include_str!("../src/defaults/mod.rs");
    assert!(defaults_src.contains("remember_window_position"), "repair must preserve the flag");
    assert!(defaults_src.contains("window_x"), "repair must handle window_x");

    let html = {
        let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();
        let key = tauri::utils::assets::AssetKey::from("index.html");
        let bytes = ctx.assets().get(&key).expect("index.html must be embedded");
        String::from_utf8_lossy(&bytes).into_owned()
    };
    assert!(html.contains("rememberPosCheckbox"), "BEHAVIOR card must have the checkbox");
    assert!(html.contains("settings_remember_pos"), "checkbox must use i18n key");

    let main_js = {
        let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();
        let key = tauri::utils::assets::AssetKey::from("main.js");
        let bytes = ctx.assets().get(&key).expect("main.js must be embedded");
        String::from_utf8_lossy(&bytes).into_owned()
    };
    assert!(main_js.contains("rememberPosCheckbox"), "main.js must hydrate/collect the checkbox");
    assert!(main_js.contains("remember_window_position"), "main.js must sync the config field");
}

/// Focus-loss auto-pause wiring: BEHAVIOR checkbox -> UiSettings/Config field
/// -> FocusGuard state machine -> click-loop choke -> UI pause toast.
#[test]
fn test_focus_loss_guard_wiring() {
    let cm_src = include_str!("../src/config_manager.rs");
    assert!(cm_src.contains("pause_on_focus_loss"), "UiSettings must carry pause_on_focus_loss (default false)");

    let cfg_src = include_str!("../src/config.rs");
    assert!(cfg_src.contains("pause_on_focus_loss"), "runtime Config must map pause_on_focus_loss");

    let sched_src = include_str!("../src/scheduler.rs");
    assert!(sched_src.contains("pub struct FocusGuard"), "FocusGuard state machine must exist");
    assert!(sched_src.contains("focus_guard_arc.arm("), "run start must arm the guard with the session exe");
    assert!(sched_src.contains("focus_guard_arc.should_pause("), "click loop must poll the guard");
    assert!(sched_src.contains("focus_guard_arc.disarm();"), "run end must disarm the session");
    assert!(sched_src.contains("focus-loss-paused"), "pause must notify the UI");
    // Own-window contract: returning to NanoClick never pauses, leaving it
    // (even via Alt+Tab from our own window) stops. A plain config save
    // must not wipe a live session (stats flush every ~5 s while clicking).
    assert!(sched_src.contains("is_own_exe"), "guard must exempt our own window");
    assert!(sched_src.contains("own_exe_name"), "own exe must resolve from current_exe");

    let plat_src = include_str!("../src/platform/mod.rs");
    assert!(plat_src.contains("pub fn own_exe_name"), "platform must expose own_exe_name");

    let html = {
        let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();
        let key = tauri::utils::assets::AssetKey::from("index.html");
        let bytes = ctx.assets().get(&key).expect("index.html must be embedded");
        String::from_utf8_lossy(&bytes).into_owned()
    };
    assert!(html.contains("pauseFocusLossCheckbox"), "BEHAVIOR card must have the focus-loss checkbox");
    assert!(html.contains("settings_pause_focus_loss"), "checkbox must use i18n key");

    let main_js = {
        let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();
        let key = tauri::utils::assets::AssetKey::from("main.js");
        let bytes = ctx.assets().get(&key).expect("main.js must be embedded");
        String::from_utf8_lossy(&bytes).into_owned()
    };
    assert!(main_js.contains("pause_on_focus_loss"), "main.js must hydrate/collect the flag");
    assert!(main_js.contains("focus-loss-paused"), "main.js must toast the auto-pause reason");

    // Typing-ms persistence: spinner `input` ticks (not just `change` on
    // blur/Enter) must reach the throttled save pipeline, or the value is
    // lost when the user hits Start right after the spinner.
    let guard_js = {
        let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();
        let key = tauri::utils::assets::AssetKey::from("settings_guard.js");
        let bytes = ctx.assets().get(&key).expect("settings_guard.js must be embedded");
        String::from_utf8_lossy(&bytes).into_owned()
    };
    assert!(guard_js.contains("typingFreezeInput"), "guard must wire the ms input");
    assert!(guard_js.contains("typing_pause_ms"), "guard must sync the ms field");
    assert!(guard_js.contains("\"input\""), "spinner ticks must save without blur");

    // Instant-stop watcher: a ~50 ms thread (direct lookup, no TTL cache)
    // wakes the sleeping click loop via the stop event; the emit stays
    // exactly-once (watcher vs loop claim).
    let guard_src = include_str!("../src/guard/mod.rs");
    assert!(guard_src.contains("FOCUS_WATCH_MS"), "watch cadence must be named");
    let watch_src = include_str!("../src/guard/focus_watch.rs");
    assert!(watch_src.contains("pub fn spawn_focus_watcher"), "watcher must exist");
    assert!(watch_src.contains("pub struct FocusWatchStop"), "stop signal must exist");
    assert!(sched_src.contains("spawn_focus_watcher"), "run must spawn watcher");
    assert!(sched_src.contains("focus_watch_stop"), "loop must drain watcher");
}

/// Background-memory guard wiring (v1.1.0).
///
/// Locks in three lessons:
/// 1. the WebView2 policy must be applied for EVERY webview (env var, not the
///    conf entry — HUD/overlay are built at runtime);
/// 2. it must never use the flags the Zero-Jitter Mandate forbids;
/// 3. it must never re-introduce a V8 heap cap (`--js-flags`) or a single
///    renderer — both were measured to change nothing (346 MB → 345 MB) while
///    risking an OOM inside the renderer, which shows up to the user as
///    "the interface does nothing".
///
/// The UI side must suspend its periodic work while the window is hidden, and
/// boot progress must be visible in the log even with debug switched off.
#[test]
fn test_background_memory_guard_wiring() {
    let main_rs = include_str!("../src/main.rs");

    assert!(
        main_rs.contains("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"),
        "the WebView2 memory policy must be applied at boot (env var covers HUD+overlay too)"
    );
    assert!(
        main_rs.contains("--disable-background-networking"),
        "background network services are pure overhead for a clicker"
    );

    // Policy string only: the doc comment above it names the banned flags on
    // purpose, to explain why they are banned.
    let policy_start = main_rs
        .find("const POLICY: &str = \"")
        .expect("the webview policy constant must exist");
    let policy_end = main_rs[policy_start..]
        .find("\";")
        .map(|off| policy_start + off)
        .expect("the policy string must be terminated");
    let policy = &main_rs[policy_start..policy_end];

    for forbidden in ["--single-process", "--in-process-gpu", "--no-sandbox"] {
        assert!(
            !policy.contains(forbidden),
            "{forbidden} violates the Zero-Jitter Mandate (GPU/renderer must stay out of process)"
        );
    }
    assert!(
        !policy.contains("--js-flags"),
        "a V8 heap cap risks a renderer OOM (\"interface does nothing\") for zero measured gain"
    );
    assert!(
        !policy.contains("--renderer-process-limit"),
        "a single renderer couples HUD/overlay to the main window for zero measured gain"
    );

    let main_js = {
        let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();
        let key = tauri::utils::assets::AssetKey::from("main.js");
        let bytes = ctx.assets().get(&key).expect("main.js must be embedded");
        String::from_utf8_lossy(&bytes).into_owned()
    };

    assert!(
        main_js.contains("function bootCanary"),
        "boot progress must be reportable without the debug flag"
    );
    assert!(
        main_js.contains("step FAILED: ${name}"),
        "a failed init step must always reach the log"
    );
    assert!(
        main_js.contains("function everyVisible"),
        "the visibility gate helper must exist"
    );
    assert!(
        main_js.contains("everyVisible(1000, renderStats)"),
        "the stats render must be gated by visibility"
    );
    assert!(
        !main_js.contains("setInterval(renderStats, 1000)"),
        "an ungated 1 s stats interval keeps a hidden renderer awake"
    );
    assert!(
        main_js.contains("everyVisible(3000, tick)"),
        "the toast polling must be gated by visibility too"
    );
}

/// `main.js` is loaded as an ES module (`<script type="module">`).
///
/// In a module a **duplicate top-level declaration is a fatal SyntaxError**:
/// the module never executes at all, so the entire UI dies silently — the
/// window still renders, but no handler is ever attached, no IPC call is ever
/// made and the hotkey UI is dead. The very same duplicate is perfectly legal
/// in a classic script, which is why `node --check main.js` (script mode) and
/// every existing test passed while the app was completely unusable.
///
/// This test is that missing check. It only looks at column-zero declarations,
/// so indented (function-local) code cannot produce a false positive.
#[test]
fn test_main_js_has_no_duplicate_top_level_declarations() {
    let js = {
        let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();
        let key = tauri::utils::assets::AssetKey::from("main.js");
        let bytes = ctx.assets().get(&key).expect("main.js must be embedded");
        String::from_utf8_lossy(&bytes).into_owned()
    };

    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for line in js.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            continue; // nested: cannot collide with a top-level binding
        }
        let rest = line
            .strip_prefix("async function ")
            .or_else(|| line.strip_prefix("function "))
            .or_else(|| line.strip_prefix("const "))
            .or_else(|| line.strip_prefix("let "))
            .or_else(|| line.strip_prefix("var "))
            .or_else(|| line.strip_prefix("class "));
        let Some(rest) = rest else { continue };
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
            .collect();
        if name.is_empty() {
            continue;
        }
        *counts.entry(name).or_insert(0) += 1;
    }

    let mut dupes: Vec<String> = counts
        .iter()
        .filter(|(_, n)| **n > 1)
        .map(|(k, n)| format!("{k} x{n}"))
        .collect();
    dupes.sort();
    assert!(
        dupes.is_empty(),
        "duplicate top-level declarations are a fatal ES-module SyntaxError and kill the whole UI: {dupes:?}"
    );
}

/// MANDATORY JavaScript grammar check (v1.1.0).
///
/// This is the check that was missing while a duplicate top-level declaration
/// in `main.js` kept the entire UI dead: the file is loaded as an ES module,
/// and in module mode a duplicate declaration is a fatal SyntaxError. It is
/// perfectly legal in a classic script, which is exactly why `node --check
/// main.js` (script mode) and the whole test suite stayed green on a completely
/// unusable build.
///
/// Strategy: extract every `<script src="…">` from the embedded HTML, then
/// syntax-check each file with Node using the SAME parse mode the browser will
/// use — `.mjs` for `type="module"` scripts, `.js` for classic ones. This is
/// V8's own parser, so it catches everything the renderer would reject:
/// duplicate declarations, reserved words, bad regex, unbalanced braces.
///
/// Node is mandatory. There is no GitHub Actions workflow in this repository
/// (`.github/` holds only images), so the enforcement points are: this test,
/// `scripts/check-js-syntax.ps1` for a manual run, and Stage 0 (PREFLIGHT) of
/// `scripts/release.ps1`, which runs both before `cargo tauri build` — because
/// `cargo tauri build` itself never runs tests. For a machine without Node, set
/// `NANOCLICK_JS_SYNTAX_STRICT=0` to downgrade to a warning (never for a release).
#[test]
fn test_frontend_js_syntax_is_valid() {
    let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();
    let read_asset = |name: &str| -> Option<String> {
        let key = tauri::utils::assets::AssetKey::from(name);
        ctx.assets()
            .get(&key)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    };

    // ── 1. Discover the scripts and their parse mode from the HTML ──────
    let mut scripts: Vec<(String, bool)> = Vec::new(); // (file, is_module)
    for page in ["index.html", "hud.html", "overlay.html"] {
        let Some(html) = read_asset(page) else { continue };
        for tag in html.split("<script").skip(1) {
            let Some(end) = tag.find('>') else { continue };
            let attrs = &tag[..end];
            let Some(src_at) = attrs.find("src=") else { continue };
            let rest = attrs[src_at + 4..].trim_start();
            let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                continue;
            };
            let rest = &rest[quote.len_utf8()..];
            let Some(close) = rest.find(quote) else { continue };
            let file = rest[..close].trim().to_string();
            if !file.ends_with(".js") {
                continue;
            }
            let is_module = attrs.contains("type=\"module\"") || attrs.contains("type='module'");
            if !scripts.iter().any(|(f, _)| *f == file) {
                scripts.push((file, is_module));
            }
        }
    }
    assert!(
        scripts.iter().any(|(f, _)| f == "main.js"),
        "index.html must load main.js — the discovery scan found: {scripts:?}"
    );

    // ── 2. Node availability ───────────────────────────────────────────
    let strict = std::env::var("NANOCLICK_JS_SYNTAX_STRICT")
        .map(|v| v != "0")
        .unwrap_or(true);
    let node_works = std::process::Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !node_works {
        assert!(
            !strict,
            "Node.js is required for the mandatory JS grammar check. \
             Install Node.js, or set NANOCLICK_JS_SYNTAX_STRICT=0 to skip it."
        );
        eprintln!("[js-syntax] Node.js not found — check skipped (strict mode off)");
        return;
    }

    // ── 3. Parse every script in the mode the browser will use ─────────
    let tmp_dir = std::env::temp_dir().join(format!("nanoclick_js_syntax_{}", std::process::id()));
    std::fs::create_dir_all(&tmp_dir).expect("temp dir for the syntax check");

    let mut failures: Vec<String> = Vec::new();
    for (file, is_module) in &scripts {
        let Some(source) = read_asset(file) else {
            failures.push(format!("{file}: listed in HTML but NOT embedded in the build"));
            continue;
        };
        // `.mjs` forces module mode; `.js` in a bare temp dir is a classic
        // script — exactly matching how the HTML loads each file.
        let path = tmp_dir.join(format!("{}.{}", file.replace('/', "_"), if *is_module { "mjs" } else { "js" }));
        std::fs::write(&path, &source).expect("write temp script");
        let out = std::process::Command::new("node")
            .arg("--check")
            .arg(&path)
            .output()
            .expect("run node --check");
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr).into_owned();
            failures.push(format!(
                "{file} ({}):\n{}",
                if *is_module { "module" } else { "classic" },
                err.trim()
            ));
        }
        let _ = std::fs::remove_file(&path);
    }
    let _ = std::fs::remove_dir_all(&tmp_dir);

    assert!(
        failures.is_empty(),
        "JavaScript grammar check failed — the browser would refuse to run these files:\n\n{}",
        failures.join("\n\n")
    );
}

/// Boot-integrity wiring (v1.1.0, Stage 0): the runtime crash trap must live in
/// a CLASSIC script that is loaded BEFORE the module, and the boot watchdog must
/// have a matching handshake flag in the page script.
///
/// Why it matters here: `main.js` is `<script type="module">`, and a module-level
/// SyntaxError means the module body never executes — so a trap written at the top
/// of `main.js` can never fire for that class, leaving a rendered-but-dead UI with
/// zero diagnostics (`tech.md` §Lessons Learned #1, `.notes/ROOT-CAUSE-ANALYSIS.md`).
#[test]
fn test_frontend_pages_load_boot_guard_before_modules() {
    let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();
    let read_asset = |name: &str| -> Option<String> {
        let key = tauri::utils::assets::AssetKey::from(name);
        ctx.assets()
            .get(&key)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    };

    // ── 1. The guard is embedded and reports through the Rust log ──────────
    let guard = read_asset("boot_guard.js").expect("boot_guard.js must be embedded in the frontend");
    for needle in [
        "__nanoclick_boot_guard_installed__",
        "__TAURI_INTERNALS__",
        "unhandledrejection",
        "debug_log",
        "nanoclick-boot-failure",
    ] {
        assert!(guard.contains(needle), "boot_guard.js must contain `{needle}`");
    }

    // ── 2. Every page: classic guard tag, loaded before the first module ────
    let pages = [
        ("index.html", "__nanoclick_boot_ok__", "main.js"),
        ("hud.html", "__nanoclick_hud_boot_ok__", "hud.js"),
        ("overlay.html", "__nanoclick_overlay_boot_ok__", "overlay.js"),
    ];
    for (page, flag, page_script) in pages {
        let html = read_asset(page).unwrap_or_else(|| panic!("{page} must be embedded"));
        // NB: search for the TAG attribute, not the bare file name — the HTML
        // comments above the tag also mention `src/boot_guard.js` (whoever
        // greps for the file name finds a comment first, and that already broke
        // this test once).
        let guard_at = html
            .find("src=\"boot_guard.js\"")
            .unwrap_or_else(|| panic!("{page} must load boot_guard.js"));
        let tag_at = html[..guard_at]
            .rfind("<script")
            .unwrap_or_else(|| panic!("{page}: boot_guard.js must sit inside a <script> tag"));
        assert!(
            !html[tag_at..guard_at].contains("module"),
            "{page}: boot_guard.js must be a CLASSIC script — a module cannot trap its own parse failure"
        );
        assert!(
            html.contains(&format!("data-boot-flag=\"{flag}\"")),
            "{page} must point the guard at its own boot flag `{flag}`"
        );
        if let Some(module_at) = html.find("type=\"module\"") {
            assert!(
                guard_at < module_at,
                "{page}: boot_guard.js must be loaded BEFORE the first module script"
            );
        }
        let page_src = read_asset(page_script).unwrap_or_else(|| panic!("{page_script} must be embedded"));
        assert!(
            page_src.contains(flag),
            "{page_script} must raise the boot flag `{flag}` — otherwise the watchdog fires on a healthy boot"
        );
    }
}

/// Harness for `test_boot_guard_behaviour_on_node`: runs the real `boot_guard.js`
/// inside a minimal DOM/IPC stub on Node (V8) and prints one JSON verdict line.
/// `process.argv[2]` = "dead" (no handshake flag → watchdog must fire) or
/// "healthy" (flag raised → watchdog must stay silent).
const BOOT_GUARD_HARNESS: &str = r#"
const fs = require("fs");
const vm = require("vm");
const path = require("path");

const mode = process.argv[2] || "dead";
const calls = [];
const created = [];
const listeners = {};

const windowStub = {
  addEventListener(type, fn) { (listeners[type] = listeners[type] || []).push(fn); },
  console: console,
  __TAURI_INTERNALS__: {
    invoke(cmd, args) { calls.push({ cmd: cmd, args: args }); return Promise.resolve(); }
  }
};

const documentStub = {
  currentScript: {
    getAttribute(name) {
      if (name === "data-boot-flag") return "__test_boot_ok__";
      if (name === "data-boot-label") return "test";
      if (name === "data-boot-timeout-ms") return "60";
      return null;
    }
  },
  readyState: "complete",
  getElementById() { return null; },
  createElement(tag) {
    const el = { tag: tag, attrs: {}, textContent: "" };
    el.setAttribute = function (k, v) { el.attrs[k] = v; };
    created.push(el);
    return el;
  },
  addEventListener() {},
  body: { appendChild() {} },
  documentElement: { appendChild() {} }
};

const sandbox = {
  window: windowStub,
  document: documentStub,
  setTimeout: setTimeout,
  console: console,
  Object: Object,
  String: String,
  Number: Number,
  parseInt: parseInt,
  Promise: Promise,
  Error: Error
};
sandbox.globalThis = sandbox;
vm.createContext(sandbox);
vm.runInContext(
  fs.readFileSync(path.join(__dirname, "boot_guard.js"), "utf8"),
  sandbox,
  { filename: "boot_guard.js" }
);

if (mode === "healthy") { windowStub.__test_boot_ok__ = true; }

// one synchronous crash (sent twice on purpose), one resource 404, one rejection
const crash = { message: "boom", filename: "main.js", lineno: 42, colno: 7, error: { stack: "at boom" } };
listeners["error"][0](crash);
listeners["error"][0](crash);
listeners["error"][0]({ target: { tagName: "SCRIPT", src: "http://x/missing.js" } });
listeners["unhandledrejection"][0]({ reason: new Error("nope") });

setTimeout(function () {
  const lines = calls.map(function (c) { return c.args && c.args.message ? String(c.args.message) : ""; });
  const levels = {};
  calls.forEach(function (c) { if (c.args && c.args.level) levels[c.args.level] = true; });
  console.log(JSON.stringify({
    crashReports: lines.filter(function (m) { return m.indexOf("[JS CRASH] boom") >= 0; }).length,
    resourceReports: lines.filter(function (m) { return m.indexOf("[JS RESOURCE FAIL]") >= 0; }).length,
    rejectReports: lines.filter(function (m) { return m.indexOf("[JS PROMISE REJECT]") >= 0; }).length,
    bootReports: lines.filter(function (m) { return m.indexOf("[BOOT] MODULE DID NOT EXECUTE") >= 0; }).length,
    levels: Object.keys(levels).sort(),
    banner: created.some(function (el) { return el.id === "nanoclick-boot-failure" || el.attrs.id === "nanoclick-boot-failure"; }),
    guardInstalled: windowStub.__nanoclick_boot_guard_installed__ === true
  }));
}, 260);
"#;

/// Behavioural verification of `boot_guard.js` on a real V8 (Node + the minimal
/// DOM/IPC stub from BOOT_GUARD_HARNESS above):
///   * "dead"    — the handshake flag is never raised: the guard must report
///                 `[BOOT] MODULE DID NOT EXECUTE (SyntaxError class)` exactly
///                 once AND render the banner. This is the only detection that
///                 survives a module-level SyntaxError.
///   * "healthy" — the flag is raised: the watchdog must stay silent (no false
///                 alarms on a normal boot).
/// Dedupe is asserted as well: one crash fired twice must produce one report.
#[test]
fn test_boot_guard_behaviour_on_node() {
    let strict = std::env::var("NANOCLICK_JS_SYNTAX_STRICT")
        .map(|v| v != "0")
        .unwrap_or(true);
    let node_works = std::process::Command::new("node")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !node_works {
        assert!(
            !strict,
            "Node.js is required for the boot-guard behavioural check. \
             Install Node.js, or set NANOCLICK_JS_SYNTAX_STRICT=0 to skip it."
        );
        eprintln!("[boot-guard] Node.js not found — check skipped (strict mode off)");
        return;
    }

    let guard = {
        let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();
        let key = tauri::utils::assets::AssetKey::from("boot_guard.js");
        let bytes = ctx.assets().get(&key).expect("boot_guard.js must be embedded");
        String::from_utf8_lossy(&bytes).into_owned()
    };

    let tmp_dir = std::env::temp_dir().join(format!("nanoclick_boot_guard_{}", std::process::id()));
    std::fs::create_dir_all(&tmp_dir).expect("temp dir for the boot-guard harness");
    std::fs::write(tmp_dir.join("boot_guard.js"), &guard).expect("write boot_guard.js copy");
    std::fs::write(tmp_dir.join("harness.js"), BOOT_GUARD_HARNESS).expect("write the harness");

    let run = |mode: &str| -> serde_json::Value {
        let out = std::process::Command::new("node")
            .arg(tmp_dir.join("harness.js"))
            .arg(mode)
            .output()
            .expect("run the boot-guard harness");
        assert!(
            out.status.success(),
            "boot-guard harness failed ({mode}):\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).trim())
            .unwrap_or_else(|e| panic!("harness must print one JSON line ({mode}): {e}"))
    };

    // ── dead module: the watchdog must fire ───────────────────────────────
    let dead = run("dead");
    assert_eq!(dead["guardInstalled"], serde_json::Value::Bool(true), "guard must install itself: {dead}");
    assert_eq!(dead["bootReports"], 1, "a missing handshake must be reported exactly once: {dead}");
    assert_eq!(dead["banner"], serde_json::Value::Bool(true), "a dead module must show the banner: {dead}");
    assert_eq!(dead["crashReports"], 1, "the same crash fired twice must be deduped: {dead}");
    assert_eq!(dead["resourceReports"], 1, "a 404 script must be reported: {dead}");
    assert_eq!(dead["rejectReports"], 1, "an unhandled rejection must be reported: {dead}");
    assert_eq!(dead["levels"], serde_json::json!(["error"]), "every report must be level \"error\": {dead}");

    // ── healthy boot: no false alarm ──────────────────────────────────────
    let healthy = run("healthy");
    assert_eq!(healthy["bootReports"], 0, "a healthy boot must not trip the watchdog: {healthy}");
    assert_eq!(healthy["banner"], serde_json::Value::Bool(false), "no banner on a healthy boot: {healthy}");

    let _ = std::fs::remove_dir_all(&tmp_dir);
}

/// i18n symmetry: every notice key used by Rust/JS must exist in all 3 locales.
#[test]
fn test_notice_i18n_symmetry() {
    let keys = [
        "deadbolt_ok",
        "notice_cfg_repaired_title",
        "notice_cfg_repaired_msg",
        "notice_cfg_corrupted_title",
        "notice_cfg_lkg_msg",
        "notice_cfg_factory_msg",
        "notice_cfg_migrated_title",
        "notice_cfg_migrated_msg",
        "notice_macros_lkg_title",
        "notice_macros_lkg_msg",
        "notice_macros_empty_title",
        "notice_macros_empty_msg",
        "notice_watch_changed_title",
        "notice_watch_changed_msg",
        "notice_watch_invalid_title",
        "notice_watch_invalid_msg",
        "toast_watch_changed",
        "toast_watch_invalid",
        "settings_pause_focus_loss",
        "settings_remember_pos",
        "focus_loss_paused_notify",
    ];
    for locale in ["en", "ua", "ru"] {
        let asset = format!("locales/{locale}.json");
        let key = tauri::utils::assets::AssetKey::from(asset.as_str());
        let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();
        let bytes = ctx.assets().get(&key).unwrap_or_else(|| panic!("{locale}.json must be embedded"));
        let text = String::from_utf8_lossy(&bytes);
        let v: serde_json::Value = serde_json::from_str(&text).expect("locale must be valid JSON");
        for k in keys {
            assert!(v.get(k).is_some_and(|x| x.is_string()), "locale {locale} missing notice key {k}");
        }
    }
}

/// Windows test-host wiring: `tauri/common-controls-v6` must stay OFF.
///
/// A test binary can die before the first test runs, and then the gate reports
/// nothing but `process didn't exit successfully ... 0xC0000139`
/// (`STATUS_ENTRYPOINT_NOT_FOUND`). That is what happened while that feature was
/// enabled: it forwards to `muda/common-controls-v6`, the only thing in the whole
/// dependency graph that calls `comctl32.dll!TaskDialogIndirect` — an export that
/// exists only in comctl32 **v6**, which an application manifest must activate.
/// The unittest binary built from `src/lib.rs` is not a bin target, so
/// `tauri-build`'s `cargo:rustc-link-arg-bins=...resource.lib` never reaches it
/// (verified: `dumpbin /dependents` reports no `.rsrc` section in it), the loader
/// binds comctl32 v5.82, and Windows kills the process before the first test.
///
/// There is no way to hand a manifest to a lib unittest target from a build
/// script: `rustc-link-arg-tests` does not reach it (measured) and
/// `/MANIFESTDEPENDENCY` embeds nothing on its own (measured). So the dependency
/// graph must not contain the import at all — hence the opt-out below.
///
/// Verify with: `cargo tree -e features -i muda --offline`
#[test]
fn test_tauri_common_controls_v6_stays_disabled() {
    let cargo_toml = include_str!("../Cargo.toml");

    // Only the dependency block: the comment above it names the feature on
    // purpose, to explain why it is banned.
    let mut dep = String::new();
    let mut inside = false;
    for line in cargo_toml.lines() {
        let t = line.trim();
        if t.starts_with("tauri = {") {
            inside = true;
        }
        if inside {
            dep.push_str(t);
            if t.starts_with(']') {
                break;
            }
        }
    }

    assert!(inside, "the `tauri = {{ ... }}` dependency block must exist");
    assert!(
        dep.contains("default-features = false"),
        "`tauri` must opt out of its default features — that set enables common-controls-v6"
    );
    assert!(
        !dep.contains("common-controls-v6"),
        "common-controls-v6 pulls comctl32!TaskDialogIndirect (via muda) into the lib \
         unittest binary, which has no manifest and dies with 0xC0000139 before the first test"
    );
    for feature in ["wry", "compression", "dynamic-acl", "x11", "dbus"] {
        assert!(
            dep.contains(feature),
            "tauri default feature `{feature}` must stay enabled: opt out of that one feature, \
             not of its siblings (see the comment in src-tauri/Cargo.toml)"
        );
    }
}

#[test]
fn test_native_tray_and_lifetime_wiring() {
    // ── The tray must stay dependency-free ──────────────────────────────────
    // `tray-icon` forwards to `muda`, and `muda`'s Windows menu code links
    // `comctl32!TaskDialogIndirect` when `common-controls-v6` is on — the exact
    // symbol that killed the lib unittest binary with 0xC0000139.
    let cargo_toml = include_str!("../Cargo.toml");
    assert!(
        !cargo_toml.contains("tray-icon"),
        "the tray must not be built from the tray-icon feature (it pulls muda)"
    );

    let tray_src = include_str!("../src/tray.rs");
    assert!(
        tray_src.contains("Shell_NotifyIconW"),
        "tray must call the Win32 shell API directly"
    );
    assert!(
        tray_src.contains("TrackPopupMenuEx"),
        "the tray menu must be a native popup menu"
    );
    assert!(
        tray_src.contains("TaskbarCreated"),
        "the icon must come back after explorer.exe restarts"
    );
    assert!(
        tray_src.contains("WS_EX_TOOLWINDOW") && tray_src.contains("WS_POPUP"),
        "the message window must be a hidden TOP-LEVEL window"
    );
    // The banned constructs are QUOTED in a comment that explains why they are
    // banned (`HWND_MESSAGE` is mentioned exactly there), so the scan strips
    // comment lines and inspects executable code only.
    let tray_code: String = tray_src
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !tray_code.contains("HWND_MESSAGE"),
        "a message-only window never receives the TaskbarCreated broadcast"
    );
    assert!(
        !tray_code.contains("EmptyWorkingSet") && !tray_code.contains("SetProcessWorkingSetSize"),
        "no working-set trimming on the tray path: it trades RAM for hard page faults"
    );

    // ── Process lifetime: the backend outlives its windows ──────────────────
    let lib_src = include_str!("../src/lib.rs");
    assert!(lib_src.contains("mod tray;"), "tray module must be wired");
    assert!(
        lib_src.contains("RunEvent::ExitRequested"),
        "the backend must handle ExitRequested"
    );
    assert!(
        lib_src.contains("prevent_exit"),
        "ExitRequested must be prevented while the app is not shutting down"
    );
    assert!(
        lib_src.contains("SHUTTING_DOWN"),
        "a latch is required: tauri raises ExitRequested from exit() too, so an \
         unlatched prevent_exit() would make quitting impossible"
    );
    assert!(
        lib_src.contains("prevent_close"),
        "close-to-tray must prevent the close instead of killing the process"
    );
    assert!(
        lib_src.contains("minimize_to_tray"),
        "the BEHAVIOR checkbox must drive the close behaviour"
    );
    assert!(
        lib_src.contains("start_tray_if_needed") && lib_src.contains("stop_tray"),
        "the icon must start with the app and be removed on shutdown"
    );

    // The exit watchdog keeps its one allowed working-set trim; nothing else may
    // reintroduce it (AGENTS.md §3 forbids it on any hot path).
    let shutdown_at = lib_src
        .find("fn shutdown_application")
        .expect("shutdown_application must exist");
    let trim_at = lib_src
        .find("SetProcessWorkingSetSize")
        .expect("the exit watchdog must keep its final trim");
    assert!(
        trim_at > shutdown_at,
        "SetProcessWorkingSetSize is only allowed inside shutdown_application \
         (after app.exit(), before process::exit) — never on a live path"
    );

    // ── No white flash, and `start_minimized` really starts hidden ──────────
    let conf: serde_json::Value = serde_json::from_str(include_str!("../tauri.conf.json"))
        .expect("tauri.conf.json must be valid JSON");
    assert_eq!(
        conf["app"]["windows"][0]["visible"],
        serde_json::json!(false),
        "the main window must be created hidden: setup() shows it, so nothing flashes"
    );
}

/// Deep sleep (Phase D) + frontend watchdog (Phase E) wiring.
///
/// Both exist for one reason: the page is disposable, the backend is not.
/// * Deep sleep DESTROYS the WebView while the app sits in the tray (~250 MB →
///   ~15 MB) and rebuilds it on the next tray click. Three traps must stay
///   closed: never mid-run (a live clicker would lose its UI and the focus
///   guard's baseline would move), never before the page flushed its state (stats
///   reach disk at most every 5 s), and never let the page's own
///   `beforeunload` → `exit_app` fallback kill the backend during that unload.
/// * The watchdog notices a page that never reported `frontend_ready` and reacts
///   without ever ending the process.
#[test]
fn test_deep_sleep_and_frontend_watchdog_wiring() {
    fn section<'a>(src: &'a str, start: &str, end: &str) -> &'a str {
        let from = src.find(start).unwrap_or_else(|| panic!("`{start}` not found"));
        match src[from..].find(end) {
            Some(i) => &src[from..from + i],
            None => &src[from..],
        }
    }

    let lib_src = include_str!("../src/lib.rs");

    // ── Phase E: frontend heartbeat ─────────────────────────────────────────
    assert!(
        lib_src.contains("fn frontend_ready"),
        "the frontend_ready command must exist"
    );
    assert!(
        lib_src.contains("FRONTEND_READY_TIMEOUT_MS") && lib_src.contains("WatchdogVerdict"),
        "the watchdog must have a timeout and a pure verdict"
    );
    let watchdog = section(
        lib_src,
        "fn spawn_frontend_watchdog",
        "// ── Deep sleep to tray",
    );
    assert!(
        watchdog.contains("location.reload()"),
        "the watchdog must try exactly one reload"
    );
    assert!(
        !watchdog.contains("process::exit") && !watchdog.contains("shutdown_application"),
        "the watchdog may never end the process: a broken page must not kill the backend"
    );

    // ── Phase D: deep sleep ────────────────────────────────────────────────
    assert!(
        lib_src.contains("fn deep_sleep_allowed") && lib_src.contains("deep_sleep_allowed_now"),
        "deep sleep must be gated by a pure predicate, not by inline state reads"
    );
    assert!(
        lib_src.contains("WebviewWindowBuilder::from_config"),
        "the rebuilt window must come from tauri.conf.json (no drift-prone second copy)"
    );
    assert!(
        lib_src.contains("MAIN_REBUILDING"),
        "two fast tray clicks must not race on the window label"
    );
    let suspend = section(
        lib_src,
        "pub(crate) fn suspend_main_webview_to_tray",
        "pub(crate) fn shutdown_application",
    );
    assert!(
        suspend.contains("TRAY_SUSPEND_ARMED") && suspend.contains(".destroy()"),
        "the suspension must arm the exit guard and destroy the WebView"
    );
    assert!(
        suspend.contains("__NANOCLICK_RELOADING__"),
        "the page must be told the unload is intentional (its beforeunload calls exit_app)"
    );
    assert!(
        suspend.contains("tray_flush_done") || suspend.contains("TRAY_FLUSH_ACK"),
        "the suspension must wait for the page's flush ack"
    );
    assert!(
        !suspend.contains("process::exit"),
        "suspending a window must never terminate the process"
    );
    let exit_app = section(lib_src, "fn exit_app", "// ── Tray + process lifetime");
    assert!(
        exit_app.find("TRAY_SUSPEND_ARMED").unwrap_or(usize::MAX)
            < exit_app.find("shutdown_application").unwrap_or(usize::MAX),
        "exit_app must consult the suspension guard BEFORE shutting anything down"
    );

    // ── The setting itself: config, repair-merge, UI, i18n ─────────────────
    assert!(
        include_str!("../src/config_manager.rs").contains("deep_sleep_to_tray"),
        "the config must persist the new flag"
    );
    assert!(
        include_str!("../src/defaults/mod.rs").contains("\"deep_sleep_to_tray\""),
        "the repair/merge keep-list must carry the flag, or an update resets it"
    );
    assert!(
        include_str!("../src/defaults/default_config.json").contains("deep_sleep_to_tray"),
        "the factory default must contain the flag"
    );
    let html = include_str!("../../src/index.html");
    assert!(
        html.contains("trayLifeCheckbox") && html.contains("settings_tray_life"),
        "the BEHAVIOR card must expose the merged tray switch with an i18n label"
    );
    let main_js = {
        let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();
        let key = tauri::utils::assets::AssetKey::from("main.js");
        let bytes = ctx.assets().get(&key).expect("main.js must be embedded");
        String::from_utf8_lossy(&bytes).into_owned()
    };
    assert!(
        main_js.contains("trayLifeCheckbox") && main_js.contains("deep_sleep_to_tray"),
        "main.js must hydrate, collect and save the tray master switch (both flags)"
    );
    assert!(
        main_js.contains("frontend_ready"),
        "main.js must report the boot heartbeat"
    );
    assert!(
        main_js.contains("__nanoclick_tray_flush__") && main_js.contains("tray_flush_done"),
        "main.js must expose the flush hook and acknowledge it"
    );
    for locale in ["ua.json", "ru.json", "en.json"] {
        let dict = match locale {
            "ua.json" => include_str!("../../src/locales/ua.json"),
            "ru.json" => include_str!("../../src/locales/ru.json"),
            _ => include_str!("../../src/locales/en.json"),
        };
        assert!(
            dict.contains("settings_tray_life"),
            "{locale} must translate the tray master switch (i18n symmetry is enforced)"
        );
    }
}

/// Tray ↔ UI state sync — **the page owns no clicker state.**
///
/// The bug class this pins: the tray menu used to run entirely behind the
/// page's back. `toggle_clicking_from_tray` called `set_active`, which returns
/// early on a veto (Work Mode / typing guard) and emitted **nothing anywhere** —
/// the 66 ms telemetry worker only exists during a live run — so the menu item
/// was indistinguishable from a dead binding. On the other end, a page rebuilt
/// after deep sleep booted with `isRunning = false` and had no way to learn that
/// the clicker was already going.
///
/// Contract now enforced here:
/// * `set_active` returns a `ToggleOutcome` (callers must be able to tell a
///   started clicker from a refused one);
/// * the tray always reports the outcome, and explains a veto;
/// * the UI toggle command returns the state that EXISTS, not the requested one;
/// * the page asks `get_status` on boot and never persists a payload that
///   arrived before its config was hydrated;
/// * the one config field the tray writes behind the page's back (`show_hud`)
///   is mirrored into the checkbox **without** a save, or the stale checkbox
///   would overwrite the menu choice on the next save.
#[test]
fn test_tray_ui_state_sync_wiring() {
    fn section<'a>(src: &'a str, start: &str, end: &str) -> &'a str {
        let from = src.find(start).unwrap_or_else(|| panic!("`{start}` not found"));
        match src[from..].find(end) {
            Some(i) => &src[from..from + i],
            None => &src[from..],
        }
    }

    let lib_src = include_str!("../src/lib.rs");
    let sched_src = include_str!("../src/scheduler.rs");

    // ── Backend: the scheduler reports, the tray tells the page ─────────────
    assert!(
        sched_src.contains(
            "pub fn set_active(&self, active: bool, app_handle: Option<&AppHandle>) -> ToggleOutcome"
        ),
        "set_active must return what really happened, or a vetoed start is invisible"
    );
    assert!(
        sched_src.contains("pub(crate) fn emit_status_now"),
        "out-of-band callers (tray menu, UI command) need a live-state emitter"
    );

    let tray_toggle = section(
        lib_src,
        "pub(crate) fn toggle_clicking_from_tray",
        "/// Tray menu: show/hide the floating HUD",
    );
    assert!(
        tray_toggle.contains("emit_status_now"),
        "a tray toggle must push the new state to the page"
    );
    assert!(
        tray_toggle.find("set_active").unwrap_or(usize::MAX)
            < tray_toggle.find("emit_status_now").unwrap_or(0),
        "the emit must carry the state AFTER the toggle, not before it"
    );
    assert!(
        tray_toggle.contains("tray-action-result"),
        "a vetoed tray action must not be a silent no-op (Work Mode / typing guard)"
    );

    let hud_toggle = section(
        lib_src,
        "pub(crate) fn toggle_hud_from_tray",
        "/// Should the window's close button",
    );
    assert!(
        hud_toggle.contains("tray-hud-changed"),
        "the tray writes ui.show_hud itself; the page's checkbox must hear about it"
    );

    let toggle_cmd = section(lib_src, "fn toggle_autoclicker", "fn get_status");
    assert!(
        toggle_cmd.contains("state.scheduler.is_active()"),
        "the UI command must answer the state that EXISTS, not the requested one"
    );
    assert!(
        !toggle_cmd.contains("let now_active = !state.scheduler.is_active()"),
        "answering the requested value paints RUNNING over a vetoed (idle) clicker"
    );
    assert!(
        toggle_cmd.contains("emit_status_now"),
        "a stop / vetoed UI toggle must settle the UI instead of leaving it guessing"
    );

    // ── Frontend: one apply function, one boot-time truth fetch ─────────────
    let main_js = {
        let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();
        let key = tauri::utils::assets::AssetKey::from("main.js");
        let bytes = ctx.assets().get(&key).expect("main.js must be embedded");
        String::from_utf8_lossy(&bytes).into_owned()
    };
    assert!(
        main_js.contains("function applyStatusUpdate"),
        "the status payload must have a single apply point (event + get_status)"
    );
    assert!(
        main_js.contains("applyStatusUpdate(event?.payload"),
        "the status-update listener must delegate to that single apply point"
    );
    assert!(
        main_js.contains("invoke(\"get_status\")"),
        "boot must ask the backend what is running — the page has no state of its own"
    );
    assert!(
        main_js.contains("configHydrated"),
        "a payload that lands before loadConfig() must not save module defaults to disk"
    );
    assert!(
        main_js.contains("listen(\"tray-action-result\""),
        "the page must be able to explain a vetoed tray action"
    );
    assert!(
        main_js.contains("listen(\"tray-hud-changed\""),
        "the BEHAVIOR checkbox must follow the tray's HUD toggle"
    );
    // The mirror must NOT save: the tray already persisted it, and a save from
    // the page would write its stale checkbox value back (the exact regression
    // this wiring exists to prevent).
    let hud_listener = section(&main_js, "listen(\"tray-hud-changed\"", "\n// ──");
    assert!(
        !hud_listener.contains("saveConfig()"),
        "mirroring tray state must not save — it would undo the tray's own write"
    );

    for locale in ["ua.json", "ru.json", "en.json"] {
        let dict = match locale {
            "ua.json" => include_str!("../../src/locales/ua.json"),
            "ru.json" => include_str!("../../src/locales/ru.json"),
            _ => include_str!("../../src/locales/en.json"),
        };
        assert!(
            dict.contains("tray_blocked_work_mode") && dict.contains("tray_blocked_typing"),
            "{locale} must translate both veto reasons (i18n symmetry is enforced)"
        );
    }
}

/// Work / Autoclicker mode must be switchable with **zero windows alive**.
///
/// With deep sleep the page does not exist, so the mode badge is gone (and the
/// mode hotkey gives no feedback either): before this, changing the mode while
/// the app sat in the tray was impossible without restoring the interface first.
/// The tray menu now carries the mode item, its check mark is refreshed from the
/// scheduler's atomic right before the menu opens (the tray pump must not touch
/// the config file), and every entry point — badge, hotkey, menu — shares one
/// helper that persists `active_mode`. The hotkey path used to flip the atomic
/// only, so the next config load (`set_config` re-reads the field) silently
/// reverted the switch.
#[test]
fn test_tray_mode_menu_and_persisted_mode_switch() {
    let tray_src = include_str!("../src/tray.rs");
    let lib_src = include_str!("../src/lib.rs");
    let win_src = include_str!("../src/platform/windows/mod.rs");

    // ── The menu item itself ───────────────────────────────────────────────
    assert!(
        tray_src.contains("MENU_ID_TOGGLE_MODE") && tray_src.contains("ToggleMode"),
        "the tray menu must offer the mode switch"
    );
    assert!(
        tray_src.contains("CheckMenuItem") && tray_src.contains("mode_item_flags"),
        "the current mode must be visible in the menu, not only in a log line"
    );
    assert!(
        tray_src.contains("crate::tray_work_mode_active"),
        "the check mark must come from the scheduler atomic, not from the config file"
    );

    // ── One shared, PERSISTED implementation ───────────────────────────────
    assert!(
        lib_src.contains("pub(crate) fn apply_mode_toggle"),
        "mode switching needs one implementation for badge, hotkey and tray"
    );
    assert!(
        lib_src.contains("cfg.active_mode = new_mode.clone()"),
        "the mode must be persisted, or the next config load reverts it"
    );
    assert!(
        lib_src.contains("pub(crate) fn toggle_mode_from_tray"),
        "the tray needs its own entry point: it runs with zero windows"
    );
    assert!(
        win_src.contains("crate::apply_mode_toggle") && !win_src.contains("scheduler.toggle_mode("),
        "the global mode hotkey must persist the mode through the shared helper"
    );
}

/// Tray hardening + input diagnostics.
///
/// Everything here fixes a "the tray/minimize works badly" report: the `_`
/// (minimize) button left the window in the taskbar, the HUD stayed on screen
/// after closing to the tray, a resized window lost its size after a deep-sleep
/// rebuild, and a tray click could not pull the window in front of a focused
/// game. On top of that the tray now shows live state and can reload or restart
/// the app, and the input layer's decision log became readable.
#[test]
fn test_tray_hardening_and_input_diagnostics_wiring() {
    fn section<'a>(src: &'a str, start: &str, end: &str) -> &'a str {
        let from = src.find(start).unwrap_or_else(|| panic!("`{start}` not found"));
        match src[from..].find(end) {
            Some(i) => &src[from..from + i],
            None => &src[from..],
        }
    }

    let lib_src = include_str!("../src/lib.rs");
    let tray_src = include_str!("../src/tray.rs");
    let win_src = include_str!("../src/platform/windows/mod.rs");
    let cm_src = include_str!("../src/config_manager.rs");
    let defaults_src = include_str!("../src/defaults/mod.rs");

    // ── F1: the minimize button must behave like the close button ───────────
    assert!(
        lib_src.contains("tauri::WindowEvent::Resized(_)") && lib_src.contains("is_minimized()"),
        "minimizing to the taskbar must route into the tray (F1)"
    );
    assert!(
        lib_src.contains("minimize requested -> hidden to tray")
            && lib_src.contains("minimize requested -> deep sleep"),
        "both tray lifestyles must be reachable from the minimize path"
    );

    // ── F2: HUD/overlay must not linger on screen ───────────────────────────
    assert!(
        lib_src.contains("fn hide_secondary_windows") && lib_src.contains("fn show_secondary_windows"),
        "the secondary windows must follow the main window into and out of the tray (F2)"
    );
    assert!(
        lib_src.contains("hide_secondary_windows(&app_handle)"),
        "the close/hide path must use it"
    );

    // ── F3: remember the window SIZE, not just the position ─────────────────
    for needle in ["window_w", "window_h"] {
        assert!(cm_src.contains(needle), "UiSettings must carry {needle} (F3)");
        assert!(defaults_src.contains(needle), "the repair must handle {needle}");
    }
    assert!(
        lib_src.contains("inner_size()") && lib_src.contains("set_size("),
        "the size must be captured on save and restored on rebuild/boot (F3)"
    );

    // ── F5: a tray click must take the foreground ───────────────────────────
    assert!(
        tray_src.contains("SetForegroundWindow(state.hwnd)"),
        "the Open path must claim the foreground, or set_focus() is dropped (F5)"
    );

    // ── Tray: live state, balloon feedback, reload/restart ──────────────────
    assert!(
        tray_src.contains("clicking_item_flags") && tray_src.contains("tip_text"),
        "both state items and the tooltip must reflect the live state"
    );
    assert!(
        tray_src.contains("pub(super) fn notify_balloon") && tray_src.contains("ICON_COPY"),
        "the balloon must use the cross-thread icon copy, never TRAY_STATE"
    );
    assert!(
        lib_src.contains("fn report_tray_action"),
        "tray actions need one place that decides toast vs balloon"
    );
    assert!(
        tray_src.contains("MENU_ID_RELOAD_UI") && tray_src.contains("MENU_ID_RESTART_APP"),
        "the menu needs Reload interface + Restart app"
    );
    assert!(
        lib_src.contains("fn reload_interface_from_tray") && lib_src.contains("fn restart_app_from_tray"),
        "both new menu actions must exist"
    );
    // The reload decision must stay a pure, single-sourced function (no inline
    // `if window.is_some()` in the handler), and restarting must tear the icon
    // down and latch the shutdown flag BEFORE tauri is asked to restart —
    // otherwise `prevent_exit` can swallow it or Windows reaps a ghost icon.
    assert!(
        lib_src.contains("fn reload_strategy")
            && lib_src.contains("reload_strategy(app.get_webview_window(\"main\").is_some())"),
        "the reload branch decision must be pure and used by the handler"
    );
    let restart = section(
        lib_src,
        "pub(crate) fn restart_app_from_tray",
        "/// Hide the secondary windows together with the main one",
    );
    assert!(
        restart.find("SHUTTING_DOWN.store").unwrap_or(usize::MAX)
            < restart.find("app.restart()").unwrap_or(0),
        "the shutdown latch must be set before restart()"
    );
    assert!(
        restart.find("stop_tray()").unwrap_or(usize::MAX) < restart.find("app.restart()").unwrap_or(0),
        "the tray icon must be removed before the process image changes"
    );

    // ── Input diagnostics: the buffers must be readable ─────────────────────
    assert!(
        lib_src.contains("fn dump_input_diagnostics"),
        "the diagnostics command must exist"
    );
    assert!(
        include_str!("../src/platform/mod.rs").contains("pub fn hotkey_diag_dump"),
        "the platform layer must expose the hook's ring buffer"
    );
    assert!(
        win_src.contains("\"reason\": \"typing\"") && win_src.contains("left_ms"),
        "a toggle swallowed by the typing lockout must say so (it looked like a dead bind)"
    );

    // ── Frontend wiring ────────────────────────────────────────────────────
    let main_js = {
        let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();
        let key = tauri::utils::assets::AssetKey::from("main.js");
        let bytes = ctx.assets().get(&key).expect("main.js must be embedded");
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let html = include_str!("../../src/index.html");
    assert!(
        main_js.contains("dumpInputDiagBtn") && main_js.contains("invoke(\"dump_input_diagnostics\")"),
        "main.js must offer the diagnostics dump"
    );
    assert!(
        main_js.contains("trayLifeCheckbox") && main_js.contains("trayLifeInitial"),
        "one master switch drives minimize + deep sleep, without silent writes"
    );
    assert!(
        main_js.contains("admin_restart_required"),
        "the admin checkbox must say that a restart is required"
    );
    assert!(
        html.contains("trayLifeCheckbox") && html.contains("settings_tray_life"),
        "the BEHAVIOR card must expose the merged tray switch"
    );
    assert!(
        html.contains("dumpInputDiagBtn") && html.contains("settings_always_run_admin_hint"),
        "the diagnostics button and the admin hint must be in the markup"
    );
    assert!(
        !html.contains("minimizeToTrayCheckbox") && !html.contains("deepSleepToTrayCheckbox"),
        "the two old tray checkboxes must be gone (merged into the master)"
    );

    for locale in ["ua.json", "ru.json", "en.json"] {
        let dict = match locale {
            "ua.json" => include_str!("../../src/locales/ua.json"),
            "ru.json" => include_str!("../../src/locales/ru.json"),
            _ => include_str!("../../src/locales/en.json"),
        };
        for key in [
            "settings_tray_life",
            "settings_tray_life_hint",
            "settings_always_run_admin_hint",
            "admin_restart_required",
            "settings_dump_diag",
            "diag_dump_done",
        ] {
            assert!(dict.contains(key), "{locale} must translate {key}");
        }
    }
}

/// The Zero-Jitter contract as a tripwire: **no IPC call may be able to panic.**
///
/// Every `emit` is fire-and-forget on a targeted window and every window lookup
/// degrades to `None`. One `.unwrap()` on either would let a dead, hidden or
/// hanging WebView kill the thread that owns the LL hook or the click loop — the
/// frontend is disposable, the backend is not (AGENTS.md §3, `tech.md` §Zero
/// Jitter). The discipline was already correct in the whole tree; this test only
/// stops it from rotting.
#[test]
fn test_no_unwrap_on_window_or_emit() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files_checked = 0usize;
    let mut offenders: Vec<String> = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("src/ must be readable ({}): {e}", dir.display()));
        for entry in entries {
            let path = entry.expect("readable dir entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let src = std::fs::read_to_string(&path).expect("readable .rs file");
            files_checked += 1;
            for (idx, line) in src.lines().enumerate() {
                let trimmed = line.trim_start();
                // A doc comment may quote the very pattern this test bans.
                if trimmed.starts_with("//") {
                    continue;
                }
                let touches_window_or_ipc =
                    line.contains(".emit(") || line.contains("get_webview_window(");
                let can_panic = line.contains(".unwrap()") || line.contains(".expect(");
                if touches_window_or_ipc && can_panic {
                    offenders.push(format!("{}:{}: {}", path.display(), idx + 1, trimmed));
                }
            }
        }
    }
    assert!(
        files_checked >= 10,
        "expected to scan the whole src/ tree, saw only {files_checked} files"
    );
    assert!(
        offenders.is_empty(),
        "IPC/window calls must never unwrap or expect — they run on the hook and \
         click paths, where a poisoned window would take the backend down:\n{}",
        offenders.join("\n")
    );
}

/// `tauri.conf.json` must parse, and it must agree with `Cargo.toml` on the version.
///
/// Both files are read by the release pipeline: `release.ps1 -Tag vX.Y.Z` names the
/// installer and `latest.json` from the tag, while tauri-build embeds the version
/// from these two files into the binary. A bump that corrupts the JSON — a bad
/// regex replacement once left `$11.1.1",` on line 4 — kills *every* cargo command
/// inside tauri-build ("unable to parse JSON ... key must be a string") and is
/// discovered only after minutes of a release preflight. This catches it in
/// seconds, at the gate.
#[test]
fn test_tauri_config_is_valid_json_and_version_matches() {
    let raw = include_str!("../tauri.conf.json");
    let cfg: serde_json::Value = serde_json::from_str(raw)
        .expect("tauri.conf.json must be valid JSON: tauri-build refuses to run otherwise");

    assert_eq!(
        cfg["version"].as_str(),
        Some(env!("CARGO_PKG_VERSION")),
        "tauri.conf.json `version` must match Cargo.toml — release.ps1 -Tag reads both"
    );
}

/// The auto-stop timer must have exactly **one** owner: the Rust scheduler.
///
/// The page used to arm its own `setTimeout` that called
/// `invoke("toggle_autoclicker")` — a TOGGLE, not a stop. When the backend had
/// already stopped the run (its own timer, or the user), the page's timer fired
/// moments later and flipped the clicker back ON; it also ignored the run's start
/// delay and was armed only on the UI-button path, so a hotkey start behaved
/// differently from a button start. The page now only renders + persists the
/// value, and the backend explains its own stop through the `auto-stop` event.
///
/// Second half of the same story: the two timer inputs never persisted on input
/// (only a repaint was wired), so the field could show "10 min" while
/// `engine.stop_duration_ms` stayed 0 on disk — the click loop reads the CONFIG,
/// so it never stopped at all. That is the `saveConfigThrottled()` assertion.
#[test]
fn test_auto_stop_timer_has_one_owner() {
    fn section<'a>(src: &'a str, start: &str, end: &str) -> &'a str {
        let from = src
            .find(start)
            .unwrap_or_else(|| panic!("`{start}` not found"));
        match src[from..].find(end) {
            Some(i) => &src[from..from + i],
            None => &src[from..],
        }
    }

    let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();
    let asset = |name: &str| -> String {
        let key = tauri::utils::assets::AssetKey::from(name);
        let bytes = ctx
            .assets()
            .get(&key)
            .unwrap_or_else(|| panic!("{name} must be embedded"));
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let main_js = asset("main.js");
    let html = asset("index.html");

    // ── 1. No page-owned auto-stop timer ────────────────────────────────────
    for banned in ["stopDurationTimer", "stopTimeTimer"] {
        assert!(
            !main_js.contains(banned),
            "`{banned}` is a SECOND owner of the auto-stop timer; the backend already snapshots stop_duration_ms at run start"
        );
    }
    let backend_stage = section(&main_js, "auto-stop-owned-by-backend", "// ── STEP 5");
    assert!(
        !backend_stage.contains("setTimeout"),
        "the start path must not arm a page-side stop timer: it TOGGLES on expiry, so a run the backend already stopped is restarted by its own frontend timer"
    );
    assert!(
        !backend_stage.contains("toggle_autoclicker"),
        "an auto-stop may never be expressed as a toggle — a toggle on an already-idle clicker STARTS it"
    );

    // ── 2. The backend reports its own stop; the page only explains it ──────
    let sched_src = include_str!("../src/scheduler.rs");
    assert!(
        sched_src.contains("\"auto-stop\""),
        "a self-inflicted stop must not be silent: emit `auto-stop` so the page can explain it"
    );
    assert!(
        sched_src.contains("auto_stop_reason"),
        "the reason is set inside the loop and emitted after it — the click path stays IPC-free (Zero-Jitter)"
    );
    let cfg_src = include_str!("../src/config.rs");
    assert!(
        cfg_src.contains("pub const MAX_STOP_DURATION_MS: u64 = 3_596_400_000;"),
        "the ceiling belongs in the backend, not only in an input's max attribute"
    );
    assert!(
        main_js.contains("const STOP_DURATION_HARD_MAX_MS = 3596400000;"),
        "frontend and backend must agree on the ceiling, or one of them silently truncates"
    );
    assert!(
        main_js.contains("listen(\"auto-stop\""),
        "the page must subscribe to the backend auto-stop to explain the idle state"
    );
    let auto_stop_listener = section(&main_js, "listen(\"auto-stop\"", "\n// ──");
    for forbidden in ["setRunningState", "saveConfig"] {
        assert!(
            !auto_stop_listener.contains(forbidden),
            "the auto-stop notification must not call `{forbidden}`: clicker state arrives through status-update, and only saveConfig writes the config"
        );
    }

    // ── 3. ms + unit plumbing, no lossy minutes round-trip ─────────────────
    let cm_src = include_str!("../src/config_manager.rs");
    assert!(
        cm_src.contains("pub stop_duration_ms: u64")
            && cm_src.contains("pub stop_duration_unit: String"),
        "the auto-stop is stored in ms plus the unit it was entered in"
    );
    assert!(
        cfg_src.contains("pub fn resolve_stop_duration_ms")
            && cfg_src.contains("parse_stop_time_str_next"),
        "Config::from must resolve ms (with the legacy-minutes migration) and the NEXT stop-at occurrence"
    );
    assert!(
        !cfg_src.contains("(app_cfg.engine.stop_duration_min as u64).saturating_mul(60_000)"),
        "the hardcoded whole-minutes conversion is the bug this change removes"
    );
    assert!(
        main_js.contains("const STOP_UNITS = {") && main_js.contains("stop_duration_unit"),
        "the page needs the unit table and must persist the chosen unit next to the ms value"
    );
    assert!(
        main_js.contains("stop_duration_min = 0"),
        "the legacy whole-minutes field must be zeroed on save, or it gets resurrected by the migration fallback"
    );

    // ── 4. The value is persisted on input (the DOM is not the config) ─────
    for field in ["stopDurationInput", "stopTimeInput"] {
        let listener = section(
            &main_js,
            &format!("document.getElementById(\"{field}\")?.addEventListener(\"input\""),
            "\n});",
        );
        assert!(
            listener.contains("saveConfigThrottled()"),
            "{field} must persist on input — the backend reads the auto-stop from the CONFIG, so a value left in the DOM never reaches the click loop"
        );
    }

    // ── 5. UI wiring + i18n ────────────────────────────────────────────────
    for id in ["stopUnitBtn", "stopUnitMenu", "stopUnitBadge"] {
        assert!(html.contains(id), "the duration unit picker needs `{id}`");
    }
    for unit in ["ms", "sec", "min", "hour"] {
        let needle = format!("data-unit=\"{unit}\"");
        assert!(
            html.contains(needle.as_str()),
            "the duration unit picker must offer `{unit}`"
        );
    }
    for locale in ["ua.json", "ru.json", "en.json"] {
        let dict = match locale {
            "ua.json" => include_str!("../../src/locales/ua.json"),
            "ru.json" => include_str!("../../src/locales/ru.json"),
            _ => include_str!("../../src/locales/en.json"),
        };
        for key in [
            "unit_dur_ms_short",
            "unit_dur_sec_short",
            "unit_dur_min_short",
            "unit_dur_hour_short",
            "unit_dur_hour_option",
            "dash_timer_unit_hint",
            "preset_field_time_limit_hint",
            "auto_stop_duration_notify",
            "auto_stop_wallclock_notify",
            "auto_stop_locked_tip",
        ] {
            assert!(dict.contains(key), "{locale} must translate {key}");
        }
    }
}

/// ONE ARMED TRIGGER. Two non-zero deadlines are ambiguous, and the card claims
/// only one of them runs — so the discriminator must exist on BOTH sides of the
/// bridge: `config::resolve_stop_mode` derives it for configs written before the
/// field existed, `scheduler.rs` snapshots it once per run and zeroes the trigger
/// it is NOT armed on, and `main.js` paints the dimmed row from the same rule.
/// The lock stays painted, never enforced, in the DOM.
#[test]
fn test_auto_stop_mode_is_the_single_armed_trigger() {
    let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();
    let asset = |name: &str| -> String {
        let key = tauri::utils::assets::AssetKey::from(name);
        let bytes = ctx
            .assets()
            .get(&key)
            .unwrap_or_else(|| panic!("{name} must be embedded"));
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let main_js = asset("main.js");
    let html = asset("index.html");
    let style_css = asset("style.css");
    let cfg_src = include_str!("../src/config.rs");
    let cm_src = include_str!("../src/config_manager.rs");
    let sched_src = include_str!("../src/scheduler.rs");
    let defaults_src = include_str!("../src/defaults/mod.rs");

    // ── 6. ONE ARMED AUTO-STOP TRIGGER (`engine.stop_mode`) ────────────────
    // Two non-zero deadlines are ambiguous, and the card claims only one of them
    // runs. The discriminator must exist on BOTH sides and the loop must gate on
    // it — otherwise the UI is a lie and the run ends on whichever came first.
    assert!(
        cfg_src.contains("pub stop_mode: String")
            && cfg_src.contains("pub fn normalize_stop_mode")
            && cfg_src.contains("pub fn resolve_stop_mode"),
        "the backend needs the armed-trigger field plus its normalizer and resolver"
    );
    assert!(
        cm_src.contains("pub stop_mode: String")
            && cm_src.matches("pub stop_mode: String").count() >= 2,
        "both the engine settings and a preset carry `stop_mode`"
    );
    assert!(
        sched_src.contains("stop_mode: Arc<Mutex<String>>")
            && sched_src.contains("crate::config::normalize_stop_mode(&cfg.stop_mode)"),
        "the scheduler must store the mode and normalize it on every config save"
    );
    assert!(
        sched_src.contains("cur_stop_mode == \"duration\"")
            && sched_src.contains("cur_stop_mode == \"wallclock\""),
        "the click loop must zero the trigger it is NOT armed on — that gate is the feature"
    );
    assert!(
        main_js.contains("function applyStopMode")
            && main_js.contains("function resolveStopMode")
            && main_js.contains("currentConfig.engine.stop_mode = currentStopMode"),
        "the page needs one place that paints the lock, one rule it shares with the backend, and must persist the mode"
    );
    assert!(
        main_js.contains("classList.toggle(\"timer-option--locked\"")
            && style_css.contains(".timer-option--locked {"),
        "the dimmed row is a CSS class the page toggles"
    );
    assert!(
        main_js
            .matches("[[\"stopAfterRow\", \"duration\"], [\"stopAtRow\", \"wallclock\"]]")
            .count()
            >= 2,
        "the dashboard rows must be wired on BOTH copies — once to paint the lock from the armed trigger and once so a click on the dimmed row re-arms it"
    );
    assert!(
        main_js
            .matches("[[\"presetStopAfterRow\", \"duration\"], [\"presetStopAtRow\", \"wallclock\"]]")
            .count()
            >= 2,
        "the preset modal needs its own paint + click pair: its draft lives in modal state, and clicking the dimmed row is the only way to arm a trigger that already holds a value"
    );
    // The lock must stay a PAINT. `pointerEvents` and `disabled` are used
    // elsewhere in the file (the Start button's GUI lock), so the check is scoped
    // to the two paint helpers — an unscoped grep would either be meaningless or
    // fail on unrelated code.
    for fname in ["function applyStopMode", "function applyPresetStopMode"] {
        // Embedded assets may carry CRLF endings — normalize before hunting for
        // the function's closing brace, or the `\n}` pattern never matches.
        let norm = main_js.replace("\r\n", "\n");
        let start = norm
            .find(fname)
            .unwrap_or_else(|| panic!("{fname} must exist"));
        let body = &norm[start..];
        let end = body.find("\n}\n").expect("a paint helper must be a function");
        let body = &body[..end];
        assert!(
            !body.contains("disabled") && !body.contains("pointerEvents"),
            "{fname} must only toggle the locked class — the lock is painted, never enforced (disabled/pointer-events would dead-end the card)"
        );
    }
    let lock_selector = ".timer-option--locked .timer-input {";
    let lock_start = style_css
        .find(lock_selector)
        .expect("the locked input needs a rule (the dimmed border/colour)");
    let lock_block = &style_css[lock_start..(lock_start + 600).min(style_css.len())];
    assert!(
        !lock_block.contains("not-allowed") && !lock_block.contains("pointer-events"),
        "a locked row must stay typable — it is how you switch triggers back"
    );
    assert!(
        html.contains("id=\"stopAfterRow\"")
            && html.contains("id=\"stopAtRow\"")
            && html.contains("id=\"presetStopAfterRow\"")
            && html.contains("id=\"presetStopAtRow\""),
        "both the dashboard card and the preset modal need the two row anchors"
    );
    assert!(
        defaults_src.contains("stop_mode"),
        "the keep-list repair must migrate the mode instead of dropping it"
    );
}

/// The timer card is the LAST card in the view, so a popover that extends the
/// scroll area used to make the scrollbar appear and push the whole layout
/// sideways — the "interface jumps while I pick a unit" report. The gutter is
/// reserved once, for every view, instead of being fixed per widget.
#[test]
fn test_auto_stop_card_does_not_jump() {
    let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();
    let asset = |name: &str| -> String {
        let key = tauri::utils::assets::AssetKey::from(name);
        let bytes = ctx
            .assets()
            .get(&key)
            .unwrap_or_else(|| panic!("{name} must be embedded"));
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let style_css = asset("style.css");
    assert!(
        style_css.contains("scrollbar-gutter: stable"),
        "the views container must reserve the scrollbar gutter, or opening a popover on the last card shifts the layout"
    );
    assert!(
        !style_css.contains(".unit-toggle-wrapper:hover .unit-popover"),
        "opening the unit popover on hover oscillates: the menu pushes the badge away from the cursor, the cursor leaves, the menu closes — open on click only"
    );
}

/// BIOMETRIC PHASE A: rare hesitation (outlier) + technique badge.
/// The outlier is 3 lines in the REGULAR branch AFTER Bates (never sequence):
/// with probability `outlier_prob` (default 2%, ceiling 10%) one interval
/// stretches to 2-3x base — relative, so it scales from 8 CPS (160-240ms)
/// to 120 CPS (16-25ms) instead of freezing the loop. The badge under CPS
/// shows auto bands (<=15 single, <=25 butterfly, 25+ drag) and a click pins
/// Auto/Single/Butterfly/Drag — display + persistence only, engine stays Single.
#[test]
fn test_biometric_outlier_and_technique_wiring() {
    let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();
    let asset = |name: &str| -> String {
        let key = tauri::utils::assets::AssetKey::from(name);
        let bytes = ctx
            .assets()
            .get(&key)
            .unwrap_or_else(|| panic!("{name} must be embedded"));
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let main_js = asset("main.js");
    let html = asset("index.html");
    let style_css = asset("style.css");
    let sched_src = include_str!("../src/scheduler.rs");
    let cfg_src = include_str!("../src/config.rs");
    let cm_src = include_str!("../src/config_manager.rs");
    let defaults_src = include_str!("../src/defaults/mod.rs");

    // ── 1. Backend: helper after Bates, only in REGULAR branch ──────────
    assert!(
        sched_src.contains("pub(crate) fn outlier_interval_ns"),
        "the hesitation must be a pure helper so rate/shape is unit-testable"
    );
    assert!(
        sched_src.contains("interval_ns = outlier_interval_ns(&mut rng, base_ns, interval_ns, outlier_prob)"),
        "the outlier applies AFTER Bates on the computed interval"
    );
    assert!(
        sched_src.contains("outlier_prob_raw")
            && sched_src.contains("crate::config::normalize_outlier_prob"),
        "the probability travels in an atomic and is normalized on every save"
    );
    assert!(
        cfg_src.contains("pub outlier_prob: f64")
            && cfg_src.contains("pub fn normalize_outlier_prob")
            && cfg_src.contains("pub fn normalize_technique")
            && cfg_src.contains("pub fn resolve_technique"),
        "Config needs the prob field plus normalizers and the band resolver"
    );
    assert!(
        cm_src.contains("pub outlier_prob: f64") && cm_src.contains("pub technique: String"),
        "both engine settings and presets carry hesitation + technique"
    );
    assert!(
        defaults_src.contains("outlier_prob") && defaults_src.contains("technique"),
        "the repair keep-list must migrate both fields instead of dropping them"
    );

    // ── 2. Page: badge under CPS + hesitation sliders, saved on input ───
    // The badge label is OWNED by updateTechniqueBadge() (dynamic: auto bands
    // + pinned mode + current locale) — it must NOT carry a static data-i18n
    // key, or applyTranslations() overwrites the dynamic text and freezes the
    // badge after any locale switch (see the language-changed repaint below).
    for id in ["techniqueBadge", "techniqueMenu", "outlierRange", "outlierInput"] {
        assert!(html.contains(id), "dashboard needs `{id}`");
    }
    assert!(
        !html.contains("id=\"techniqueLabel\" data-i18n"),
        "techniqueLabel is dynamic — a static data-i18n key would freeze it after a locale switch"
    );
    assert!(
        html.contains("data-technique=\"auto\"")
            && html.contains("data-technique=\"single\"")
            && html.contains("data-technique=\"butterfly\"")
            && html.contains("data-technique=\"drag\""),
        "the badge popover must offer Auto/Single/Butterfly/Drag"
    );
    assert!(
        main_js.contains("function updateTechniqueBadge")
            && main_js.contains("function setTechnique")
            && main_js.contains("function resolveTechnique")
            && main_js.contains("updateTechniqueBadge(targetCps)")
            && main_js.contains("updateTechniqueBadge(safeVal)")
            && main_js.contains("currentConfig.engine.technique = norm"),
        "the badge resolves bands, pins on click, follows CPS and persists"
    );
    assert!(
        main_js.contains("nanoclick-language-changed")
            && main_js.contains("languageChanged"),
        "the dynamic badge label must repaint after a locale switch (i18n only repaints static data-i18n nodes)"
    );
    assert!(
        main_js.contains("outlierPctToProb")
            && main_js.contains("currentConfig.engine.outlier_prob = outlierPctToProb"),
        "hesitation is % in the DOM and prob in the config — convert on save"
    );
    assert!(
        style_css.contains(".technique-badge"),
        "the badge needs its own rule (it is not a unit-badge)"
    );

    // ── 3. i18n symmetry (the gate checks 3 locales) ────────────────────
    for locale in ["ua.json", "ru.json", "en.json"] {
        let dict = match locale {
            "ua.json" => include_str!("../../src/locales/ua.json"),
            "ru.json" => include_str!("../../src/locales/ru.json"),
            _ => include_str!("../../src/locales/en.json"),
        };
        for key in [
            "dash_outlier_prob",
            "technique_single",
            "technique_butterfly",
            "technique_drag",
            "technique_auto",
        ] {
            assert!(dict.contains(key), "{locale} must translate {key}");
        }
    }
}
