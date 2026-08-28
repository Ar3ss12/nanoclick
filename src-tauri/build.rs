fn main() {
    // Force rebuild when frontend files change — Tauri embeds these at compile time
    // via include_dir!/tauri::generate_context!, so we have to invalidate the build.
    println!("cargo:rerun-if-changed=../src/index.html");
    println!("cargo:rerun-if-changed=../src/main.js");
    println!("cargo:rerun-if-changed=../src/style.css");
    println!("cargo:rerun-if-changed=../src/sequence_editor.js");
    println!("cargo:rerun-if-changed=../src/hud.html");
    println!("cargo:rerun-if-changed=../src/hud.js");
    println!("cargo:rerun-if-changed=tauri.conf.json");
    tauri_build::build();
}

// touch
