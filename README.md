# ⚡ NanoClick

<p align="center">
  <b>The lightweight native auto clicker & macro editor for Windows.</b><br>
  Human-like clicks. Zero lag. No Electron. Pure Rust speed.
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Platform-Windows%2010%20%2F%2011-0078D6?style=flat-square&logo=windows&logoColor=white" alt="Platform">
  <img src="https://img.shields.io/badge/RAM_in_tray-5.3_MB-brightgreen?style=flat-square" alt="RAM">
  <img src="https://img.shields.io/badge/Installer-%7E3.4_MB-orange?style=flat-square" alt="Size">
  <a href="https://github.com/Ar3ss12/nanoclick/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/Ar3ss12/nanoclick/ci.yml?branch=main&style=flat-square&label=CI" alt="CI"></a>
</p>

<p align="center">
  <a href="https://github.com/Ar3ss12/nanoclick/releases/latest">
    <img src="https://img.shields.io/github/v/release/Ar3ss12/nanoclick?style=for-the-badge&label=%E2%AC%87%EF%B8%8F_Download&color=2ea44f" height="38" alt="Download latest release">
  </a>
  &nbsp;
  <a href="https://github.com/Ar3ss12/nanoclick/releases/latest">
    <img src="https://img.shields.io/badge/PORTABLE_%7C_9.7_MB-gray?style=for-the-badge" height="38" alt="Portable build">
  </a>
</p>

<p align="center">
  <img src=".github/assets/main_preview.png" alt="NanoClick Dashboard Preview" width="800">
</p>

---

## ⚡ Quick Start (10 Seconds)

1. **[Download](https://github.com/Ar3ss12/nanoclick/releases/latest)** the installer (`.exe`) or grab the portable version (zero install).
2. **Press <kbd>F6</kbd> or <kbd>R</kbd>** — clicking starts immediately.
3. **Press <kbd>Escape</kbd>** — instant emergency stop anytime.

---

## 🥊 Why NanoClick?

| Typical clickers from the web | NanoClick |
|---|---|
| ❌ Bloated Electron apps eating **150+ MB RAM** | ✅ **Only 5.3 MB RAM in tray** — pure native Rust |
| ❌ Crashes leave your mouse **stuck clicking** | ✅ **Watchdog outside the process** — the mouse always releases, even on crash |
| ❌ Robotic metronome-flat timing | ✅ **Human-like jitter & biomechanics** — and the timing math costs nanoseconds, so your FPS never feels it |

---

## ✨ Features

<table>
<tr><th width="50%" valign="top">🖱️ Click Engine</th><th width="50%" valign="top">🎬 Visual Macro Builder</th></tr>
<tr><td valign="top">

- **Modes** — Single, Double, Hold, Butterfly, Drag (0.1–160 CPS)
- **Human-like timing** — natural jitter, hold times and spatial variance modeled on real hand biomechanics
- **Zero lag** — the click math takes nanoseconds per click; your game never stutters
- **Targeting** — click at the cursor position or lock to fixed screen coordinates
- **Clean UI** — CPS/ms switcher, floating HUD, 6 themes

</td><td valign="top">

- **Record or create** — capture live input or build macros from blocks
- **Smart compression** — recorded drag paths shrink by **80–95 %** for smooth playback
- **Logic blocks** — `Repeat`, `If/Else` with pixel-color conditions, variables, nested macro calls
- **Instant presets** — switch full engine profiles on the fly with hotkeys
- **Stats** — live telemetry and persistent click history

</td></tr>
</table>

### 🛡️ Built Like a Tank

* **Never gets stuck:** if the app freezes, crashes, or is killed in Task Manager — an independent watchdog outside the process instantly releases all mouse buttons.
* **Typing Guard:** automatically pauses clicking while you type real text, so your chat never gets ruined.
* **FocusGuard:** halts clicking within ~50 ms when you Alt+Tab or switch windows.
* **Immortal daemon:** the click engine runs in a dedicated background process — closing or killing the UI window never stops your clicks.

---

## ⌨️ Default Hotkeys

| Action | Hotkey |
|---|---|
| **Start / Stop Clicker** | <kbd>F6</kbd> / <kbd>R</kbd> / <kbd>K</kbd> |
| **Emergency Stop** | <kbd>Escape</kbd> |
| **Speed Up / Down (±1 CPS)** | <kbd>Ctrl</kbd>+<kbd>=</kbd> / <kbd>Ctrl</kbd>+<kbd>−</kbd> |
| **Pick Screen Coordinates** | <kbd>Ctrl</kbd>+<kbd>P</kbd> |
| **Record Macro (Start / Stop)** | <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>R</kbd> |
| **Toggle Work Mode** | <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>M</kbd> |

---

<details>
<summary><b>🛠️ Developer & Build Details (click to expand)</b></summary>

### System requirements

* Windows 10 / 11 (64-bit)
* WebView2 runtime (preinstalled on Windows 10/11; downloaded automatically if missing)

### Build from source

```bash
git clone https://github.com/Ar3ss12/nanoclick.git
cd nanoclick
cargo tauri dev                    # run in development
cargo tauri build --bundles nsis   # build the release installer
```

*Prerequisites: [Rust](https://rustup.rs) (stable-msvc), Microsoft C++ Build Tools.*

Full architecture, test commands and the release/signing flow:
[CONTRIBUTING.md](CONTRIBUTING.md) · What's new: [CHANGELOG.md](CHANGELOG.md)

</details>

---

## 📄 License

Free for personal and educational use under the **PolyForm Noncommercial
License 1.0.0** ([LICENSE.md](LICENSE.md)).
*Commercial operations and selling binaries require a commercial license.*

---

## 🌟 Star History

<p align="center">
  <a href="https://star-history.com/#Ar3ss12/nanoclick&Date">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/svg?repos=Ar3ss12/nanoclick&type=Date&theme=dark" />
      <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/svg?repos=Ar3ss12/nanoclick&type=Date" />
      <img alt="NanoClick Star History Chart" src="https://api.star-history.com/svg?repos=Ar3ss12/nanoclick&type=Date" width="750" />
    </picture>
  </a>
</p>

<p align="center">
  <sub>© 2026 Ar3ss12 · Built with native Rust & <a href="https://tauri.app">Tauri</a></sub>
</p>
