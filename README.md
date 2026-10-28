# ⚡ NanoClick

**The lightweight native auto clicker for Windows** — human-like clicks, macro
recorder, visual editor. No Electron, no bloat: **~3.4 MB** installer
(measured 2026-10-28).

![Platform](https://img.shields.io/badge/platform-Windows%2010%2F11-blue)
[![CI](https://github.com/Ar3ss12/nanoclick/actions/workflows/ci.yml/badge.svg)](https://github.com/Ar3ss12/nanoclick/actions/workflows/ci.yml)

**⬇️ [Download v1.3.0](https://github.com/Ar3ss12/nanoclick/releases/tag/v1.3.0)** —
installer `NanoClick_1.3.0_x64-setup.exe` (~3.4 MB) or zero-install portable
`NanoClick-portable.exe` (~9.7 MB)

<p align="center">
  <img src=".github/assets/main_preview.png" alt="NanoClick Dashboard Preview" width="800">
</p>

---

## Why NanoClick

| A clicker from a random download site | NanoClick |
|---|---|
| Electron UI eating 150+ MB of RAM; an unsigned exe of unknown origin | **9.7 MB portable, 5.3 MB in tray** (measured) — open source, signed updates |
| A crash can leave your mouse stuck in a click loop | **a watchdog *outside* the process releases the buttons — always, even after End Task** (proven end-to-end) |
| Metronome-flat fixed-interval clicks a bot detector smells instantly | **human-like jitter and timing modeled on real hand biomechanics** — and the timing math costs nanoseconds per click, so your FPS never feels it (measured) |

---

## What it does

<table>
<tr><th valign="top">🖱️ Clicker</th><th valign="top">🎬 Macros</th></tr>
<tr><td valign="top">

- Single / Double / Hold / Butterfly / Drag, 0.1–160 CPS
- Hotkey speed adjustment on the fly, live telemetry
- Human-like timing: Bates-model jitter (±0–35 %), spatial variance (±0–50 px), hold times and a fatigue tail — reads like a hand, not a metronome
- Bind to a fixed point or follow the cursor
- CPS/ms switcher right on the dashboard, floating HUD, 6 themes

</td><td valign="top">

- Record real input in real time, or build macros from blocks
- Visual editor: rename, drag-to-reorder, run-from-here, step, disable, duplicate
- Recorded drag paths are compressed by **80–95 %** — a chaotic mouse swipe becomes a clean macro
- Logic included: `Repeat`, `If/Else` with pixel-color conditions, variables, nested macro calls
- Presets switch instantly on hotkeys; import/export; click stats persist across updates

</td></tr>
</table>

### And it is hard to break

- **No stuck mouse — ever.** If the app freezes, crashes or gets killed in
  Task Manager, an independent watchdog outside the process instantly
  releases the buttons
- **Honest status.** If anything wedges internally, the app refuses to start
  and tells you why — it never shows a fake "Running"
- **Kill the window, nothing happens.** The engine lives in a separate daemon
  process; closing or killing the UI never interrupts clicking
- **Stops when you type** (default on) and **when you leave the window**
  (optional) — FocusGuard reacts within ~50 ms and names the app that stole
  focus
- **Config that heals itself.** Last-Known-Good snapshots, quarantine of
  corrupt files, factory reset with a 5-second cooldown
- **Zero-latency stop** — <kbd>Escape</kbd> and hotkey stop land at 0 ms

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

## 📦 System requirements

- Windows 10/11; installs per-user, no admin rights needed
- WebView2 runtime (preinstalled on Windows 10/11; downloaded automatically if missing)
- Updates are signed and verified before install

## 🛠️ Build from Source

```bash
git clone https://github.com/Ar3ss12/nanoclick.git
cd nanoclick
cargo tauri dev          # dev mode
cargo tauri build --bundles nsis
# → target/release/bundle/nsis/NanoClick_*-setup.exe
```

**Prerequisites:** [Rust](https://rustup.rs) (stable-msvc), Microsoft C++ Build Tools.

Developer docs — test commands, the JS gate table, version bumps, the release
and signing flow, and the architecture diagram — live in
[CONTRIBUTING.md](CONTRIBUTING.md). What's new: [CHANGELOG.md](CHANGELOG.md).

## 📄 License & good to know

- Free for personal use — PolyForm Noncommercial 1.0.0 plus the NanoClick
  Additional Terms ([LICENSE.md](LICENSE.md) is the single source of truth)
- Windows only — the engine is built on Win32 input APIs
- Clicks into elevated windows are detected and warned about, never forced

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
