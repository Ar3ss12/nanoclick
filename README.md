# ⚡ NanoClick

**A fast, modern Windows desktop automation tool** — autoclicker, macro recorder, and visual macro editor in one lightweight app.

Built with **Tauri 2 + Rust + vanilla JS**. No Electron, no bundler, no bloat: the production installer is **~3.5 MB**.

![Platform](https://img.shields.io/badge/platform-Windows%2010%2F11-blue) ![Tests](https://img.shields.io/badge/tests-96%2F96-brightgreen) ![Tauri](https://img.shields.io/badge/Tauri-2.x-FFC131) ![Rust](https://img.shields.io/badge/rust-stable--msvc-DEA584)

---

## ✨ Features

### 🖱️ Click Engine
- **Single / Double / Hold** click modes with configurable press & pause durations
- **Human jitter** — randomize timing (±0–30%) *and* cursor position (±0–30 px) so clicks look natural
- **Position picker** — bind clicks to a fixed screen point or follow the cursor
- **Precise CPS control** (0.1–100+) with live hotkey speed adjustment and real-time performance telemetry

### 🎬 Smart Macro Recorder & Visual Editor
- **Two ways to create a macro:** 🔴 record real input in real-time, or ＋ build it from blocks manually
- **Ramer-Douglas-Peucker (RDP) Trajectory Simplification** — advanced curve reduction algorithm that compresses recorded drag mouse paths by 80–95% while keeping exact curvature
- **Smart Hover Path Elimination** — strips redundant intermediate hover movements, keeping only the precise target position before clicks/key actions
- **⚡ Optimize** — one-click macro cleanup at three aggressiveness levels (Subtle / Balanced / Aggressive)
- **Visual editor** — inline edit, rename, drag-to-reorder, context menu (Run from here / Step / Disable / Duplicate)

### 🧠 Control Flow & Smart Hotkeys
- **Zero-Lock Win32 Hooks** — thread-local event listeners for sub-millisecond hotkey response with zero interface stuttering
- **Smart Key Memory** — TTL-based keypress memory (configurable 100–3000ms) for effortless recording of complex hotkeys and modifier combinations (Ctrl, Alt, Shift, Win)
- **Multi-point Sequence Editor** — high-performance Canvas editor with O(1) transform caching and snap-to-grid
- **Advanced Automation Primitives** — `Repeat`, `If/Else` with pixel-color conditions, variables (`SetVar`/`GetVar`), and nested macro calls
- **Presets & Statistics** — save, import/export full engine presets and persist total click analytics across application updates

### 🛡️ Safety & Convenience
- **Work Mode** — suspends global hotkeys while you're using other applications
- **Auto-pause on navigation**, emergency stop (<kbd>Escape</kbd>), start-delay & auto-stop timers
- **Floating HUD** overlay for real-time click tracking
- **Windows autostart + system tray** integration
- 🎨 **6 Themes**: Dark Cyberpunk, Neon Grass, Dark Slate, Midnight Blue, Dracula Crimson, Amethyst Purple

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

Download the latest release installer from [Releases](../../releases/tag/v1.2.1):

```
NanoClick_1.2.1_x64-setup.exe   (~3.5 MB)
```

- Installs per-user (no admin rights needed)
- Uses the system WebView2 runtime; downloads it automatically if missing
- Updates are delivered through the built-in updater (signed artifacts)

---

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
cargo test --lib
# → 96 passed; 0 failed
```

The test suite covers:
- **Win32 Hook routing** & physical input matching (`SendInput`, Numpad, Mouse X-Buttons)
- **Normalizer 5-phase pipeline** & RDP mouse trajectory simplification
- **Recorder handle idempotency** & thread safety
- **Hotkey debouncing** & config persistence

---

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
- **Zero-Lock Hooks** — `thread_local!` state avoids mutex contention on high-frequency input
- **Updater**: artifacts signed with a minisign keypair; verification is mandatory and built into Tauri's updater plugin

---

## 📁 Project Structure

```
nanoclick/
├── src/                  # Frontend: index.html, main.js, style.css (no bundler)
├── src-tauri/
│   ├── src/
│   │   ├── core/         # Action engine, executor, conditions
│   │   ├── recorder/     # Raw event capture, Smart Normalizer (RDP), optimizer
│   │   ├── scheduler.rs  # Click loop, CPS, timers, stop events
│   │   ├── persistence/  # Config, macros, presets (+ migrations, stats)
│   │   ├── platform/     # windows/ (hooks, input) + backend traits
│   │   └── lib.rs        # Tauri commands, setup, logging
│   ├── capabilities/     # Tauri 2 permission manifests
│   └── tauri.conf.json
└── scripts/              # upload_release.ps1 (automated release publishing)
```

---

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

---

## 📄 License

All rights reserved. © 2026 NanoClick Project

---

<p align="center">Built with ❤️ using <a href="https://tauri.app">Tauri</a></p>
