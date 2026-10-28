# Changelog

All notable changes to NanoClick. Sizes, timings and RAM figures are measured
and carry the command that re-derives them. Releases before 1.1.1 predate this
file.

## 1.4.0 (unreleased)

A resilience release: the click engine gains an out-of-process safety net and
a stability interlock (snubber), the interface becomes disposable, and the
repository ships with a real CI gate. Suite green: **494 tests** (387 lib +
102 asset tripwires + 3 daemon handshake + 2 guardian e2e), clippy
`-D warnings`, all lanes on the GitHub Actions runner in one measured
20 m 14 s pass.

### Added

- **Out-of-process input guardian** (`--input-watchdog`): the RAII
  `MouseHoldGuard` releases the mouse on every loop exit, `?`-error and
  unwinding panic; fatal kills (`End Task`, OOM, crash below Rust) are handled
  by the guardian, because `Drop` cannot run there. Verified end-to-end: the
  OS accepts all three `SendInput` releases (exit `3`), and a one-shot
  launcher hop keeps the guardian out of the app's process tree, so
  `taskkill /F /T` cannot take it down.
- **Stability interlock (snubber)**: a 200 ms maintenance thread observes the
  click worker's pulse, rollback bodies and RSS. A wedged engine or a hardware
  release still in flight latches (`ENGINE_STUCK`, `RELEASE_PENDING`,
  `MEMORY_HARD`/`MEMORY_PRESSURE`), Start is refused with a named verdict, and
  the status surfaces (in-app status line + `get_full_state`) carry the
  verdicts to the UI. Cold diagnostics: one 512-byte `SNUB v6` frame per node
  plus a one-write-back triage file (`nanoclick_triage.prev.txt`).
- **Click-pacing bench**: `cargo bench --bench click_pacing` measures the
  per-click humanization compute — **35–223 ns** in the worst corridor
  (jitter 35 %, hesitation 0.10) against a 6.25 ms minimum interval at 160 CPS.
- **GitHub Actions CI** (`.github/workflows/ci.yml`): windows-latest, one
  gate job — JS gates → `panic = "abort"` compile gate (lib + both bins) →
  lib tests (non-physical + physical lanes) → clippy `-D warnings` → asset
  tripwires → daemon handshake (+ ignored chaos lane) → guardian e2e. All
  lanes `--locked -j 2`.
- Settings: System/Typing behavior cards split with toggle switches, custom
  theme dropdown; the interface language swaps without a reload (cached
  dictionaries); backup button with honest states, in-page import modal and a
  5 MB size guard.
- Capture, catalog, pixel, picker, dismiss, stats and dispatcher plumbing
  migrated from the page into the Rust backend; catalog search is debounced
  behind a generation token and allocates nothing per keystroke.

### Fixed

- An explicit stop on an already-idle scheduler now clears the `engine_stuck`
  latch: the early-return path skipped `clear_engine_stuck()`, so a
  start-after-stop under a latched engine stayed refused. Covered by a
  dedicated interlock test; the test-facing race it exposed (3 spurious
  failures measured against process-global latches under parallel tests) is
  serialized by a cfg(test) reentrant toggle gate.
- Config writers migrated to snubber scoped saves (`node.scoped()`); a NaN can
  no longer poison the Bates jitter guard (hermetic test).
- Satellite windows (HUD/overlay): a deferred HUD log is a warn, tray HUD
  toggles go through the facade, and the hide/show satellite model matches the
  tripwire gates (9 stale gates re-aligned).

### Changed

- The shipped profile keeps `panic = "unwind"` on purpose: hook callbacks run
  under `catch_unwind`, so a panicking callback cannot take the process down
  while `Drop` guards still run. Test *execution* under abort is impossible
  with today's tooling (4 measured probes; rust-lang/rust#67650) — CI compiles
  lib + bins with `panic = "abort"` and executes tests on the shipped profile.
- Deep-sleep / hide lifecycle: satellite windows are created behind a 300 ms
  latch and **destroyed** (never `hide()`) on close — a hidden WebView keeps
  ~120 MB alive and a stray `show()` resurrects it; restore rebuilds from the
  saved preference. Measured: 117–123 MB alive → 5.3 MB in deep sleep
  (`scripts/measure-ram.ps1`).

## 1.3.0 (2026-10-05)

A hardening release: quieter in the tray, harder to break, and updates itself
cleanly — including the zero-install portable binary.

- NSIS updates tear down cleanly before the installer takes over
  (`download()` → `prepare_nsis_update` → `install()`): the tray icon, the
  clicker, hooks and the foreground patrol stop and the final config flushes
  before the updater plugin exits the process.
- Portable updates swap the running exe in place after a minisign re-verify
  and keep `.old` beside the new binary until the first healthy boot as a free
  rollback copy.
- The app is actually idle in the tray: the keyboard hook thread parks in the
  Windows kernel instead of spinning on a 5 ms timeout, the app-profile thread
  checks a file timestamp instead of re-parsing config every 1500 ms, the
  config watcher polls at 2 s, and hidden pages stop polling GitHub.
- A `SetWinEventHook` foreground patrol warns about elevated windows at 0 %
  idle CPU; seamless elevation is opt-in via a Task Scheduler logon task.
- Config saves are refused before hydration finishes; `config.json.bak-<epoch>`
  is written before the first write of every session; presets live in their
  own verified `presets.json`.

## 1.2.1

A safety release: nothing new to configure — but nothing you configure can be
lost any more, no recording is dropped, and the window stays where you put it.

## 1.2.0 (2026-09-25)

The interface became disposable, side mouse buttons became full citizens,
presets became full working profiles that can be bound to hotkeys and switched
instantly on the fly, and the click engine gained realistic biomechanics with
dynamic fadeout from 1 to 160 CPS.

## 1.1.1 (2026-09-21)

A stability release: boot guard makes a dead frontend impossible to ship, and
two guards that were not doing their job — Auto-Pause on Focus Loss and the
Typing Guard delay field — finished their work.
