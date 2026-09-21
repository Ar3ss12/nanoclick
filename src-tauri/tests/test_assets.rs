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
    assert!(sched_src.contains("focus_guard_arc.disarm();"), "run end / set_config must disarm the session");
    assert!(sched_src.contains("focus-loss-paused"), "pause must notify the UI");

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


