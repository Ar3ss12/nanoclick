use std::sync::Arc;

#[test]
fn test_assets() {
    let ctx: tauri::Context<tauri::Wry> = tauri::generate_context!();
    let key = tauri::utils::assets::AssetKey::from("index.html");
    if let Some(bytes) = ctx.assets().get(&key) {
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(120)]);
        println!("OK len={} head={:?}", bytes.len(), head);
        let full = String::from_utf8_lossy(&bytes);
        assert!(
            full.contains("presetEditModal") || full.contains("prPointsCanvas"),
            "embedded index.html is missing sequence-editor markers — got {} bytes",
            bytes.len()
        );
    } else {
        panic!("assets.get(\"index.html\") returned None — frontend not embedded");
    }
}
