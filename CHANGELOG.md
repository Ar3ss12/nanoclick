# Changelog

All notable changes to NanoClick. Sizes, timings and RAM figures are measured
and carry the command that re-derives them. Releases before 1.1.1 predate this
file.

## 1.5.0 (2026-10-31)

A hardware-ownership release: one token now gates every `SendInput` path, RSS
is judged by its derivative instead of a ceiling, a secure desktop stops the
run instead of swallowing clicks, and a frozen hook pump re-registers itself.
On top of that, adoption of the `snubber` 0.7.6 surface: the handle table gets
its own derivative watch, the cold frame learns WHAT happened, a refused retry
is spread instead of fixed, a slope shorter than a full window is named instead
of hidden, and the compiler is pinned at the repository root.

Suite green: **525 tests** (415 lib + 105 asset tripwires + 3 daemon handshake
+ 2 guardian e2e), clippy `-D warnings` over `--all-targets` (every target, not
only the lib), abort-shaped build (`ci-abort`), JS gate 10/10 scripts.

### Added

- **Hardware arbiter** (`stability::HardwareArbiter`, `HardwareToken`): the
  click run and macro playback can no longer inject at the same time. The
  cross-class gate is a single atomic word (`INPUT_CLAIM`: click leases
  counted in the low bits, the macro lease in the top bit) won by one
  `compare_exchange`, so "both owners at once" is unrepresentable rather than
  a race the losing side might notice; the token also holds a one-slot
  `hardware_arbiter` admission for the ledger. A Start during playback is
  refused at entry with `hardware_busy` / `HARDWARE_BUSY` instead of
  interleaving two owners of one cursor. The release is the admission's own
  armed rollback, not a second guard beside it: dropping a token (panic, `?`,
  generator unwind) runs `emergency_release_all_hardware` inside the node,
  then frees the slot, then gives the claim back, in that order. Because the
  body belongs to the node, the 1 Hz release latch times the same `SendInput`
  calls it reads.
- **Memory-leak detector by derivative**: RSS is sampled every 30 s into a
  10-sample ring (5 minutes). Ten strictly rising samples that add up to
  ≥ 120 MB latch `MEMORY_LEAK`, refuse Start and emit
  `AnomalyKind::MemoryGrowth`; a window that stops climbing (or dips) clears
  the verdict. The 1/2 GiB marks stay as a backstop for a process about to
  die, not as monitoring.
- **Secure-desktop watcher**: while a run can inject, the maintenance tick
  calls `OpenInputDesktop` on every tick. `ACCESS_DENIED` means the input is on
  the UAC / lock-screen desktop, where `SendInput` is swallowed and held
  buttons stay held: the tick releases every button, stops the click run and
  the macro playback (both own `SendInput`, and a playback left running kept
  `MACRO_CLAIM_BIT` so Start refused `hardware_busy` after unlock), forces the
  type-state to Idle and logs the transition once per loss.
- **Hook pump pulse + re-install**: the hook pump kicks `hook_pulse` every
  iteration; a 500 ms freeze is a deaf-hook threat (Windows drops a low-level
  hook whose owner stops pumping for `LowLevelHooksTimeout`, 200–1000 ms).
  The tick reports `FaultKind::Timeout`, counts the event and asks the hook
  thread to re-install; the hooks come back on the same thread (the OS only
  calls a low-level hook back on its installing thread). The lifetime count is
  in `hotkey_diag_dump` as `hook_reinstalls_total=`.
- **Handle-leak detector** (`stability::HANDLE_RING_LEN` /
  `snubber::HandleWatch<10>`): `GetProcessHandleCount` is sampled every 30 s,
  the same cadence as RSS, and only a full, strictly rising window that gained
  64 handles latches `HANDLE_LEAK` (`handle_leak` / `HANDLE_LEAK`, Start
  refused, cleared when the count stops climbing). A leaked `HANDLE` is kernel
  state and never shows up in the working set, so the RSS watch cannot see it.
  A probe that fails returns `None` and records nothing — a fabricated `0`
  would read as the dip that disarms the detector.
- **Cold event tape** (`stability::record_tape_event`): a hardware release, a
  lost input desktop and a hook re-install are recorded into the owning node's
  six-cell `FlightTape`, which rides in that node's 512 B `.snub` frame (crate
  wire v7) and in its triage card. Cold edges only — the click path never
  takes the tape lock.
- **Jittered retry** (`retry::retry_delay_ms`): the satellite build retry
  (90 ms ±20 %, band 72–108 ms, above the 50 ms COM teardown floor) and the
  OOP supervisor respawn (500 ms ±20 %) are salted by label, so a hud+overlay
  retry no longer wakes on the same millisecond and collides on the one-slot
  bulkhead. Deterministic: the same salt always yields the same delay.
- **`rust-toolchain.toml`** at the repository root: `stable` (the channel CI
  installs) with `clippy` + `rustfmt`, `profile = "minimal"`.
- **Handle-window diagnostics** (`stability::handle_window_line()`): the tick
  mirrors the sampled window into atomics (`handles`, `samples/10`, `trend`,
  `growth`, `trip`, `rate`) and `dump_input_diagnostics` logs it at `warn`, so
  a field log carries the count the 64-handle mark was armed without; the tick
  also logs one baseline line per completed window (5 min). `-` means "not
  available", never a fabricated `0`.
- **One-snubber lock tripwire** (`test_the_lock_carries_exactly_one_snubber`):
  the existing check reads only the first lock entry, so a second copy — a
  stale path package beside the registry one — would have stayed green while
  two APIs shared the tree.
- **Advisory slope is named** (`snubber::LeakClassification::Advisory`, new in
  0.7.6): a window that is 3–9 samples deep and projects past the trip mark is
  reported as its own verdict instead of falling into the table's `other` arm —
  `handle_window_line()` prints `trend=advisory` with the measured growth and
  rate, and the maintenance tick logs one `warn` slope line per RSS and handle
  window on the sample that ENTERS the advisory shape (`advisory_edge`), so a
  climbing window says so five minutes before it can trip. It never latches
  `MEMORY_LEAK` / `HANDLE_LEAK` and never refuses Start: `is_rising()` is false
  by design, and `start_refusal()` stays `None`. Tripwires:
  `test_an_advisory_slope_is_named_and_never_refuses_a_start`,
  `test_the_advisory_edge_logs_once_until_the_window_changes_shape`.
- **3D relative motion is a preset field** (`PresetItem::jitter_relative_3d`).
  The engine setting already existed, and the page already captured it into a
  preset snapshot, but the preset store had no field for it: the value was
  dropped on save, the editor had no control for it, and `apply_preset_engine`
  never wrote it, so a preset could neither carry nor clear the 3D mode. The
  store keeps it now, the preset editor renders and reads it, applying a 2D
  preset clears it instead of inheriting the previous run's value, records
  written before the field existed read as 2D (`#[serde(default)]`), and the
  card shows a `3D` tag. Tripwires:
  `a_preset_carries_the_3d_relative_motion_flag`, plus the frontend
  `the preset editor carries the 3D relative-motion flag`.

### Changed

- **The `BETA` badge is gone.** The header and the About panel show the
  version string only: the label described a pre-release line, and this build
  ships as a published release. The `.badge-beta` rule went with it, since
  nothing else used it.
- **A watched rollback is timed by the node that owns it.** The 1 Hz watcher
  calls `snubber::watchdog::poll_rollback(&node)` and the release latch reads
  `phase.budget_ms`, so no observer holds a copy of `ROLLBACK_BUDGET_MS` that
  a builder change could leave behind. The drift class — a threshold that
  describes a budget no node armed — is gone, not documented.
- **The leak window is the crate's, not ours.** The local `RssGradient`
  (a ring plus the strict-rise rule, previously written twice) is replaced by
  `snubber::LeakWatch<10>`, and the trip mark by `snubber::growth_crosses`.
  Behaviour is unchanged — ten 30 s samples, strictly rising, ≥ 120 MB — and
  the app keeps only the two policy numbers (`RSS_RING_LEN`,
  `MEMORY_GROWTH_TRIP_KB`). The migration added a flat-window case to the
  crate's tests: an equal neighbour provably clears the window, which the
  local copy asserted nowhere.
- **`snubber` 0.7.0 → 0.7.1: the memory verdict is one crate value.** The
  tick now feeds `snubber::MemoryTrend<10>` (level + ring + trip mark +
  sampling interval) instead of `LeakWatch` plus a local `growth_crosses`
  call, and reads the trend from `classification()` rather than the
  `sample()` verdict, so a `Soft` level cannot mask a rising window. The
  leak log now names the shape and the rate (`~Nkb/s`) from the crate's
  `LeakClassification::Rising`. Same latch, same clearing rule: a window
  that stops rising still clears `MEMORY_LEAK`. CI's snubber pin moves to
  the 0.7.1 release commit (9a73db7) in the same change.
- **The lint lane covers every target** (I49). CI ran `cargo clippy --lib`, and
  `--lib` never compiles `cfg(test)` code: findings inside the `#[cfg(test)]`
  modules and inside every `tests/*.rs` suite were invisible to the gate while
  those very suites ran green. The lane is `--all-targets` now, and the code
  under it is clean — `assertions_on_constants`, `bool_assert_comparison`,
  `items_after_test_module`, `field_reassign_with_default`, `type_complexity`,
  `len_zero`, `redundant_closure`, `manual_flatten`,
  `manual_pattern_char_comparison`, `doc_overindented_list_items` and
  `doc_lazy_continuation` across 12 source files, every one fixed at its source
  with no rule allowed out and no assertion weakened. The three constant-bound
  guards (`TRAY_FLUSH_WAIT_MS`, `WM_TRAY_CALLBACK`, `OWN_WRITE_GRACE_MS`) moved
  from runtime `assert!` to `const _: () = assert!(…)`, which is both what the
  lint asks for and strictly stronger: inside `cfg(test)` code, a violated bound
  stops every build that compiles the test lanes — the lint lane included —
  instead of failing one test nobody reruns. `test_overlay_module_exists` was
  `assert!(true)` — deleted, not repaired, which is why the lib lane reads 412
  where it read 413. Cost, cold
  re-check of this crate with warm dependency artifacts: `--lib` 8.4 s,
  `--all-targets` 33.7 s, both `-D warnings`. Proved by injecting a `len_zero`
  finding into `overlay.rs`'s test module: the `--lib` lane exited 0 over it, the
  `--all-targets` lane failed on it, and a one-line revert returned the lane to
  exit 0. The tripwire is proved the same way — writing `--lib` back into the
  workflow turns it red (`0 passed; 1 failed`, "the clippy lane must lint every
  target") and widening it again clears it. On the runner the widened step took
  153 s (CI run 41082043859, `5083ae3`, whole gate job 13m36s) against 208 s for
  the `--lib` step on the previous run 41071685085 (`2a33933`, 18m35s), so the
  wider scope costs no extra CI minutes. Tripwire:
  `test_the_ci_lint_lane_covers_every_target`.
- **Teardown order is pinned front to back.** `shutdown_application` runs nine
  owner actions plus the exit call; the existing tripwire now asserts the
  whole sequence, not two anchors. `snubber::RankedSet` was considered and not
  adopted: it ranks `&SnubberNode` stops, and none of these steps is a node.
- **`snubber` is an exact registry pin.** `src-tauri/Cargo.toml` carries
  `snubber = "=0.7.6"` (lock entry `0.7.6`, checksum `b533ab7f…`). The path dep,
  the second CI checkout and its SHA pin (I44) are gone: a path dep compiled
  whatever the dev host had, so the runner could stay green while users built a
  different API. A caret left the same hole one step later — `cargo update -p
  snubber` would move the app onto a release no gate here has measured without
  any line in this repo changing — so the requirement uses `=`.
  `test_ci_no_longer_pins_a_sha` holds all three: registry source, exact pin, and
  the absence of the caret form — measured by writing the caret back: the asset
  suite goes `104 passed; 1 failed` on that tripwire, and a one-character revert
  returns it to `105 passed; 0 failed`. 0.7.6 is a source-compatible minor release
  for this app: every `LeakClassification` match already carries a wildcard, and
  the drain arm-identity fix touches nothing NanoClick calls (it uses no
  `PipeDrain` / `WorkerPool`).
- **Secure desktop is sampled on every maintenance tick** (5 Hz), not once a
  second: a 1 Hz sample left buttons held for up to a second on UAC / Win+L,
  which is the whole damage. The gate is the injection-capable run, not the
  tick counter, so an idle tray process still reads nothing (I16). Tripwire:
  `test_desktop_is_sampled_every_tick_while_injecting`.
- **`HANDLE_LEAK` clears only on a readable, non-rising verdict.** A probe
  that returned `None` used to fall into the same branch as `Stable` and clear
  the latch while the window it contradicted had not changed.
- **One memory question per sample**: `MemoryTrend::sample()`'s `MemoryReport`
  carries the trend, so the tick reads `report.trend` instead of calling
  `classification()` a second time.

### Fixed

- `RELEASE_PENDING` read only the click node, so a wedged **macro** release —
  the same `SendInput` body, the same budget — was logged but never refused a
  new run. The 1 Hz tick now votes every `ROLLBACK_WATCHED` entry that holds
  hardware (`holds_hardware` column) and latches on the worst elapsed time.
- The `hardware_arbiter` row of `ROLLBACK_WATCHED` timed an empty closure
  (`enter_erased_holding_auto(|| {})`) while the real release lived in a
  `Protected` guard that published no phase, so the row could never fire and
  the latch read "fine" during a wedged `SendInput`. The admission now arms
  `emergency_release_all_hardware` itself; the second guard is gone.
  Tripwire: `test_arbiter_watch_times_the_release_body`.
- A secure desktop stopped the click run only: a macro playback kept looping
  and holding `MACRO_CLAIM_BIT`, so after unlock Start refused
  `hardware_busy` with nothing injecting on the visible desktop.
  `stop_for_desktop_loss` now stops the executor and the scheduler.
  Tripwire: `test_desktop_loss_stops_every_input_owner`.
- A stalled tick could latch `ENGINE_STUCK` from the process-wide CPU delta on
  its first sample of a thread. The latch now requires a real thread-scoped
  sample; the process delta is a log fallback only.
- Hook-stall classification samples the hook thread's own CPU
  (`ThreadCpuSampler::observe_tid`), so a busy UI can no longer be read as a
  busy hook.
- Dead surface removed: `dump_all_stats()` and `board_all_nodes()` had no
  callers (the board is read through `SNAPSHOT_NODES`). The inert
  `ProcessToken` arm is gone as well — it wrote only under `SNUBBER_SUPERVISED`
  and read as a guarantee; `input_guardian` is the real rescue.
- **Stop self-deadlock**: `set_active(false, Some(app))` held `self.state`
  while `finish_stop` emitted IDLE through `emit_status_now`, which read
  `is_active()` and re-locked the same non-reentrant `std::sync::Mutex`. The
  stop never returned: the hook thread froze mid-pump (`SleepBlocked`,
  `DeadlockStall`, `cpu_delta` 0 ms for 34.5 s in the 2026-10-31 field log),
  `get_status` wedged the WebView thread, and 17 re-install requests were
  posted with `hook_reinstalls_total` unchanged — none serviced. A double bind
  press under the user's `hotkey_debounce_ms = 5` reproduced it (start, then
  toggle-stop). The IDLE emit now runs after `drop(state_guard)` and
  `emit_status_now` takes `active` by value. Tripwire:
  `test_stop_status_emit_never_runs_under_the_state_lock` (negative proof:
  `self.is_active()` planted in the emitter and an emit planted before the
  release — both failed the gate, then were reverted).
- **Reinstall requested vs serviced**: the maintenance tick checks
  `hook_reinstall_pending()` before requesting again and logs
  `reinstall not serviced <idle>ms after the request` with `unserviced=`,
  `requests=` and `serviced=`; `hotkey_diag_dump` carries
  `hook_reinstall_requests_total=`, `hook_reinstall_unserviced_total=` and
  `hook_reinstall_pending=`, and the cold tape gains
  `TAPE_HOOK_REINSTALL_UNSERVICED` (code 4). A hook thread blocked inside an
  event handler never leaves its pump, so a request could not be told apart
  from a re-install. Tripwire:
  `test_a_pending_reinstall_is_named_apart_from_a_serviced_one`.
- `HANDLE_LEAK` could be cleared by a failed handle probe (`None` took the
  "not rising" branch), while the window still said `Rising`.
- `ErasedGuard` type annotations at `core/executor.rs` and `scheduler.rs`
  (`ErasedGuard<ProbeSlot>`), the only two call sites the 0.8 typed slots
  broke.

## 1.4.0 (2026-10-29)

A resilience release: the click engine gains an out-of-process safety net and
a stability interlock (snubber), the interface becomes disposable, and the
repository ships with a real CI gate. Suite green: **499 tests** (392 lib +
102 asset tripwires + 3 daemon handshake + 2 guardian e2e), clippy
`-D warnings`, all lanes on the GitHub Actions runner in one measured
15 m 34 s pass (run 40773643808, 2026-10-29).

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
  plus a one-write-back triage file, both scoped to the process role
  (`nanoclick_<role>_<node>.snub`, `nanoclick_triage_<role>.txt`).
- **Click-pacing bench**: `cargo bench --bench click_pacing` measures the
  per-click humanization compute — **35–223 ns** in the worst corridor
  (jitter 35 %, hesitation 0.10) against a 6.25 ms minimum interval at 160 CPS.
- **GitHub Actions CI** (`.github/workflows/ci.yml`): windows-latest, one
  gate job — JS gates → clippy `-D warnings` → `panic = "abort"` compile gate
  (lib + both bins) → lib tests (non-physical + physical lanes) → integration
  suites (asset tripwires, daemon handshake, guardian e2e in one cargo
  invocation) → ignored chaos lane. All lanes `--locked -j 2`. Test lanes run
  on `[profile.ci-test]`: same opt-level, `panic = "unwind"` and `strip` as
  release, with ThinLTO and 16 codegen units, so each `tests/*.rs` executable
  stops paying a whole-program FatLTO pass over the dependency graph; the
  shipped binary is still built from `[profile.release]`. Doc-only pushes
  (`**.md`, `docs/**`, `.github/assets/**`, `LICENSE*`, `.gitignore`) do not
  start a run at all.
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
- Cold evidence no longer collides in the daemon topology: daemon and UI child
  both run a maintenance thread, and the shared `%TEMP%/nanoclick_<node>.snub`
  let the long-lived daemon overwrite the frames of the UI process that had
  just crashed. Every snapshot and the triage file now carry the process role,
  derived from `--ui-child` / the `nanoclick-daemon` exe stem.
- `hotkey_hook` published a `rollback_budget_ms` that no tick read (the 1 Hz
  watchdog polled three nodes by hand). `ROLLBACK_WATCHED` now owns the poll
  list, and a test fails if a node publishes a budget no tick observes.
- `ENGINE_STUCK` could be latched by a busy UI: the click-stall classifier
  read the process-wide CPU delta, so a repainting WebView could turn a
  blocked worker into `CpuSpin` and refuse Start. The worker publishes its TID
  around the pulse and the tick samples `GetThreadTimes` through a reusable
  `OpenThread` handle; the process delta stays a fallback only when the thread
  cannot be opened.

### Changed

- `snubber` 0.6.13 → 0.7.0. Erased admissions use the canonical
  `probe_erased_auto` name: same semantics as the old alias (slot released at
  entry, rollback still armed for an implicit drop), so a refused or panicking
  run still releases mouse buttons through `SendInput`.
- The shipped profile keeps `panic = "unwind"` on purpose: hook callbacks run
  under `catch_unwind`, so a panicking callback cannot take the process down
  while `Drop` guards still run. Test *execution* under abort is impossible
  with today's tooling (4 measured probes; rust-lang/rust#67650) — CI compiles
  lib + bins with `panic = "abort"` and executes tests on
  `[profile.ci-test]`.
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
