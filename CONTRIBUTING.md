# Contributing (maintainer notes)

Everything that does not belong on the product front page: version bumps,
test commands, the JS gate table, and the release/signing flow.

## Prerequisites

[Rust](https://rustup.rs) (stable-msvc), Microsoft C++ Build Tools, WebView2
(preinstalled on Windows 10/11).

```bash
git clone https://github.com/Ar3ss12/nanoclick.git
cd nanoclick
cargo tauri dev          # dev mode
cargo tauri build --bundles nsis
# → target/release/bundle/nsis/NanoClick_*-setup.exe
```

## Bump the version

`release.ps1 -Tag vX.Y.Z` only *reads* the version (installer file name +
`latest.json`), so the files must already carry it. One command rewrites every
place the version lives and verifies the result:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\set-version.ps1 -Version 1.2.0
# add -DryRun to preview, -SkipDocs to touch only the two files tauri reads
```

It updates `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json` and the
three "Current version" doc headers; historical version numbers further down
those files are left alone. The new JSON is parsed *before* it is written,
because a malformed `tauri.conf.json` makes every cargo command die inside
tauri-build.

## Run tests

```bash
cd src-tauri
cargo test --release --locked -j 2 --lib -- --skip physical_
# physical_ lanes are hardware-timing tests: run them separately
cargo test --release --locked -j 2 --lib -- physical

cargo test --release --locked -j 2 --tests
# integration suites: asset tripwires, daemon handshake (+ chaos lane), guardian e2e
```

CI ([.github/workflows/ci.yml](.github/workflows/ci.yml)) runs the full matrix
on `windows-latest`: JS gates → `panic = "abort"` compile gate → lib test
lanes → clippy `-D warnings` → integration suites. All lanes `--locked -j 2`.

> **Copy these commands verbatim.** `cmd.exe` has no `#` comments: a trailing
> `# …` is passed to the test harness as extra filters and silently runs a
> fraction of the suite. Use PowerShell if you want comments, or keep the
> command on its own line.

## Frontend safety net (gates, not vibes)

`main.js` is an ES module compiled by nothing: one duplicate top-level
declaration is a fatal `SyntaxError` that leaves the window painted with every
handler unattached — while the whole Rust suite stays green. These gates exist
for exactly that class:

| Gate | Command | What it catches |
|---|---|---|
| Grammar (browser-accurate) | `powershell -ExecutionPolicy Bypass -File scripts\check-js-syntax.ps1` | module-mode `SyntaxError`s (checked as `.mjs`), 404 script refs |
| Lint (broad tripwire) | `powershell -ExecutionPolicy Bypass -File scripts\check-js-lint.ps1` | undefined/unused identifiers, dead code, duplicate keys — oxlint (Rust binary, no `node_modules` required). Gate = **0 errors / 0 warnings** |
| Rust gates | `cd src-tauri && cargo test --release -j 1 --test test_assets` | grammar + duplicate-declaration scan + boot-guard wiring, run as part of `cargo test` |
| Release preflight | `scripts\release.ps1` → Stage 0 (PREFLIGHT) | runs the gates **before** `cargo tauri build`, which never runs tests |

Runtime trap: `src/boot_guard.js` is a **classic** script loaded first on every
page. It traps uncaught errors, unhandled rejections and 404 assets, forwards
them to `%TEMP%\nanoclick_web.log`, and if a page never raises its boot flag
within ~3 s it reports `[BOOT] MODULE DID NOT EXECUTE (SyntaxError class)` and
shows a visible banner. That watchdog is the only detection that survives a
module which cannot execute at all.

## Changelog

Notable user-facing changes land in [CHANGELOG.md](CHANGELOG.md) under
`1.4.0 (unreleased)` and are dated only when the release tag is cut. Numbers
in the changelog are measured and carry the command that re-derives them.

## Signing & releases

Two independent signing systems:

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

> ⚠️ Keep the private updater key safe — installations already shipped only
> trust updates signed with the same key they were built with.

## Architecture

```

## Architecture

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

- All timing runs on a Rust worker thread — the UI never drives the click
  loop; 66 ms (15 FPS) IPC telemetry from lock-free atomics.
- Lazy secondary WebViews: cold boot creates only the main window; overlay and
  HUD are built on demand and destroyed when toggled off.
- In-process GPU mode is deliberately off (a DirectX reset would hang the
  click/hook thread); RAM is saved by lazy windows instead.
