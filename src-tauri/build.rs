fn main() {
    // Force rebuild when frontend files change — Tauri embeds these at compile time
    // via include_dir!/tauri::generate_context!, so we have to invalidate the build.
    //
    // IMPORTANT: also rerun-if-changed the whole frontend directory. On Windows
    // mtime on a directory does NOT update when files inside change, so without
    // this Tauri misses `index.html` edits and ships stale HTML/JS.
    println!("cargo:rerun-if-changed=../src/index.html");
    println!("cargo:rerun-if-changed=../src/main.js");
    println!("cargo:rerun-if-changed=../src/stats.js");
    println!("cargo:rerun-if-changed=../src/uipi_manager.js");
    println!("cargo:rerun-if-changed=../src/i18n.js");
    println!("cargo:rerun-if-changed=../src/locales/ua.json");
    println!("cargo:rerun-if-changed=../src/locales/ru.json");
    println!("cargo:rerun-if-changed=../src/locales/en.json");
    println!("cargo:rerun-if-changed=../src/style.css");
    println!("cargo:rerun-if-changed=../src/sequence_editor.js");
    println!("cargo:rerun-if-changed=../src/hud.html");
    println!("cargo:rerun-if-changed=../src/hud.js");
    println!("cargo:rerun-if-changed=tauri.conf.json");

    let manifest = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0" xmlns:asmv3="urn:schemas-microsoft-com:asm.v3">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <!-- Windows 10 and Windows 11 -->
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
      <!-- Windows 8.1 -->
      <supportedOS Id="{1f676c76-80e1-4239-95bb-83d0f6d0da78}"/>
      <!-- Windows 8 -->
      <supportedOS Id="{4a2f28e3-53b9-4441-ba9c-d69d4a4a6e38}"/>
      <!-- Windows 7 -->
      <supportedOS Id="{35138b9a-5d96-4fbd-8e2d-a2440225f93a}"/>
    </application>
  </compatibility>
  <asmv3:application>
    <asmv3:windowsSettings>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2, PerMonitor</dpiAwareness>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/PM</dpiAware>
    </asmv3:windowsSettings>
  </asmv3:application>
</assembly>
"#;

    let windows = tauri_build::WindowsAttributes::new().app_manifest(manifest);
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("failed to run tauri-build");
}

// touch
