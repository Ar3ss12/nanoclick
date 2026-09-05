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
