# ⚡ NanoClick

**A fast, modern Windows desktop automation tool** — autoclicker, macro recorder, and visual macro editor in one lightweight app.

Built with **Tauri 2 + Rust + vanilla JS**. No Electron, no bundler, no bloat: the production installer is **~3.5 MB**.

![Platform](https://img.shields.io/badge/platform-Windows%2010%2F11-blue) ![Tests](https://img.shields.io/badge/tests-126%2F126-brightgreen) ![i18n](https://img.shields.io/badge/i18n-UA%20%7C%20RU%20%7C%20EN-blue) ![Tauri](https://img.shields.io/badge/Tauri-2.x-FFC131) ![Rust](https://img.shields.io/badge/rust-stable--msvc-DEA584)

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

### 🌍 100% Tri-Lingual Internationalization (i18n)
- **Comprehensive coverage** across 🇺🇦 Ukrainian, 🇷🇺 Russian, and 🇬🇧 English (337 symmetric keys).
- **Zero-overhead memory footprint** — lazy loads only the chosen JSON locale on demand (~12 KB).
- **Instant reactive updates** — all static UI and dynamic runtime strings adapt immediately without restart.

### 🛡️ Windows UIPI Security & Admin Elevation
- **Elevated Window Detection** — detects when hovering over elevated processes (Task Manager, admin consoles, protected games) to prevent dropped clicks.
- **Audio & Visual Alerts** — synthesized chime warning and informative dialogue when Windows UIPI blocks `SendInput`.
- **One-Click Admin Restart** — instant UAC prompt (`runas`) and Windows Compatibility Layer registry persistence (`~ RUNASADMIN`).

### 🖥️ Per-Monitor V2 DPI Scaling & Manifest Stability
- **PerMonitorV2 awareness** — zero coordinate drift across mixed DPI multi-monitor configurations.
- **Full Side-by-Side Windows Manifest** — integrates `Microsoft.Windows.Common-Controls` v6.0.0.0 and OS compatibility tags.

### 🛡️ Safety & Convenience
- **Smart Typing Guard & Kill-Switch** — auto-stops clicking when typing real text and locks out hotkeys for a configurable window (100–5000ms), while gamer keys (WASD, Space, QERF) are preserved
- **Zero-Latency Stop & Fast Double-Tap** — stops clicking instantly on keypress (0ms) without debounce blocking
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

Download the first `v1.0.0-beta` installer from [Releases](../../releases/tag/v1.0.0-beta):

```
NanoClick_1.0.0-beta_x64-setup.exe   (~3.5 MB)
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
cargo test -- --skip physical_
# → 126 passed; 0 failed (119 unit + 7 integration)
```

The test suite covers:
- **i18n Key Symmetry & DOM Validation** — 100% 3-way synchronization across UA, RU, and EN (337 keys)
- **Windows UIPI & Elevation Integration** — token privilege checks and app manifests
- **Stats Triple-Redundancy** — `config.json` + `stats.json` + `localStorage` fallback
- **Modal Scrollability & Trap Immunity** — Escape handlers and backdrop closes across all overlays
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
- **25 FPS (40 ms) IPC Update Throttling** — click counter & status telemetry are throttled to 25 FPS in Rust, keeping IPC overhead negligible even during 100+ CPS clicking
- **Low-Memory Chromium Profile** — enforced `--js-flags="--max-old-space-size=64"` and `--in-process-gpu` reduce RAM usage by 2–2.5x (~120–140 MB) while eliminating startup CPU spikes
- **Global hotkeys** use an event-driven channel (hook → mpsc → matcher), not polling
- **Zero-Lock Hooks** — `thread_local!` state avoids mutex contention on high-frequency input
- **Updater**: artifacts signed with a minisign keypair; verification is mandatory and built into Tauri's updater plugin

---

## 📁 Project Structure

```
nanoclick/
├── src/                  # Frontend (no bundler):
│   ├── locales/          #   i18n JSON dictionaries: ua.json, ru.json, en.json (337 keys)
│   ├── i18n.js           #   Zero-dependency lightweight lazy-loading i18n engine
│   ├── stats.js          #   Modular analytics & live Canvas CPS chart engine
│   ├── uipi_manager.js   #   Windows UIPI detection, audio chime & elevation UI
│   ├── main.js           #   Core UI logic, state bindings, presets & visual editor
│   ├── index.html        #   Main application structure & data-i18n bindings
│   └── style.css         #   Cyberpunk/Neon themes, modal layouts & responsive styles
├── src-tauri/
│   ├── src/
│   │   ├── core/         # Action engine, executor, conditions
│   │   ├── recorder/     # Raw event capture, Smart Normalizer (RDP), optimizer
│   │   ├── scheduler.rs  # Click loop, CPS, timers, stop events
│   │   ├── persistence/  # Config, macros, presets (+ migrations, stats)
│   │   ├── platform/     # windows/ (hooks, input, uipi.rs) + backend traits
│   │   └── lib.rs        # Tauri commands, setup, logging, DPI initialization
│   ├── build.rs          # Windows application manifest (ComCtl32 v6.0 + PerMonitorV2)
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

Build a signed installer locally (private key stays outside the repo):

```powershell
# Private updater key is stored OUTSIDE the repository, e.g. $env:USERPROFILE\.tauri\
# and provided via environment variables at build time. Never commit it.
$env:TAURI_SIGNING_PRIVATE_KEY = (Get-Content $env:TAURI_SIGNING_PRIVATE_KEY_PATH -Raw).Trim()
cargo tauri build --bundles nsis
```

> ⚠️ Keep the private updater key safe — installations already shipped only trust
> updates signed with the same key they were built with.

---

## 📄 License

NanoClick is licensed under the **PolyForm Noncommercial License 1.0.0** plus the
NanoClick Additional Terms (branding policy, permitted gaming/streaming uses,
commercial-use examples, governing law). See [`LICENSE.md`](LICENSE.md) for the
full text — the license file is the single source of truth and is bundled with
each release.

Short version:

- ✅ Free for personal, noncommercial use — including gaming, community
  tournaments, streams and monetized videos (as personal use of the software).
- ✅ Hobby, research and study use.
- ❌ No selling of binaries, no adware/miner bundling, no paid bot/farm service
  without a separate commercial license.
- 🔁 Forks and redistributions must rebrand (remove the NanoClick name/logo).

© 2026 Ar3ss12 · All rights not expressly granted are reserved.

---

<p align="center">Built with ❤️ using <a href="https://tauri.app">Tauri</a></p>
