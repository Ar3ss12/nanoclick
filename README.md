# ⚡ NanoClick

**A Windows desktop automation tool** — autoclicker, macro recorder, and visual
macro editor in one lightweight native app. Tauri 2 + Rust + vanilla JS: no
Electron, no bundler, no bloat — the production installer is **~3.4 MB**, the
portable binary **~9.7 MB** (measured 2026-10-28).

![Platform](https://img.shields.io/badge/platform-Windows%2010%2F11-blue)
[![CI](https://github.com/Ar3ss12/nanoclick/actions/workflows/ci.yml/badge.svg)](https://github.com/Ar3ss12/nanoclick/actions/workflows/ci.yml)

---

<p align="center">
  <img src=".github/assets/main_preview.png" alt="NanoClick Dashboard Preview" width="800">
</p>

---

## 📦 Install

Download `v1.3.0` from [Releases](https://github.com/Ar3ss12/nanoclick/releases/tag/v1.3.0):

```
NanoClick_1.3.0_x64-setup.exe   (~3.4 MB)
NanoClick-portable.exe          (~9.7 MB, zero-install)
```

- Installs per-user (no admin rights needed)
- Uses the system WebView2 runtime; downloads it automatically if missing
- Updates are delivered through the built-in updater (signed artifacts)

What's new: [CHANGELOG.md](CHANGELOG.md).

---

## ✨ What it does

### 🖱️ Click engine
- **Every click mode** — Single / Double / Hold / Butterfly / Drag, 0.1–160 CPS, hotkey speed adjustment on the fly, live telemetry
- **Your FPS never feels it** — the per-click timing math costs **nanoseconds** (measured 35–223 ns against a 6.25 ms interval at 160 CPS): no background lag, no stutter, even in heavy games
- **Human, not robotic** — every click carries Bates-model jitter (±0–35 %), spatial variance (±0–50 px), hold times and a fatigue tail, so the timing curve reads like a hand, not a metronome
- **Click anywhere** — bind to a fixed point or follow the cursor; CPS/ms switcher right on the dashboard

### 🎬 Macros
- **Record or build** — capture real input in real time, or assemble macros from blocks in the visual editor (inline edit, rename, drag-to-reorder, run-from-here, step, disable, duplicate)
- **Smart recording** — recorded drag paths are compressed by **80–95 %** (Ramer–Douglas–Peucker) while the trajectory stays smooth; redundant hover moves are stripped automatically, so a chaotic mouse swipe becomes a clean macro
- **Logic included** — `Repeat`, `If/Else` with pixel-color conditions, variables (`SetVar`/`GetVar`), nested macro calls
- **Presets** — instant hotkey switching between full engine profiles; import/export; click statistics persist across updates

### 🛡️ Safety & reliability
- **No stuck mouse — ever** — if the app freezes, crashes or is killed in Task Manager, an independent watchdog *outside* the process instantly releases the buttons (verified end-to-end: the OS accepts all three release paths). Even `taskkill /F /T` cannot reach it
- **Honest status, not a lying `RUNNING`** — a 200 ms maintenance thread watches the click worker; if anything wedges, Start is refused and the app names the reason (`ENGINE_STUCK`, `RELEASE_PENDING`, `MEMORY_HARD`)
- **Survives the window being killed** — the engine lives in a separate daemon process; close or kill the UI and clicking never flinches — proven by chaos tests
- **Stops when you type** — Smart Typing Guard (default ON) halts clicking and locks hotkeys while you write real text; 2-second cooldown before re-arming
- **Stops when you leave the window** — FocusGuard (opt-in) halts within **~50 ms** of a foreground change and names the process that stole focus
- **Config that heals itself** — Last-Known-Good snapshots, quarantine of corrupt files, mandatory Deadbolt modal, factory reset with a 5-second cooldown
- **Zero-latency stop** — <kbd>Escape</kbd> and hotkey stop land at 0 ms

### 🖥️ Windows integration
- **Event-driven hotkeys** (`WH_KEYBOARD_LL` / `WH_MOUSE_LL`) — zero busy-polling, sub-millisecond reaction; Mouse 4/5 work on gaming mice up to 8000 Hz
- **Invisible in the background** — close to tray and the engine keeps clicking; deep sleep drops RAM to **5.3 MB** (measured `scripts/measure-ram.ps1`, from 117–123 MB) while hooks, scheduler and tray stay alive
- **Elevated windows handled** — detects admin windows (Task Manager, admin consoles), warns with a chime + dialog, one-click restart as admin
- **Multi-monitor DPI done right** — PerMonitorV2: zero coordinate drift across mixed-DPI screens
- **6 themes**, floating HUD, autostart, UA + EN interface

---

## 🥊 Why NanoClick

| Tool | What it is | Size & idle RAM | If it crashes | Click realism | Source & trust |
|---|---|---|---|---|---|
| Typical "clicker suite" from download sites | closed `.exe` of unclear origin | Electron UIs eat **150+ MB RAM**; bundles of unknown content | the click loop can stick — nothing releases it | fixed interval, metronome-flat | unsigned, adware-bundling risk |
| [OP Auto Clicker](https://www.opautoclicker.com/) | dedicated single-point clicker | **512 KB** portable | nothing releases a stuck loop | fixed interval, no humanization | closed-source freeware |
| [GS Auto Clicker](https://www.opauto-clicker.com/op-auto-clicker-vs-gs-auto-clicker) | one-point repeated clicking with a hotkey | lightweight | nothing releases a stuck loop | fixed interval | closed-source freeware |
| [TinyTask](https://tinytask.net/) | macro **recorder** — record & replay, no editing | **36 KB** | playback can wedge mid-run | replays exactly what you did — jitter included, randomness not | closed-source freeware |
| [AutoHotkey](https://www.autohotkey.com/) | scripting language — you write the automation yourself | small runtime, no GUI | whatever your script does | DIY math | open source |
| [AlphaClicker](https://thetinytask.com/best-auto-clicker-windows/) | open-source clicker | lightweight | nothing releases a stuck loop | fixed interval | MIT open source |
| **NanoClick** | **open-source app with a full GUI: clicker + recorder + visual editor** | **9.7 MB portable, 5.3 MB in tray** (measured) | **out-of-process guardian releases the mouse** (E2E-proven) | **Bates-model jitter + spatial variance; ns-level pacing measured** | **open source (PolyForm), signed updater** |

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

## 🚫 Not for

- **macOS / Linux** — the engine is Win32 (`SendInput`, `WH_*_LL` hooks); there is no cross-platform backend.
- **Commercial use** — PolyForm Noncommercial (see [License](#-license)); selling binaries or operating paid automation services requires a separate license.
- **Forcing clicks into elevated windows** — UIPI blocks are detected and reported, never bypassed.

---

## 🛠️ Build from Source

**Prerequisites:** [Rust](https://rustup.rs) (stable-msvc), Microsoft C++ Build Tools, WebView2 (preinstalled on Windows 10/11).

```bash
# 1. Clone
git clone https://github.com/Ar3ss12/nanoclick.git
cd nanoclick

# 2. Run in dev mode
cargo tauri dev

# 3. Or build the release installer (NSIS)
cargo tauri build --bundles nsis
# → target/release/bundle/nsis/NanoClick_*-setup.exe
```

Before every release, strict gates run: browser-accurate JS syntax check,
oxlint and boot-guard wiring — no bundler means one dead `main.js` module
would otherwise ship silently. The maintainer kitchen — version bumps, the
full gate table, test commands and the release/signing flow — lives in
[CONTRIBUTING.md](CONTRIBUTING.md).

### Run tests

```bash
cd src-tauri
cargo test --release --locked -j 2 --lib -- --skip physical_
# physical_ lanes are hardware-timing tests: run them separately
cargo test --release --locked -j 2 --lib -- physical

cargo test --release --locked -j 2 --tests
# integration suites: asset tripwires, daemon handshake (+ chaos lane), guardian e2e
```

CI ([.github/workflows/ci.yml](.github/workflows/ci.yml)) runs the full matrix on `windows-latest`: JS gates → `panic = "abort"` compile gate → lib test lanes → clippy `-D warnings` → integration suites. All lanes `--locked -j 2`.

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

- **All timing runs on a Rust worker thread** — the UI never drives the click loop; 66 ms (15 FPS) IPC telemetry from lock-free atomics keeps overhead negligible even at 160 CPS
- **Lazy secondary WebViews** — cold boot creates only the main window; the overlay and HUD are built on demand and fully `destroy()`ed when toggled off — idle RAM holds zero secondary WebViews
- **Exactly-once stats finalize** — the `active→idle` transition is the single entry point per run; junk runs (`<5 clicks` and `<1s`) grow the counters but never pollute the history chart; disk writes happen only when clicks happened
- **In-process GPU mode is deliberately off** — merging Chromium's GPU into our process would hang the click/hook thread on a DirectX reset; RAM is saved by lazy windows instead
- **Updater**: artifacts signed with a minisign keypair; verification is mandatory and built into Tauri's updater plugin

---

## 📁 Project Structure

```
nanoclick/
├── src/                  # Frontend (no bundler):
│   ├── locales/          #   i18n JSON dictionaries: ua.json, en.json
│   ├── boot_guard.js     #   CLASSIC-script crash trap + boot watchdog (must load first)
│   ├── i18n.js           #   Zero-dependency lazy-loading i18n engine
│   ├── stats.js          #   Modular analytics & live Canvas CPS chart engine
│   ├── uipi_manager.js   #   Windows UIPI detection, audio chime & elevation UI
│   ├── main.js           #   Core UI logic, state bindings, presets & visual editor
│   └── index.html / style.css
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
└── scripts/              # check-js / release / measure-ram / set-version (PowerShell)
```

---

## 🔒 Signing & Updates

Updates are proven authentic, not just downloaded: every artifact carries a
minisign signature (`.sig` per build), verification is mandatory and built
into Tauri's updater plugin, and portable updates swap the exe in place only
after re-verification. A paid Windows certificate (removing SmartScreen
warnings) is pending — see [CONTRIBUTING.md](CONTRIBUTING.md) for the signing
and release flow.

---

## 📄 License

NanoClick is licensed under the **PolyForm Noncommercial License 1.0.0** plus the
NanoClick Additional Terms (branding policy, permitted gaming/streaming uses,
commercial-use examples, governing law). See [`LICENSE.md`](LICENSE.md) for the
full text — the license file is the single source of truth and is bundled with
each release.

### 📋 License at a Glance

| Activity / Use Case | Status | Terms |
|---|:---:|---|
| **Personal automation & productivity** | ✅ YES | Free for personal, hobby, and educational use |
| **Gaming & community play** | ✅ YES | Permitted for personal gameplay and automation |
| **Streaming on Twitch / YouTube** | ✅ YES | Streaming and monetized video content creation are 100% permitted |
| **Source inspection & forks** | ✅ YES | Open for study; public redistributions must rebrand |
| **Selling .exe / commercial licenses** | ❌ NO | Selling binaries or charging for access is strictly prohibited |
| **Commercial bot-farms / paid services** | ❌ NO | Automated commercial operations require a commercial license |
| **Adware / miner bundling** | ❌ NO | Strictly prohibited |

© 2026 Ar3ss12 · All rights not expressly granted are reserved.

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

---

<p align="center">Built with ❤️ using <a href="https://tauri.app">Tauri</a></p>
