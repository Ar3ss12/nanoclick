# ⚡ NanoClick

**A fast, modern Windows desktop automation tool** — autoclicker, macro recorder, and visual macro editor in one lightweight app.

Built with **Tauri 2 + Rust + vanilla JS**. No Electron, no bundler, no bloat: the production installer is **~3.4 MB**.

![Platform](https://img.shields.io/badge/platform-Windows%2010%2F11-blue) ![Tests](https://img.shields.io/badge/tests-66%2F66-brightgreen) ![Tauri](https://img.shields.io/badge/Tauri-2.x-FFC131) ![Rust](https://img.shields.io/badge/rust-stable--msvc-DEA584)

---

## ✨ Features

### 🖱️ Click Engine
- **Single / Double / Hold** click modes with configurable press & pause durations
- **Human jitter** — randomize timing (±0–30%) *and* cursor position (±0–30 px) so clicks look natural
- **Position picker** — bind clicks to a fixed screen point or follow the cursor
- **Precise CPS control** (0.1–100+) with live hotkey speed adjustment

### 🎬 Macro Recorder & Visual Editor
- **Two ways to create a macro:** 🔴 record real input, or ＋ build it from blocks manually
- **Smart Normalizer** (default-on) — collapses thousands of raw mouse-move events into clean actions
- **⚡ Optimize** — one-click cleanup at three aggressiveness levels (Subtle / Balanced / Aggressive)
- **Visual editor** — inline edit, rename, drag-to-reorder, context menu (Run from here / Step / Disable / Duplicate)

### 🧠 Control Flow (v4.0)
- `Repeat`, `If/Else` with pixel-color conditions, variables (`SetVar`/`GetVar`), and nested macro calls
- Presets: save, import/export, and migrate your full engine configuration

### 🛡️ Safety & Convenience
- **Work Mode** — suspends global hotkeys while you're using other apps
- **Auto-pause on navigation**, emergency stop (`Escape`), start-delay & auto-stop timers
- **Windows autostart + system tray**
- 🎨 **6 themes**: Dark Cyberpunk, Neon Grass, Dark Slate, Midnight Blue, Dracula Crimson, Amethyst Purple

---

## ⌨️ Default Hotkeys

| Action | Keys |
|---|---|
| Start / Stop autoclicker | <kbd>R</kbd> / <kbd>K</kbd> / <kbd>F6</kbd> |
| Toggle Autoclicker ↔ Work Mode | <kbd>Ctrl+Alt+M</kbd> |
| Emergency stop | <kbd>Escape</kbd> |
| Speed up (+1 CPS) | <kbd>Ctrl+=</kbd> |
| Slow down (−1 CPS) | <kbd>Ctrl+−</kbd> |
| Capture screen coordinates | <kbd>Ctrl+P</kbd> |
| Start / Stop macro recording | <kbd>Ctrl+Shift+R</kbd> |

All hotkeys are handled by an event-driven `WH_KEYBOARD_LL` listener in Rust — zero busy-polling at idle, sub-millisecond reaction time.

---

## 📦 Install

Download the latest installer from [Releases](../../releases):

```
NanoClick_1.0.0_x64-setup.exe   (~3.4 MB)
```

- Installs per-user (no admin rights needed)
- Uses the system WebView2 runtime; downloads it automatically if missing
- Updates are delivered through the built-in updater (signed artifacts)

## 🛠️ Build from Source

**Prerequisites:** [Rust](https://rustup.rs) (stable-msvc), Microsoft C++ Build Tools, WebView2 (preinstalled on Windows 10/11).

```bash
# 1. Clone
git clone <repo-url> nanoclick
cd nanoclick

# 2. Run in dev mode
cargo tauri dev

# 3. Or build the release installer (NSIS)
cargo tauri build --bundles nsis
# → target/release/bundle/nsis/NanoClick_*-setup.exe
```

### Run tests

```bash
cd src-tauri
cargo test --release
# → 66 passed; 0 failed
```

The suite includes physical integration tests that inject real keyboard events via `SendInput` and verify them against a low-level hook.

## 🏗️ Architecture

```
┌────────────────────────────  Frontend (vanilla JS) ───────────────────────────┐
│  UI · presets · visual macro builder · IPC via window.__TAURI__.core.invoke  │
└──────────────────────────────────────┬───────────────────────────────────────┘
                                       │ Tauri commands (12)
┌──────────────────────────────────────▼───────────────────────────────────────┐
│                              Rust backend                                     │
│  ┌───────────┐  ┌────────────┐  ┌─────────────┐  ┌────────────────────────┐  │
│  │ Scheduler │  │  Recorder  │  │ Persistence │  │ Platform (windows/)    │  │
│  │ CPS loop, │  │ WH_MOUSE/  │  │ config.json │  │ SendInput injection,   │  │
│  │ timers,   │→ │ KEYBOARD_LL│  │ macros.json │  │ WH_KEYBOARD_LL hotkeys,│  │
│  │ control   │  │ normalizer │  │ presets     │  │ single-instance lock   │  │
│  └───────────┘  └────────────┘  └─────────────┘  └────────────────────────┘  │
└──────────────────────────────────────────────────────────────────────────────┘
```

- **All timing runs on a Rust worker thread** — UI never drives the click loop
- **Global hotkeys** use an event-driven channel (hook → mpsc → matcher), not polling
- **Updater**: artifacts signed with a minisign keypair; verification is mandatory and built into Tauri's updater plugin

## 📁 Project Structure

```
nanoclick/
├── src/                  # Frontend: index.html, main.js, style.css (no bundler)
├── src-tauri/
│   ├── src/
│   │   ├── core/         # Action engine, executor, conditions
│   │   ├── recorder/     # Raw event capture, Smart Normalizer, optimizer
│   │   ├── scheduler.rs  # Click loop, CPS, timers, stop events
│   │   ├── persistence/  # Config, macros, presets (+ migrations)
│   │   ├── platform/     # windows/ (hooks, input) + backend traits
│   │   └── lib.rs        # Tauri commands, setup, logging
│   ├── capabilities/     # Tauri 2 permission manifests
│   └── tauri.conf.json
└── scripts/              # sign-windows.ps1 (code signing, cert required)
```

## 🔒 Signing & Updates

NanoClick uses two independent signing systems:

| System | Purpose | Status |
|---|---|---|
| **Tauri updater signing** (minisign keypair, free) | Proves update artifacts are authentic | ✅ Active — `.sig` generated per build |
| **Windows code signing** (certificate, paid) | Removes SmartScreen warnings | ⏳ Awaiting certificate |

Build the signed installer locally:

```bash
TAURI_SIGNING_PRIVATE_KEY="C:/Users/<you>/.tauri/nanoclick.key" \
cargo tauri build --bundles nsis
```

> ⚠️ Never lose the private updater key — installations already shipped trust updates only for the key they were built with.

## 📄 License

All rights reserved. © 2026 NanoClick Project

---

<p align="center">Built with ❤️ using <a href="https://tauri.app">Tauri</a></p>
