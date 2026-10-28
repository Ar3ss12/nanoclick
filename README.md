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

---

## ✨ What it does

### 🖱️ Click engine
- **Click modes** — Single / Double / Hold / Butterfly / Drag, 0.1–160 CPS with live hotkey speed adjustment and real-time telemetry
- **Humanized timing** — Bates B3 jitter (±0–35 %), Gaussian spatial variance (±0–50 px), hold times, fatigue tail; smooth fadeout keeps every mode controllable at high speeds
- **Measured pacing** — the per-click humanization compute costs **35–223 ns** in the worst corridor against a 6.25 ms minimum interval (`cargo bench --bench click_pacing`)
- **Position picker** — bind clicks to a fixed screen point or follow the cursor
- **Unit switchers** — toggle between CPS (clicks/sec) and ms (interval) right from the dashboard

### 🎬 Macros
- **Record or build** — capture real input in real time, or assemble macros from blocks in the visual editor (inline edit, rename, drag-to-reorder, run-from-here, step, disable, duplicate)
- **Trajectory cleanup** — Ramer–Douglas–Peucker simplification compresses recorded drag paths by **80–95 %** while keeping curvature; redundant hover moves are stripped before every click/key action
- **Control flow** — `Repeat`, `If/Else` with pixel-color conditions, variables (`SetVar`/`GetVar`), nested macro calls
- **Presets** — instant hotkey preemption between presets via direct hotkey trigger; import/export full engine presets; total click statistics persist across updates

### 🛡️ Safety & reliability
- **Out-of-process input guardian** (`--input-watchdog`) — the mouse is released even when the process dies below Rust (`End Task`, OOM, crash below Rust), where a Rust `Drop` cannot run. Verified end-to-end: the OS accepts all three `SendInput` releases (exit `3`), and a one-shot launcher hop keeps the guardian outside `taskkill /F /T` reach
- **Stability interlock (snubber)** — a 200 ms maintenance thread observes the click worker's pulse, rollback bodies and RSS; a wedged engine or a release still in flight latches, Start is refused, and the status surfaces name it (`ENGINE_STUCK`, `RELEASE_PENDING`, `MEMORY_HARD`/`MEMORY_PRESSURE`) instead of a lying `RUNNING`. Cold diagnostics: one 512-byte `SNUB v6` frame per node plus a one-write-back triage file
- **Daemon split** — `nanoclick-daemon` owns iron, config and truth (single-instance mutex, TCP `127.0.0.1:44771`, UI supervisor with 200 ms respawn); the Tauri UI is a thin remote. Kill the window and the engine never flinches — proven by handshake + kill-UI chaos gates
- **Smart Typing Guard** (default ON) — auto-stops clicking when you type real text and locks out hotkeys; 2-second intentional cooldown before disarming
- **FocusGuard** (opt-in) — stops within **~50 ms** of a foreground change (Alt+Tab, click-away, system toast); fail-open on unresolvable focus, and a toast names the process that stole focus
- **Self-healing configuration** — Last-Known-Good snapshots (hardened readonly+hidden), timestamped quarantine of corrupt files, mandatory Deadbolt modal queue, factory reset with a 5-second cooldown
- **Zero-latency stop** — <kbd>Escape</kbd> and hotkey stop land at 0 ms; emergency stop always available

### 🖥️ Windows integration
- **Event-driven Win32 hooks** (`WH_KEYBOARD_LL` / `WH_MOUSE_LL`) — zero busy-polling at idle, sub-millisecond reaction, zero-lock thread-local state
- **Tray & deep sleep** — close to tray and the engine keeps clicking; deep sleep drops resident RAM to **5.3 MB** (measured `scripts/measure-ram.ps1`, from 117–123 MB) while hooks, scheduler and tray stay alive; the tray icon re-registers after an `explorer.exe` restart
- **UIPI awareness** — detects elevated windows (Task Manager, admin consoles), warns with a synthesized chime + dialog, one-click admin restart (`runas`)
- **PerMonitorV2 DPI** — zero coordinate drift across mixed-DPI multi-monitor setups
- **6 themes** (cyberpunk/neon palette), floating HUD overlay, autostart, UA + EN localization (lazy-loaded ~12 KB JSON, zero-overhead)

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

### Bump the version

`release.ps1 -Tag vX.Y.Z` only *reads* the version (installer file name + `latest.json`),
so the files must already carry it. One command rewrites every place the version lives and
verifies the result:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\set-version.ps1 -Version 1.2.0
# add -DryRun to preview, -SkipDocs to touch only the two files tauri reads
```

It updates `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json` and the
three "Current version" doc headers; historical version numbers further down
those files are left alone. The new JSON is parsed *before* it is written,
because a malformed `tauri.conf.json` makes every cargo command die inside
tauri-build.

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

### Frontend safety net (gates, not vibes)

`main.js` is an ES module compiled by nothing: one duplicate top-level declaration is a
fatal `SyntaxError` that leaves the window painted with every handler unattached — while
the whole Rust suite stays green. These gates exist for exactly that class:

| Gate | Command | What it catches |
|---|---|---|
| Grammar (browser-accurate) | `powershell -ExecutionPolicy Bypass -File scripts\check-js-syntax.ps1` | module-mode `SyntaxError`s (checked as `.mjs`), 404 script refs |
| Lint (broad tripwire) | `powershell -ExecutionPolicy Bypass -File scripts\check-js-lint.ps1` | undefined/unused identifiers, dead code, duplicate keys — oxlint (Rust binary, no `node_modules` required). Gate = **0 errors / 0 warnings** |
| Rust gates | `cd src-tauri && cargo test --release -j 1 --test test_assets` | grammar + duplicate-declaration scan + boot-guard wiring, run as part of `cargo test` |
| Release preflight | `scripts\release.ps1` → Stage 0 (PREFLIGHT) | runs the gates **before** `cargo tauri build`, which never runs tests |

> **Copy these commands verbatim.** `cmd.exe` has no `#` comments: a trailing `# …`
> is passed to the test harness as extra filters and silently runs a fraction of the
> suite. Use PowerShell if you want comments, or keep the command on its own line.

Runtime trap: `src/boot_guard.js` is a **classic** script loaded first on every page. It
traps uncaught errors, unhandled rejections and 404 assets, forwards them to
`%TEMP%\nanoclick_web.log`, and if a page never raises its boot flag within ~3 s it
reports `[BOOT] MODULE DID NOT EXECUTE (SyntaxError class)` and shows a visible banner.
That watchdog is the only detection that survives a module which cannot execute at all.

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
