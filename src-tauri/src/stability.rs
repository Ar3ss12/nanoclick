//! # NanoClick stability layer (`snubber` integration).
//!
//! Transaction admission (bulkheads), failure classification (catastrophic
//! vs. shedding), circuit breaking, scoped rollback execution, task
//! steering (`TaskConduit` / `TaskHandle` / `Directive`) and cold
//! diagnostics (`SnapshotV1`, `NodeSet`, `[DIAG 0x...]` lines).
//!
//! Layering: `snubber` provides admission math, guard lifecycle and the
//! 512 B snapshot codec. This file owns the node set, the worker
//! steering handles, the snapshot file paths and the log emission.

use snubber::supervisor::ProcessToken;
use snubber::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

// ── 0. Published P1 budgets (one number per decision, never a literal) ─────

/// Rollback-body budget. A body past this is named by the 1 Hz maintenance
/// tick — `emergency_release_all_hardware` calls `SendInput`, which can block
/// on UIPI, so "rollback is running" is a state that must be observable.
pub const ROLLBACK_BUDGET_MS: u64 = 250;

/// Click-worker pulse lease. The worker kicks once per loop iteration and
/// once per ≤100 ms slice inside a long wait, so 1000 ms of silence is a
/// wedged thread, not a slow cadence.
pub const CLICK_PULSE_LEASE_MS: u64 = 1000;
/// Click-worker CPU lease: a blown wall lease with ≥200 ms of CPU in the same
/// window is a spin (`loop {}`), not a blocked syscall.
pub const CLICK_CPU_BUDGET_MS: u64 = 200;
/// Click-worker silence ceiling: 2 s of wall silence with no CPU growth is a
/// deadlock, not an OS/EDR pause.
pub const CLICK_STALL_CEILING_MS: u64 = 2000;

/// Hook-loop pulse lease. `MsgWaitForMultipleObjects` wakes the loop every
/// 20 ms, so 2000 ms is 100 consecutive missed waits.
pub const HOOK_PULSE_LEASE_MS: u64 = 2000;
pub const HOOK_CPU_BUDGET_MS: u64 = 500;
pub const HOOK_STALL_CEILING_MS: u64 = 4000;

/// Admission cadence of the maintenance thread: pulses are observed at 200 ms,
/// rollback + memory at 1 Hz (every fifth tick), snapshots every 30 s.
pub const MAINTENANCE_TICK_MS: u64 = 200;

/// RSS limits for the engine node, KiB. Soft refuses new admissions while a
/// run drains; hard is the supervisor's "terminate without draining" mark.
/// A long tray session that leaks past 1 GiB is a bug, not normal load (the
/// measured idle working set is ~120 MB).
pub const MEMORY_SOFT_KB: u64 = 1_048_576;
pub const MEMORY_HARD_KB: u64 = 2_097_152;

// ── 1. Supervisor opcodes for cross-process input release ──────────────────
pub const OP_RELEASE_MOUSE_LEFT: u32 = 10;
pub const OP_RELEASE_MOUSE_RIGHT: u32 = 11;
pub const OP_RELEASE_MOUSE_MIDDLE: u32 = 12;
pub const OP_RELEASE_ALL_INPUTS: u32 = 13;

static NEXT_TASK_ID: AtomicU64 = AtomicU64::new(1);

#[inline]
pub fn next_task_id() -> u64 {
    NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed)
}

/// Arm the hardware token for an out-of-process supervisor.
///
/// Inert in the shipped topology: snubber only emits the token when it is the
/// parent (`SNUBBER_SUPERVISED`) and drains its own pipe. NanoClick's rescue
/// path is [`crate::input_guardian`]; these calls stay because they are the
/// seam for adopting `snubber::Supervisor` later, and they cost two writes per
/// run (never per click).
pub fn arm_hardware_token(task_id: u64, opcode: u32, tag: u8) {
    let token = ProcessToken::simple(task_id, opcode, tag);
    token.emit_if_supervised();
}

/// Emit hardware commit token to external supervisor, clearing armed rescue action.
pub fn commit_hardware_token(task_id: u64) {
    let token = ProcessToken::commit(task_id);
    token.emit_if_supervised();
}

/// Emergency fallback: release any physically held mouse buttons and keys.
/// Called in-process during teardown or recovery, and by the supervisor on EOF.
pub fn emergency_release_all_hardware() {
    crate::debug_log_internal("warn", "[Stability] Emergency hardware release triggered");
    let backend = crate::platform::default_input_backend();
    backend.release_mouse_hold(crate::core::action::MouseButton::Left);
    backend.release_mouse_hold(crate::core::action::MouseButton::Right);
    backend.release_mouse_hold(crate::core::action::MouseButton::Middle);
}

// ── 2. Snubber nodes (bulkheads + breakers + telemetry) ─────────────────────

static CLICK_NODE: OnceLock<SnubberNode> = OnceLock::new();
static CLICK_STATE_NODE: OnceLock<SnubberNode> = OnceLock::new();
static HOTKEY_NODE: OnceLock<SnubberNode> = OnceLock::new();
static MACRO_NODE: OnceLock<SnubberNode> = OnceLock::new();
static WATCHER_NODE: OnceLock<SnubberNode> = OnceLock::new();
static PROFILE_SWITCH_NODE: OnceLock<SnubberNode> = OnceLock::new();
static SATELLITE_NODE: OnceLock<SnubberNode> = OnceLock::new();
static CONFIG_NODE: OnceLock<SnubberNode> = OnceLock::new();
static CLICK_RUN_NODE: OnceLock<SnubberNode> = OnceLock::new();

/// Clicker State Transition Node (Phase 7):
/// - Guards ClickerState transitions (Idle <-> Running <-> Paused)
/// - Short stack-local transactions (5 us), never blocks on long-running workers.
pub fn click_state_node() -> &'static SnubberNode {
    // Compile-time capacities (`StaticNode` const-asserts IN_FLIGHT > 0 and
    // QUEUE >= IN_FLIGHT); runtime per-transaction state lives in scheduler.rs.
    CLICK_STATE_NODE.get_or_init(|| snubber::StaticNode::<8, 8>::build("click_state"))
}

// ── 2a. Typed clicker automaton (`LegalMove` ribs) ──────────────────────────

/// Marker for `ClickerState::Idle`; the ribs below are the ONLY legal edges:
/// Idle→Running, Running→Paused, Paused→Running, Running→Idle, Paused→Idle.
/// A call site that names an illegal source state sends a pair with no
/// `impl LegalMove` and does not compile (E0277) — the graph stops being a
/// comment and becomes a type.
pub struct ClickIdle;
pub struct ClickRunning;
pub struct ClickPaused;

impl snubber::LegalMove<ClickRunning> for ClickIdle {
    fn convert(_from: Self) -> ClickRunning {
        ClickRunning
    }
}
impl snubber::LegalMove<ClickPaused> for ClickRunning {
    fn convert(_from: Self) -> ClickPaused {
        ClickPaused
    }
}
impl snubber::LegalMove<ClickRunning> for ClickPaused {
    fn convert(_from: Self) -> ClickRunning {
        ClickRunning
    }
}
impl snubber::LegalMove<ClickIdle> for ClickRunning {
    fn convert(_from: Self) -> ClickIdle {
        ClickIdle
    }
}
impl snubber::LegalMove<ClickIdle> for ClickPaused {
    fn convert(_from: Self) -> ClickIdle {
        ClickIdle
    }
}

/// Why a clicker-state move did not happen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickMoveError<E> {
    /// `click_state` node refused admission (breaker/bulkhead).
    Admission(snubber::EnterError),
    /// The guarded work returned an error.
    Work(E),
    /// The requested edge does not exist (`Idle -> Paused`, `Paused -> Paused`…).
    IllegalMove(crate::scheduler::ClickerState),
    /// A future `SnubberError` variant this build does not know.
    Unknown,
}

impl<E: std::fmt::Debug> From<snubber::SnubberError<E>> for ClickMoveError<E> {
    fn from(err: snubber::SnubberError<E>) -> Self {
        match err {
            snubber::SnubberError::Admission(e) => ClickMoveError::Admission(e),
            snubber::SnubberError::Work(e) => ClickMoveError::Work(e),
            // `SnubberError` is `#[non_exhaustive]`: fail closed, never guess.
            _ => ClickMoveError::Unknown,
        }
    }
}

/// Idle→Running or Paused→Running. `IllegalMove` for a Running/unknown source.
pub fn click_to_running<T, E, R, W>(
    from: crate::scheduler::ClickerState,
    rollback: R,
    work: W,
) -> Result<T, ClickMoveError<E>>
where
    R: FnOnce(),
    W: FnOnce() -> Result<T, E>,
    E: std::fmt::Debug,
{
    let node = click_state_node();
    match from {
        crate::scheduler::ClickerState::Idle => node
            .transition_checked(ClickIdle, rollback, move |_s| work())
            .map(|(_, out): (ClickRunning, T)| out)
            .map_err(ClickMoveError::from),
        crate::scheduler::ClickerState::Paused => node
            .transition_checked(ClickPaused, rollback, move |_s| work())
            .map(|(_, out): (ClickRunning, T)| out)
            .map_err(ClickMoveError::from),
        crate::scheduler::ClickerState::Running => Err(ClickMoveError::IllegalMove(from)),
    }
}

/// Running→Idle or Paused→Idle. `IllegalMove` for an Idle source.
pub fn click_to_idle<T, E, R, W>(
    from: crate::scheduler::ClickerState,
    rollback: R,
    work: W,
) -> Result<T, ClickMoveError<E>>
where
    R: FnOnce(),
    W: FnOnce() -> Result<T, E>,
    E: std::fmt::Debug,
{
    let node = click_state_node();
    match from {
        crate::scheduler::ClickerState::Running => node
            .transition_checked(ClickRunning, rollback, move |_s| work())
            .map(|(_, out): (ClickIdle, T)| out)
            .map_err(ClickMoveError::from),
        crate::scheduler::ClickerState::Paused => node
            .transition_checked(ClickPaused, rollback, move |_s| work())
            .map(|(_, out): (ClickIdle, T)| out)
            .map_err(ClickMoveError::from),
        crate::scheduler::ClickerState::Idle => Err(ClickMoveError::IllegalMove(from)),
    }
}

/// Running→Paused only. `IllegalMove` for an Idle or already-Paused source —
/// "pause the idle clicker" used to write `Paused` over `Idle` silently.
pub fn click_to_paused<T, E, R, W>(
    from: crate::scheduler::ClickerState,
    rollback: R,
    work: W,
) -> Result<T, ClickMoveError<E>>
where
    R: FnOnce(),
    W: FnOnce() -> Result<T, E>,
    E: std::fmt::Debug,
{
    match from {
        crate::scheduler::ClickerState::Running => click_state_node()
            .transition_checked(ClickRunning, rollback, move |_s| work())
            .map(|(_, out): (ClickPaused, T)| out)
            .map_err(ClickMoveError::from),
        _ => Err(ClickMoveError::IllegalMove(from)),
    }
}

/// Click Worker Node:
/// - Trip breaker after 5 repeated failures in a 10s window.
///
/// `max_in_flight(1)` is an *admission* probe, not a lifetime lease: snubber's
/// erased guards release the slot at entry (`settle_admission_only`), so this
/// number never serialises live workers. Actual click-run serialisation is
/// owned by `ClickerState` + `run_gen` in `scheduler.rs`; the typed lifetime
/// guard is deliberately avoided here (see the `emergency_stop` preemption
/// note there).
pub fn click_node() -> &'static SnubberNode {
    CLICK_NODE.get_or_init(|| {
        SnubberNode::builder("click_worker")
            .max_in_flight(1)
            .trip_after_failures(5)
            // The armed rollback IS `emergency_release_all_hardware`: it calls
            // `SendInput`, which can sit in UIPI, so its duration is timed and
            // `RollbackReentrancy::Refuse` stops a release that re-enters the
            // node from competing with itself for the slot.
            .rollback_budget_ms(ROLLBACK_BUDGET_MS)
            .rollback_reentrancy(RollbackReentrancy::Refuse)
            // Shared with the maintenance classifier (`classify_stall`): one
            // number per decision, so the builder and the observer cannot drift.
            // `timeout_ms` stays 0 on purpose — this node's guard is an
            // entry-time probe, so there is no live guard to reap.
            .cpu_budget_ms(CLICK_CPU_BUDGET_MS)
            .stall_ceiling_ms(CLICK_STALL_CEILING_MS)
            // A leak in a long tray session must stop new runs before it OOMs
            // the desktop; the 1 Hz sampler in `maintenance_loop` feeds this.
            .memory_limits(MEMORY_SOFT_KB, MEMORY_HARD_KB)
            .on_fault(|record, stats, _| {
                crate::debug_log_internal(
                    "error",
                    &format!(
                        "[Stability:click_worker] Fault: kind={:?} drops={} aborts={}",
                        record.kind, stats.implicit_drops, stats.explicit_aborts
                    ),
                );
            })
            .build()
    })
}

/// Click Run Node (`ExecutionEnvelope` on a settled run):
/// - Telemetry only. A run that commits with 0 clicks injected did nothing,
///   which is exactly the "RUNNING but no clicks" class of bug; the floor is
///   one payload byte (one click), so the counter names it.
/// - Wall bounds 0 BY MEASUREMENT (benches/click_pacing.rs, `cargo bench`):
///   the per-click pacing compute measured ≤ 223 ns in the worst corridor
///   (jitter 35%, outlier 0.10, 1 CPS), against a minimum legal interval of
///   6.25 ms — the run wall is sleep-dominated, so any ceiling tight enough
///   to catch a wedged run would fire on a legal 1-second sleep instead.
///   The payload floor is the detector; walls would only be noise.
/// - `trip_after_failures(0)`: an envelope miss is telemetry, never breaker
///   fuel (same law as the crate's `envelope_underflows` column).
/// - Wide bulkhead: a preempting preset spawns the next run while the old one
///   is still unwinding, and this node must never refuse either of them.
pub fn click_run_node() -> &'static SnubberNode {
    CLICK_RUN_NODE.get_or_init(|| {
        SnubberNode::builder("click_run")
            .max_in_flight(8)
            .trip_after_failures(0)
            .execution_envelope(ExecutionEnvelope::new(0, 0, 1, 0))
            .build()
    })
}

/// Hotkey Hook Node:
/// - Guards keyboard/mouse LL hook event processing
pub fn hotkey_node() -> &'static SnubberNode {
    HOTKEY_NODE.get_or_init(|| {
        SnubberNode::builder("hotkey_hook")
            .max_in_flight(1)
            .trip_after_failures(3)
            // The hook thread must never block on a release: its budget is
            // observed by the maintenance tick, and the spin/deadlock verdict
            // shares these numbers with `classify_stall`.
            .rollback_budget_ms(ROLLBACK_BUDGET_MS)
            .cpu_budget_ms(HOOK_CPU_BUDGET_MS)
            .stall_ceiling_ms(HOOK_STALL_CEILING_MS)
            .on_fault(|record, stats, _| {
                crate::debug_log_internal(
                    "error",
                    &format!(
                        "[Stability:hotkey_hook] Fault: kind={:?} drops={}",
                        record.kind, stats.implicit_drops
                    ),
                );
            })
            .build()
    })
}

/// Macro Executor Node:
/// - Guards playback of actions
pub fn macro_node() -> &'static SnubberNode {
    MACRO_NODE.get_or_init(|| {
        SnubberNode::builder("macro_executor")
            .max_in_flight(2)
            .trip_after_failures(3)
            // Macro playback releases held buttons on abort: same SendInput
            // rollback story as the click worker, same Refuse guard.
            .rollback_budget_ms(ROLLBACK_BUDGET_MS)
            .rollback_reentrancy(RollbackReentrancy::Refuse)
            .on_fault(|record, stats, _| {
                crate::debug_log_internal(
                    "error",
                    &format!(
                        "[Stability:macro_executor] Fault: kind={:?} drops={}",
                        record.kind, stats.implicit_drops
                    ),
                );
            })
            .build()
    })
}

/// Watcher Node:
/// - Guards config and macro file observation
pub fn watcher_node() -> &'static SnubberNode {
    WATCHER_NODE.get_or_init(|| {
        SnubberNode::builder("file_watcher")
            .max_in_flight(1)
            // A stalled poll is a notification lost in silence: the body is
            // timed so the 1 Hz tick can name it instead of guessing.
            .rollback_budget_ms(ROLLBACK_BUDGET_MS)
            .build()
    })
}

/// Profile Switch Node:
/// - Guards app-profile auto-switch background loop
pub fn profile_switch_node() -> &'static SnubberNode {
    PROFILE_SWITCH_NODE.get_or_init(|| snubber::StaticNode::<1, 1>::build("profile_switch"))
}

/// Planned admission refusals (`BulkheadFull`) are a normal bulkhead outcome,
/// not a failure: keep them at `warn` so a healthy burst does not read as an
/// error in the release log. Every other fault stays at `error`.
fn log_bulkhead_fault(node: &str, kind: FaultKind, implicit_drops: u64) {
    let level = if matches!(kind, FaultKind::BulkheadFull) {
        "warn"
    } else {
        "error"
    };
    crate::debug_log_internal(
        level,
        &format!("[Stability:{node}] Fault: kind={kind:?} drops={implicit_drops}"),
    );
}

/// Satellite Node (Ripple overlay + floating HUD WebViews):
/// - Max 1 in flight: serializes WebView2 create/show/destroy so a fast
///   settings-toggle / tray / boot race can never double-spawn a satellite
///   (the WebView2 double-`build()` on one label was the crash).
/// - Honest bulkhead ONLY (SATELLITE_PLAN §4): NO trip knob. Planned
///   aborts never feed the breaker streak by snubber design, so
///   `trip_after_failures(3)` here was a dead setting promising "fail
///   fast after 3" while delivering "retry forever". Rollback is real
///   (overlay::rollback_satellite cleans the half-built window).
pub fn satellite_node() -> &'static SnubberNode {
    SATELLITE_NODE.get_or_init(|| {
        SnubberNode::builder("satellite_window")
            .max_in_flight(1)
            .on_fault(|record, stats, _| {
                log_bulkhead_fault("satellite_window", record.kind, stats.implicit_drops);
            })
            .build()
    })
}

/// Config Node (config.json + engine mirror):
/// - Max 1 in flight: serializes the 5 config writers (save_app_config,
///   reset-to-defaults, tray HUD toggle, tray mode toggle, window-visibility
///   stamp) so two parallel load→save windows can never last-write-wins each
///   other's fields.
/// - Honest bulkhead ONLY, NO trip knob (same pattern as satellite_window):
///   a refused save is `Err("config busy — retry")` and the FRONT retries
///   once with a fresh collect — dedup by retry, never a backend queue.
///   Rollback is a no-op: the disk is untouched until `save()`, so an abort
///   before it is clean by construction.
pub fn config_node() -> &'static SnubberNode {
    CONFIG_NODE.get_or_init(|| {
        SnubberNode::builder("config_save")
            .max_in_flight(1)
            .on_fault(|record, stats, _| {
                log_bulkhead_fault("config_save", record.kind, stats.implicit_drops);
            })
            .build()
    })
}

/// Dump comprehensive stability ledger to logs.
/// Cold path: cached `stats()` per node, rendered outside the lock.
pub fn dump_all_stats() {
    let nodes = [
        ("click_worker", click_node()),
        ("hotkey_hook", hotkey_node()),
        ("macro_executor", macro_node()),
        ("file_watcher", watcher_node()),
        ("profile_switch", profile_switch_node()),
        ("satellite_window", satellite_node()),
        ("config_save", config_node()),
    ];

    crate::debug_log_internal("warn", "[Stability] === Snubber Diagnostics Ledger ===");
    let policy = snubber::AnomalyPolicy::default();
    for (name, node) in nodes {
        let stats = node.stats();
        crate::debug_log_internal(
            "warn",
            &format!(
                "[Stability:{name}] started={} committed={} aborts={} drops={} bulk_rejects={} rate_rejects={} stale={} breaker_trips={} poison_recoveries={} catastrophic={} shed_bulk={} shed_rate={} shed_stale={}",
                stats.total_started,
                stats.committed,
                stats.explicit_aborts,
                stats.implicit_drops,
                stats.bulkhead_rejections,
                stats.rate_rejected,
                stats.stale_epochs_rejected,
                stats.breaker_trips,
                stats.poison_recoveries,
                stats.catastrophic_faults(),
                stats.shed_bulkhead(),
                stats.shed_rate(),
                stats.shed_stale(),
            ),
        );
        // One-line verdict per node: silence when healthy, [DIAG] when not.
        let snap = node.snapshot_v1();
        for hit in snubber::find_anomalies(name, &stats, &policy) {
            crate::debug_log_internal(
                "warn",
                &format!(
                    "[Stability] {}",
                    snubber::diagnostic_line(name, &hit, &snap)
                ),
            );
        }
    }
    crate::debug_log_internal("warn", "[Stability] ===================================");
}

// ── 2b. Worker pulses (Front 4: "RUNNING, but no clicks") ───────────────────

/// Click-worker heartbeat. The worker kicks it once per loop iteration and
/// once per ≤100 ms slice of every long wait, so a frozen thread is a frozen
/// counter, not an inference from `clicks_done` (which legitimately stands
/// still between clicks).
static CLICK_PULSE: OnceLock<WorkerPulse> = OnceLock::new();
/// Hook-loop heartbeat, kicked once per `MsgWaitForMultipleObjects` iteration.
static HOOK_PULSE: OnceLock<WorkerPulse> = OnceLock::new();
/// A pulse is observed only while the thread that owns it exists: an idle
/// clicker has no worker, and its frozen counter is not a stall.
static CLICK_PULSE_ARMED: AtomicBool = AtomicBool::new(false);

pub fn click_pulse() -> &'static WorkerPulse {
    CLICK_PULSE.get_or_init(WorkerPulse::new)
}

pub fn hook_pulse() -> &'static WorkerPulse {
    HOOK_PULSE.get_or_init(WorkerPulse::new)
}

/// Arm/disarm click-pulse observation around one worker run. Disarming must
/// only happen for the CURRENT run generation (the preempting run keeps it
/// armed for itself).
pub fn arm_click_pulse(armed: bool) {
    CLICK_PULSE_ARMED.store(armed, Ordering::Release);
}

fn click_pulse_armed() -> bool {
    CLICK_PULSE_ARMED.load(Ordering::Acquire)
}

/// The hook pulse is only meaningful while the LL-hook thread exists.
#[cfg(target_os = "windows")]
fn hook_pulse_armed() -> bool {
    crate::platform::windows::hotkey_listener_running()
}

#[cfg(not(target_os = "windows"))]
fn hook_pulse_armed() -> bool {
    false
}

// ── 2c. Maintenance: rollback, pulses, stall class, memory, snapshots ──────

static MAINTENANCE_STARTED: AtomicBool = AtomicBool::new(false);

/// Start the single stability maintenance thread for this process.
///
/// Idempotent: the UI process and the daemon process each own one engine and
/// therefore each start one thread; a second call is a no-op. Cadence:
/// 200 ms pulses, 1 Hz rollback + memory, 30 s snapshots. Everything here is
/// cold — no click path calls into this module.
pub fn start_maintenance() {
    if MAINTENANCE_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("nanoclick-stability".into())
        .spawn(maintenance_loop);
    if let Err(e) = spawned {
        MAINTENANCE_STARTED.store(false, Ordering::Release);
        crate::debug_log_internal(
            "warn",
            &format!("[Stability] maintenance thread not started: {e}"),
        );
    }
}

fn maintenance_loop() {
    let mut click_watch = PulseWatch::new(CLICK_PULSE_LEASE_MS);
    let mut hook_watch = PulseWatch::new(HOOK_PULSE_LEASE_MS);
    let mut last_cpu = process_cpu_ms();
    // One line per wedged body, not one per second.
    let mut reported_rollback_epoch = [u64::MAX; 3];
    let mut tick: u64 = 0;
    loop {
        std::thread::sleep(Duration::from_millis(MAINTENANCE_TICK_MS));
        tick = tick.wrapping_add(1);
        let now = snubber::now_ms();

        // ── Pulses: a frozen counter names a frozen thread ──────────────
        if click_pulse_armed() {
            let landed = click_watch.observe(click_pulse().tick(), now);
            if !landed {
                let idle_ms = click_watch.idle_ms(now);
                let cpu_delta = cpu_delta_since(&mut last_cpu);
                let kind = classify_stall_ceiling(
                    idle_ms,
                    cpu_delta,
                    CLICK_PULSE_LEASE_MS,
                    CLICK_CPU_BUDGET_MS,
                    CLICK_STALL_CEILING_MS,
                );
                click_node().note_pulse_stall(idle_ms);
                // Only the two hard classes latch a refusal: `SleepBlocked`
                // is an OS/EDR pause below the ceiling, and refusing Start on
                // every slow driver callback would be worse than the stall.
                let hard = matches!(kind, StallKind::CpuSpin | StallKind::DeadlockStall);
                if hard && !ENGINE_STUCK.swap(true, Ordering::AcqRel) {
                    crate::debug_log_internal(
                        "error",
                        &format!("[Stability] engine_stuck latched (kind={kind:?})"),
                    );
                }
                crate::debug_log_internal(
                    "error",
                    &format!(
                        "[Stability] click_worker pulse stalled {idle_ms}ms (kind={kind:?}, cpu_delta={cpu_delta}ms)"
                    ),
                );
            } else if ENGINE_STUCK.swap(false, Ordering::AcqRel) {
                crate::debug_log_internal(
                    "warn",
                    "[Stability] engine_stuck cleared — click_worker pulse landed again",
                );
            }
        }
        if hook_pulse_armed() {
            let landed = hook_watch.observe(hook_pulse().tick(), now);
            if !landed {
                let idle_ms = hook_watch.idle_ms(now);
                let cpu_delta = cpu_delta_since(&mut last_cpu);
                let kind = classify_stall_ceiling(
                    idle_ms,
                    cpu_delta,
                    HOOK_PULSE_LEASE_MS,
                    HOOK_CPU_BUDGET_MS,
                    HOOK_STALL_CEILING_MS,
                );
                hotkey_node().note_pulse_stall(idle_ms);
                crate::debug_log_internal(
                    "error",
                    &format!(
                        "[Stability] hotkey_hook pulse stalled {idle_ms}ms (kind={kind:?}, cpu_delta={cpu_delta}ms)"
                    ),
                );
            }
        }

        // ── 1 Hz: rollback bodies + memory ─────────────────────────────
        if tick.is_multiple_of(5) {
            let watched = [
                ("click_worker", click_node()),
                ("macro_executor", macro_node()),
                ("file_watcher", watcher_node()),
            ];
            for (idx, (name, node)) in watched.iter().enumerate() {
                if let Some(stall) = snubber::watchdog::poll_rollback(node, ROLLBACK_BUDGET_MS) {
                    if reported_rollback_epoch[idx] != stall.epoch {
                        reported_rollback_epoch[idx] = stall.epoch;
                        crate::debug_log_internal(
                            "error",
                            &format!(
                                "[Stability] {name} rollback body wedged: epoch={} task={} elapsed={}ms budget={}ms",
                                stall.epoch, stall.task_id, stall.elapsed_ms, stall.budget_ms
                            ),
                        );
                    }
                }
            }

            // ── Release latch: a rollback body may still hold hardware ──
            // Sampled from the phase mirror (atomics only), not from the log
            // path above: a body that finished between reads must clear the
            // latch on this very tick, or Start stays refused forever.
            let phase = click_node().rollback_phase();
            let release_pending = phase.active && phase.elapsed_ms > ROLLBACK_BUDGET_MS;
            if release_pending != ROLLBACK_PENDING.load(Ordering::Acquire) {
                ROLLBACK_PENDING.store(release_pending, Ordering::Release);
                crate::debug_log_internal(
                    if release_pending { "error" } else { "warn" },
                    &format!(
                        "[Stability] release_pending -> {release_pending} (elapsed={}ms budget={}ms)",
                        phase.elapsed_ms, ROLLBACK_BUDGET_MS
                    ),
                );
            }

            if let Some(used_kb) = process_rss_kb() {
                let verdict = click_node().note_memory_kb(used_kb);
                let level = memory_level(verdict);
                if MEMORY_LEVEL.swap(level, Ordering::AcqRel) != level {
                    crate::debug_log_internal(
                        if verdict.is_hard() { "error" } else { "warn" },
                        &format!(
                            "[Stability] memory {} — rss={}kb soft={}kb hard={}kb",
                            verdict.code(),
                            used_kb,
                            MEMORY_SOFT_KB,
                            MEMORY_HARD_KB
                        ),
                    );
                }
            }
        }

        // ── 30 s: keep the cold snapshot files close to live ──────────
        if tick.is_multiple_of(150) {
            flush_all_snapshots(SNAPSHOT_GENERATION);
        }
    }
}

// ── 2d. Reaction policy: an observation becomes a refusal ─────────────────
// P1 taught the maintenance tick to *see* a wedged rollback body, a frozen
// pulse and memory pressure — and every verdict ended in a log line while a
// Start press still armed hardware. The latches below are the whole reaction
// surface. They are lock-free on purpose: `start_refusal()` sits on the
// toggle path (hotkey/hook thread) and must never take the node lock.

/// A rollback body has been running past `ROLLBACK_BUDGET_MS`. That body may
/// be mid-`SendInput` on a held button, so a new run would race the release.
static ROLLBACK_PENDING: AtomicBool = AtomicBool::new(false);

/// The click-worker pulse is frozen in a hard class (`CpuSpin` /
/// `DeadlockStall`). Latched by the maintenance tick, cleared when the pulse
/// lands again or when the user stops the run explicitly (see
/// `clear_engine_stuck`).
static ENGINE_STUCK: AtomicBool = AtomicBool::new(false);

/// Last memory verdict, as a rank so it fits one atomic.
static MEMORY_LEVEL: AtomicU64 = AtomicU64::new(LEVEL_OK);
const LEVEL_OK: u64 = 0;
const LEVEL_SOFT: u64 = 1;
const LEVEL_HARD: u64 = 2;

/// Rank one crate verdict. `Soft` and `Hard` stay distinct: the node refuses
/// admissions at `Soft` too, but they mean different operator actions.
fn memory_level(verdict: MemoryVerdict) -> u64 {
    if verdict.is_hard() {
        LEVEL_HARD
    } else if verdict.is_pressured() {
        LEVEL_SOFT
    } else {
        LEVEL_OK
    }
}

/// Stability verdicts for the poll surfaces (`get_status`, daemon frames).
/// Live `status-update` frames carry the override text instead (see
/// [`status_text_override`]), so they keep their existing shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct StabilityStatus {
    /// A rollback body overran its budget and has not returned yet.
    pub release_pending: bool,
    /// The worker pulse is frozen in a hard stall class.
    pub engine_stuck: bool,
    /// `"ok"` / `"soft"` / `"hard"` — `MemoryVerdict::code()` of the last RSS
    /// sample.
    pub memory: &'static str,
    /// Catastrophic faults (panic/timeout/process-died/implicit drop).
    pub catastrophic_faults: u64,
    /// Shedding columns, kept separate: `(bulkhead, rate, stale)`.
    pub shed_bulkhead: u64,
    pub shed_rate: u64,
    pub shed_stale: u64,
}

fn memory_code(level: u64) -> &'static str {
    match level {
        LEVEL_HARD => "hard",
        LEVEL_SOFT => "soft",
        _ => "ok",
    }
}

/// Pure refusal matrix (unit-tested without touching the latches).
fn refusal_from(release_pending: bool, engine_stuck: bool, memory: u64) -> Option<&'static str> {
    if release_pending {
        return Some("release_pending");
    }
    if engine_stuck {
        return Some("engine_stuck");
    }
    match memory {
        LEVEL_HARD => Some("memory_hard"),
        LEVEL_SOFT => Some("memory_soft"),
        _ => None,
    }
}

/// Pure label matrix (the texts the page shows for a latch).
fn label_from(release_pending: bool, engine_stuck: bool, memory: u64) -> Option<&'static str> {
    if release_pending {
        return Some("RELEASE_PENDING");
    }
    if engine_stuck {
        return Some("ENGINE_STUCK");
    }
    match memory {
        LEVEL_HARD => Some("MEMORY_HARD"),
        LEVEL_SOFT => Some("MEMORY_PRESSURE"),
        _ => None,
    }
}

/// Pure override matrix. Only `RUNNING` is replaced: `IDLE` is already the
/// truth for a stopped clicker, and labels like `LIMIT` / `BREAKER OPEN`
/// carry the run outcome, not engine health.
fn override_from(
    current: &str,
    release_pending: bool,
    engine_stuck: bool,
    memory: u64,
) -> Option<&'static str> {
    if current != "RUNNING" {
        return None;
    }
    label_from(release_pending, engine_stuck, memory)
}

/// Why a Start must be refused right now, or `None`.
///
/// Order is severity, not age: an unfinished release outranks a stuck engine
/// (the release owns the hardware), and both outrank memory pressure.
pub fn start_refusal() -> Option<&'static str> {
    refusal_from(
        ROLLBACK_PENDING.load(Ordering::Acquire),
        ENGINE_STUCK.load(Ordering::Acquire),
        MEMORY_LEVEL.load(Ordering::Acquire),
    )
}

/// Status text a surface must show INSTEAD of the click loop's own text.
pub fn status_text_override(current: &str) -> Option<&'static str> {
    override_from(
        current,
        ROLLBACK_PENDING.load(Ordering::Acquire),
        ENGINE_STUCK.load(Ordering::Acquire),
        MEMORY_LEVEL.load(Ordering::Acquire),
    )
}

/// Label for a refused start, usable when there is no run to override
/// (the page button, the tray toast). Same texts as the `RUNNING` override.
pub fn refusal_text() -> Option<&'static str> {
    label_from(
        ROLLBACK_PENDING.load(Ordering::Acquire),
        ENGINE_STUCK.load(Ordering::Acquire),
        MEMORY_LEVEL.load(Ordering::Acquire),
    )
}

/// Snapshot of every reaction latch (one board read for the aggregates).
pub fn status() -> StabilityStatus {
    let (shed_bulkhead, shed_rate, shed_stale) = total_hydraulic_shed();
    StabilityStatus {
        release_pending: ROLLBACK_PENDING.load(Ordering::Acquire),
        engine_stuck: ENGINE_STUCK.load(Ordering::Acquire),
        memory: memory_code(MEMORY_LEVEL.load(Ordering::Acquire)),
        catastrophic_faults: total_nuclear_breaches(),
        shed_bulkhead,
        shed_rate,
        shed_stale,
    }
}

/// Explicit Stop is the user's acknowledgement of a wedged run: the state
/// machine is back at Idle and the next Start mints a fresh worker, so the
/// latch must not outlive the run. Without this, an engine that wedged once
/// could never be started again in the same session.
pub fn clear_engine_stuck() {
    if ENGINE_STUCK.swap(false, Ordering::AcqRel) {
        crate::debug_log_internal("warn", "[Stability] engine_stuck cleared by explicit stop");
    }
}

/// CPU-time delta (ms) of this process since the previous call.
///
/// The crate's `classify_stall` wants an on-CPU sample; in-process we have
/// no per-thread handle for the worker, so the sample is process-wide. That
/// is the honest limit: a spinning worker always grows process CPU, while an
/// idle UI cannot fake growth during the same window — but a busy UI can.
/// The verdict is diagnostic aid, never a kill switch.
fn cpu_delta_since(last: &mut Option<u64>) -> u64 {
    let now = process_cpu_ms();
    let delta = match (*last, now) {
        (Some(prev), Some(cur)) => cur.saturating_sub(prev),
        _ => 0,
    };
    *last = now;
    delta
}

#[cfg(target_os = "windows")]
fn process_cpu_ms() -> Option<u64> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let (mut create, mut exit, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    let ok = unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut create,
            &mut exit,
            &mut kernel,
            &mut user,
        )
    };
    ok.ok()?;
    let ms = |ft: FILETIME| {
        ((u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)) / 10_000
    };
    Some(ms(kernel) + ms(user))
}

#[cfg(not(target_os = "windows"))]
fn process_cpu_ms() -> Option<u64> {
    None
}

#[cfg(target_os = "windows")]
fn process_rss_kb() -> Option<u64> {
    use windows::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;
    let mut pmc = PROCESS_MEMORY_COUNTERS::default();
    let cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    let ok = unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut pmc, cb) };
    ok.ok()?;
    Some(pmc.WorkingSetSize as u64 / 1024)
}

#[cfg(not(target_os = "windows"))]
fn process_rss_kb() -> Option<u64> {
    None
}

// ── 3. Task steering (TaskConduit / TaskHandle) ─────────────────────────────

/// Live telemetry published by the click worker through the steering conduit.
#[derive(Debug, Clone, Default)]
pub struct WorkerTelemetry {
    pub clicks_done: u32,
    pub cps: f64,
    pub current_point_idx: usize,
    pub stage_label: &'static str,
}

static ACTIVE_STEERING: Mutex<Option<TaskHandle<WorkerTelemetry>>> = Mutex::new(None);

/// Create a connected steering pair for one autoclicker worker run.
pub fn create_worker_steering() -> (
    TaskConduit<WorkerTelemetry, ()>,
    TaskHandle<WorkerTelemetry, ()>,
) {
    let (conduit, handle) = TaskConduit::<WorkerTelemetry, ()>::pair(WorkerTelemetry::default());
    if let Ok(mut g) = ACTIVE_STEERING.lock() {
        *g = Some(handle.clone());
    }
    (conduit, handle)
}

/// Send a steering directive (Pause, Cancel, SkipStage) to the running worker.
pub fn send_worker_directive(directive: Directive) {
    if let Ok(guard) = ACTIVE_STEERING.lock() {
        if let Some(handle) = guard.as_ref() {
            handle.send_directive(directive);
        }
    }
}

/// Read the live worker telemetry snapshot.
pub fn inspect_worker_telemetry() -> Option<WorkerTelemetry> {
    if let Ok(guard) = ACTIVE_STEERING.lock() {
        guard.as_ref().map(|h| h.inspect())
    } else {
        None
    }
}

// ── 4. Admission invariants ─────────────────────────────────────────────────

/// Validate click execution inputs before arming the click worker.
pub fn validate_click_invariants(
    cps: f64,
    x: i32,
    y: i32,
    screen_w: i32,
    screen_h: i32,
) -> Result<(), &'static str> {
    snub_check!(
        (0.1..=300.0).contains(&cps),
        "CPS outside valid range [0.1, 300.0]"
    );
    if x != 0 || y != 0 {
        snub_check!(
            (0..=screen_w).contains(&x),
            "Target X coordinate outside screen bounds"
        );
        snub_check!(
            (0..=screen_h).contains(&y),
            "Target Y coordinate outside screen bounds"
        );
    }
    Ok(())
}

// ── 5. Poison-tolerant mutex access (delegates to `snubber::SafeLockExt`) ────

/// Explicit-node lock: thin delegate over the core primitive.
///
/// Used by the click worker (`scheduler.rs`), which locks its config arcs
/// against a run-local node instead of the shared click ledger.
#[inline]
pub fn safe_lock<'a, T>(mutex: &'a Mutex<T>, node: &SnubberNode) -> std::sync::MutexGuard<'a, T> {
    snubber::SafeLockExt::safe_lock(mutex, node)
}

/// Zero-arg poison-tolerant lock bound to the click worker ledger.
///
/// This trait is the official application facade over
/// `snubber::SafeLockExt`: the 100+ hot-path call sites (`scheduler.rs`,
/// hooks, recorder) cannot name a node per lock — they share one ledger
/// (`click_node()`), the same node that owns the worker admission bulkhead.
/// Recovery semantic (from the core): poisoned mutex recovers via
/// `into_inner()` and ticks one `poison_recoveries` on the click node.
/// The poison flag is sticky — each recovery on an already-poisoned
/// mutex ticks again.
pub trait SafeLockExt<T> {
    fn safe_lock(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> SafeLockExt<T> for std::sync::Mutex<T> {
    #[inline]
    fn safe_lock(&self) -> std::sync::MutexGuard<'_, T> {
        snubber::SafeLockExt::safe_lock(self, click_node())
    }
}

// ── 6. Full state snapshot (backend → frontend reconciliation) ───────────────

/// Live rollback observability for the status surfaces: `(active, elapsed_ms)`.
///
/// One lock-free `RollbackPhase` read (atomics only, no node lock), so a 1 Hz
/// status poll can never stall behind a wedged rollback body. `elapsed_ms` is
/// 0 when no body is active or no budget is armed.
pub fn rollback_status() -> (bool, u64) {
    let phase = click_node().rollback_phase();
    (phase.active, phase.elapsed_ms)
}

pub fn is_breaker_tripped() -> bool {
    click_node().stats().breaker_trips > 0 || hotkey_node().stats().breaker_trips > 0
}

pub fn total_system_faults() -> u64 {
    let nodes = [
        click_node(),
        click_run_node(),
        hotkey_node(),
        macro_node(),
        watcher_node(),
        profile_switch_node(),
    ];
    nodes
        .iter()
        .map(|n| n.stats().implicit_drops + n.stats().breaker_trips)
        .sum()
}

// ── 6. Node aggregates (one static board, single registration site) ──────

static ALL_NODES: OnceLock<snubber::NodeSet<'static>> = OnceLock::new();

/// The single node board: all eight nodes registered once.
///
/// Every aggregate below reads this board — no per-call `register` lists,
/// no second board type. Add a new node here and it appears in every
/// ledger automatically.
pub fn all_nodes() -> &'static snubber::NodeSet<'static> {
    ALL_NODES.get_or_init(|| {
        let mut set = snubber::NodeSet::new();
        set.register("click_worker", click_node());
        set.register("click_state", click_state_node());
        set.register("click_run", click_run_node());
        set.register("hotkey_hook", hotkey_node());
        set.register("macro_executor", macro_node());
        set.register("file_watcher", watcher_node());
        set.register("profile_switch", profile_switch_node());
        set.register("satellite_window", satellite_node());
        set.register("config_save", config_node());
        set
    })
}

/// Catastrophic fault sum across worker nodes (panic/timeout/process-died/
/// implicit-drop). Non-zero means a code defect — inspect the fault ledger.
/// Public name retained for the status bar and test call sites.
pub fn total_nuclear_breaches() -> u64 {
    all_nodes().total_catastrophic_faults()
}

/// Shedding sum across worker nodes: `(bulkhead, rate, stale)` stay separate
/// columns (saturated worker vs. input spam vs. stale epoch) — never merged.
/// Public name retained for the status bar and test call sites.
pub fn total_hydraulic_shed() -> (u64, u64, u64) {
    all_nodes().total_hydraulic_shed()
}

// ── 7. Cold persistence (snapshot files) + board + diagnosis ───────────────
// The crate gives `SnapshotV1::encode() -> [u8; 512]`; THIS file owns the
// path, the tmp+rename write and the boot diagnosis. No fs in snubber.

/// Path of the cold snapshot (`%TEMP%/nanoclick_app.snub` on Windows).
pub fn cold_snapshot_path() -> std::path::PathBuf {
    std::env::temp_dir().join("nanoclick_app.snub")
}

/// Generation stamped into every frame this process writes. The supervisor
/// generation seam (`snapshot_v1_in`) is 1 today: one engine per process.
pub const SNAPSHOT_GENERATION: u64 = 1;

/// A `.snub` frame owner: its name and the constructor of its node.
pub type SnapshotNodeEntry = (&'static str, fn() -> &'static SnubberNode);

/// Every node that owns a `.snub` frame, in one place: flush, diagnosis and
/// the diagnostics board all iterate this table, so a new node is added once.
pub const SNAPSHOT_NODES: [SnapshotNodeEntry; 9] = [
    ("click_worker", click_node),
    ("click_state", click_state_node),
    ("click_run", click_run_node),
    ("hotkey_hook", hotkey_node),
    ("macro_executor", macro_node),
    ("file_watcher", watcher_node),
    ("profile_switch", profile_switch_node),
    ("satellite_window", satellite_node),
    ("config_save", config_node),
];

fn snapshot_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("nanoclick_{name}.snub"))
}

fn triage_path() -> std::path::PathBuf {
    std::env::temp_dir().join("nanoclick_triage.txt")
}

/// Flush one node's snapshot (crash-safe-ish: tmp + rename, no torn frame).
pub fn flush_node_snapshot(node: &SnubberNode, generation: u64, file_name: &str) {
    let bytes = node.snapshot_v1_in(generation).encode();
    let path = std::env::temp_dir().join(file_name);
    let tmp = path.with_extension("snub.tmp");
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Flush every node (one 512 B frame each, `nanoclick_<name>.snub`).
pub fn flush_all_snapshots(generation: u64) {
    for (name, node) in SNAPSHOT_NODES {
        flush_node_snapshot(node(), generation, &format!("nanoclick_{name}.snub"));
    }
}

/// Boot diagnosis: decode last run's frames, log one line per anomaly.
/// Returns the snapshots read, so the exit diff has a real baseline.
pub fn boot_diagnosis_from_snapshots() -> Vec<(&'static str, SnapshotV1)> {
    let policy = snubber::AnomalyPolicy::default();
    let mut previous = Vec::with_capacity(SNAPSHOT_NODES.len());
    let mut clean = true;
    for (name, _) in SNAPSHOT_NODES {
        let Ok(bytes) = std::fs::read(snapshot_path(name)) else {
            continue;
        };
        if bytes.len() != snubber::SNAPSHOT_LEN {
            crate::debug_log_internal(
                "warn",
                &format!(
                    "[Stability] torn snapshot for {name} ({} B) — ignoring",
                    bytes.len()
                ),
            );
            clean = false;
            continue;
        }
        let mut fixed = [0u8; snubber::SNAPSHOT_LEN];
        fixed.copy_from_slice(&bytes);
        match snubber::SnapshotV1::decode(&fixed) {
            Err(e) => {
                crate::debug_log_internal(
                    "warn",
                    &format!("[Stability] snapshot corrupt for {name} ({e}) — starting fresh"),
                );
                clean = false;
            }
            Ok(snap) => {
                let hits = snubber::find_anomalies_for_snapshot(name, &snap, &policy);
                for h in &hits {
                    clean = false;
                    crate::debug_log_internal(
                        "warn",
                        &format!("[Stability] {}", snubber::diagnostic_line(name, h, &snap)),
                    );
                }
                previous.push((name, snap));
            }
        }
    }
    if clean {
        crate::debug_log_internal(
            "warn",
            &format!(
                "[Stability] previous shutdown clean ({} nodes)",
                SNAPSHOT_NODES.len()
            ),
        );
    }
    previous
}

/// Boot hook: diagnose last run, then stamp this run's baseline frames.
///
/// The baseline is held in memory for the exit diff and written to disk
/// immediately, so a crash leaves a frame that belongs to THIS run instead of
/// a stale one from yesterday that the next boot would misread as a crash.
pub fn boot_snapshot_cycle() {
    // Diagnose the previous run's frames BEFORE they are overwritten below.
    let previous = boot_diagnosis_from_snapshots();
    let baseline = snapshot_pairs();
    if let Ok(mut g) = BOARD_BASELINE
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
    {
        *g = baseline;
    }
    flush_all_snapshots(SNAPSHOT_GENERATION);
    crate::debug_log_internal(
        "warn",
        &format!(
            "[Stability] boot baseline written: {} frames (previous run: {} readable)",
            SNAPSHOT_NODES.len(),
            previous.len()
        ),
    );
}

/// Snapshot of every node right now (name + 512 B frame).
fn snapshot_pairs() -> Vec<(&'static str, SnapshotV1)> {
    SNAPSHOT_NODES
        .iter()
        .map(|(name, node)| (*name, node().snapshot_v1_in(SNAPSHOT_GENERATION)))
        .collect()
}

/// Exit hook: flush final frames, then write the triage artifact.
///
/// `nanoclick_triage.txt` holds a header (schema, app version, generation,
/// wall clock) plus, per node, the crate's `write_triage` card and the
/// `SnapshotDiff` against the boot baseline — a real file, not a promise in
/// a plan document. The previous triage is kept one write back as
/// `nanoclick_triage.prev.txt`: a run that crashes BETWEEN rotate and write
/// leaves the last good file readable, and a triage from two exits ago is
/// archaeology, not evidence. Cold path: once per process exit.
///
/// The `.snub` frames themselves need no extra versioning on this side: the
/// crate stamps `SNUB` + `SNAPSHOT_VERSION` into every 512 B frame and
/// refuses anything else as `BadVersion` / `BadCrc` on read.
pub fn shutdown_snapshot_cycle() {
    flush_all_snapshots(SNAPSHOT_GENERATION);
    let baseline = BOARD_BASELINE
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default();
    let current = snapshot_pairs();
    let mut out = triage_header();
    for (name, snap) in &current {
        let _ = snubber::write_triage(&mut out, name, snap);
        if let Some((_, before)) = baseline.iter().find(|(n, _)| n == name) {
            let diff = before.diff(snap);
            if !diff.is_empty() {
                let _ = diff.write_summary(&mut out);
            }
        }
        out.push('\n');
    }
    let path = triage_path();
    rotate_triage(&path);
    match std::fs::write(&path, out.as_bytes()) {
        Ok(()) => crate::debug_log_internal(
            "warn",
            &format!("[Stability] triage written: {}", path.display()),
        ),
        Err(e) => crate::debug_log_internal(
            "warn",
            &format!("[Stability] triage write failed ({}): {e}", path.display()),
        ),
    }
}

/// Triage artifact schema version. Bump when the file layout changes in a
/// way a reader must know about (today: header + per-node cards + diffs).
pub const TRIAGE_SCHEMA_VERSION: u32 = 1;

/// First lines of the triage file. Pinned numbers only: app version, the
/// crate's snapshot schema, our generation and the wall clock (unix seconds,
/// no chrono dependency).
fn triage_header() -> String {
    let unix_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!(
        "nanoclick triage schema v{TRIAGE_SCHEMA_VERSION}\n\
         app={} snapshot_schema={} generation={SNAPSHOT_GENERATION} written_unix={unix_secs}\n\n",
        env!("CARGO_PKG_VERSION"),
        snubber::SNAPSHOT_VERSION,
    )
}

/// One-backup rotation: current -> `.prev.txt`, overwriting any stale backup.
/// Best effort — a failed rotation must not stop the fresh write.
fn rotate_triage(path: &std::path::Path) {
    let prev = path.with_extension("prev.txt");
    let _ = std::fs::remove_file(&prev);
    if std::fs::rename(path, &prev).is_ok() {
        crate::debug_log_internal(
            "warn",
            &format!("[Stability] previous triage kept: {}", prev.display()),
        );
    }
}

/// Boot baseline for the exit diff (empty when the boot hook never ran).
static BOARD_BASELINE: OnceLock<Mutex<Vec<(&'static str, SnapshotV1)>>> = OnceLock::new();

/// Explicit multi-node board for diagnostics (never a `static` inside
/// the crate — the app builds it on demand from its own handles).
pub fn board_all_nodes() -> snubber::NodeSet<'static> {
    let mut board = snubber::NodeSet::new();
    for (name, node) in SNAPSHOT_NODES {
        board.register(name, node());
    }
    board
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_refusal_precedence_release_engine_memory() {
        // No latch -> Start allowed.
        assert_eq!(refusal_from(false, false, LEVEL_OK), None);
        // The node refuses admissions at Soft too, so a Start would only
        // produce a dead run — refuse it, but name Soft separately from Hard.
        assert_eq!(refusal_from(false, false, LEVEL_SOFT), Some("memory_soft"));
        assert_eq!(refusal_from(false, false, LEVEL_HARD), Some("memory_hard"));
        // Engine stall outranks memory pressure.
        assert_eq!(refusal_from(false, true, LEVEL_HARD), Some("engine_stuck"));
        // An unfinished release outranks everything: it owns the hardware.
        assert_eq!(
            refusal_from(true, true, LEVEL_HARD),
            Some("release_pending")
        );
    }

    #[test]
    fn test_status_text_override_only_touches_running() {
        assert_eq!(override_from("RUNNING", false, false, LEVEL_OK), None);
        assert_eq!(
            override_from("RUNNING", true, false, LEVEL_OK),
            Some("RELEASE_PENDING")
        );
        assert_eq!(
            override_from("RUNNING", false, true, LEVEL_OK),
            Some("ENGINE_STUCK")
        );
        assert_eq!(
            override_from("RUNNING", false, false, LEVEL_SOFT),
            Some("MEMORY_PRESSURE")
        );
        assert_eq!(
            override_from("RUNNING", false, false, LEVEL_HARD),
            Some("MEMORY_HARD")
        );
        // Outcome labels pass through: the override must not swallow them.
        assert_eq!(override_from("IDLE", true, true, LEVEL_HARD), None);
        assert_eq!(override_from("LIMIT", true, true, LEVEL_HARD), None);
    }

    #[test]
    fn test_memory_verdict_maps_to_three_distinct_levels() {
        assert_eq!(memory_level(MemoryVerdict::Ok), LEVEL_OK);
        assert_eq!(memory_level(MemoryVerdict::Soft), LEVEL_SOFT);
        assert_eq!(memory_level(MemoryVerdict::Hard), LEVEL_HARD);
        assert_eq!(memory_code(LEVEL_OK), "ok");
        assert_eq!(memory_code(LEVEL_SOFT), "soft");
        assert_eq!(memory_code(LEVEL_HARD), "hard");
    }

    #[test]
    fn test_triage_header_pins_versions_and_generation() {
        let h = triage_header();
        assert!(h.starts_with(&format!("nanoclick triage schema v{TRIAGE_SCHEMA_VERSION}\n")));
        assert!(h.contains(&format!("app={}", env!("CARGO_PKG_VERSION"))));
        assert!(h.contains(&format!("snapshot_schema={}", snubber::SNAPSHOT_VERSION)));
        assert!(h.contains(&format!("generation={SNAPSHOT_GENERATION}")));
        assert!(h.contains("written_unix="));
    }

    #[test]
    fn test_triage_rotation_keeps_one_backup() {
        let unique = std::process::id();
        let path = std::env::temp_dir().join(format!("nanoclick_triage_test_{unique}.txt"));
        let prev = path.with_extension("prev.txt");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&prev);
        // No current file: rotation is a no-op, nothing appears.
        rotate_triage(&path);
        assert!(!prev.exists());
        // One current file: it moves to .prev.txt and the current slot is free.
        std::fs::write(&path, b"older").unwrap();
        rotate_triage(&path);
        assert!(!path.exists());
        assert_eq!(std::fs::read(&prev).unwrap(), b"older");
        // A stale backup is replaced, not duplicated.
        std::fs::write(&path, b"newer").unwrap();
        rotate_triage(&path);
        assert_eq!(std::fs::read(&prev).unwrap(), b"newer");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&prev);
    }

    #[test]
    fn test_invariants_pass_on_valid_inputs() {
        assert!(validate_click_invariants(50.0, 100, 200, 1920, 1080).is_ok());
        assert!(validate_click_invariants(1.0, 0, 0, 1920, 1080).is_ok());
    }

    #[test]
    fn test_invariants_reject_invalid_cps() {
        assert!(validate_click_invariants(0.0, 100, 200, 1920, 1080).is_err());
        assert!(validate_click_invariants(500.0, 100, 200, 1920, 1080).is_err());
    }

    #[test]
    fn test_invariants_reject_out_of_bounds_coordinates() {
        assert!(validate_click_invariants(10.0, 2500, 200, 1920, 1080).is_err());
        assert!(validate_click_invariants(10.0, 100, 2000, 1920, 1080).is_err());
    }

    #[test]
    fn test_illegal_click_moves_are_refused_before_admission() {
        use crate::scheduler::ClickerState;
        let admissions_before = click_state_node().stats().admission_total();
        let work = || Ok::<(), &'static str>(());
        // Idle -> Paused has no rib; the runtime `from` must refuse it.
        assert!(matches!(
            click_to_paused(ClickerState::Idle, || {}, work),
            Err(ClickMoveError::IllegalMove(ClickerState::Idle))
        ));
        // Running -> Running and Idle -> Idle are not edges either.
        assert!(matches!(
            click_to_running(ClickerState::Running, || {}, work),
            Err(ClickMoveError::IllegalMove(ClickerState::Running))
        ));
        assert!(matches!(
            click_to_idle(ClickerState::Idle, || {}, work),
            Err(ClickMoveError::IllegalMove(ClickerState::Idle))
        ));
        // Refusals happen before the node is touched: no phantom transactions.
        assert_eq!(
            click_state_node().stats().admission_total(),
            admissions_before,
            "illegal moves must not consume click_state admissions"
        );
    }

    #[test]
    fn test_legal_click_move_returns_work_outcome() {
        use crate::scheduler::ClickerState;
        let got = click_to_running(ClickerState::Idle, || {}, || Ok::<u8, &'static str>(7));
        assert_eq!(got.unwrap(), 7);
        let got = click_to_paused(ClickerState::Running, || {}, || Ok::<u8, &'static str>(8));
        assert_eq!(got.unwrap(), 8);
        let got = click_to_idle(ClickerState::Paused, || {}, || Ok::<u8, &'static str>(9));
        assert_eq!(got.unwrap(), 9);
    }

    #[test]
    fn test_safe_lock_recovers_poisoned_mutex() {
        let node = SnubberNode::builder("test_lock_node").build();
        let m = Arc::new(Mutex::new(42));
        let m_clone = m.clone();

        // Deliberately poison the mutex from another thread.
        let _ = std::thread::spawn(move || {
            let _lock = m_clone.lock().unwrap();
            panic!("intentional poison");
        })
        .join();

        assert!(m.is_poisoned());

        // Recovery via into_inner, one poison_recoveries tick on the node.
        let val = crate::stability::safe_lock(&*m, &node);
        assert_eq!(*val, 42);
        assert_eq!(node.stats().poison_recoveries, 1);
    }

    #[test]
    fn test_steering_conduit_lifecycle() {
        let (conduit, handle) =
            TaskConduit::<WorkerTelemetry, ()>::pair(WorkerTelemetry::default());
        assert_eq!(conduit.check_directive(), Directive::Proceed);

        handle.send_directive(Directive::Pause);
        assert_eq!(conduit.check_directive(), Directive::Pause);

        conduit.update_state(|s| {
            s.clicks_done = 100;
            s.cps = 25.0;
        });

        let snapshot = handle.inspect();
        assert_eq!(snapshot.clicks_done, 100);
        assert_eq!(snapshot.cps, 25.0);

        handle.send_directive(Directive::Cancel("user_stop"));
        assert_eq!(conduit.check_directive(), Directive::Cancel("user_stop"));
    }

    #[test]
    fn test_supervisor_hardware_token() {
        let task_id = next_task_id();
        let token = ProcessToken::simple(task_id, OP_RELEASE_ALL_INPUTS, 1);
        let bytes = token.to_bytes();
        assert_eq!(&bytes[0..4], b"SNUB");

        let parsed = ProcessToken::from_body_bytes(&bytes[4..44].try_into().unwrap());
        assert_eq!(parsed.task, task_id);
        assert_eq!(parsed.opcode, OP_RELEASE_ALL_INPUTS);
        assert_eq!(parsed.payload[0], 1);
    }

    #[test]
    fn test_catastrophic_vs_shedding_gauges_split() {
        // Catastrophic: supervisor-reported panic lands in the breach gauge.
        let nuke = SnubberNode::builder("test_catastrophic_gauge").build();
        nuke.report(FaultKind::Panic, 1, 1);
        assert_eq!(nuke.stats().catastrophic_faults(), 1);
        // Shedding: bulkhead/stale refusals never feed the catastrophic gauge.
        let shed = SnubberNode::builder("test_shed_gauge").build();
        shed.report(FaultKind::BulkheadFull, 1, 1);
        shed.report(FaultKind::StaleEpoch, 2, 2);
        let s = shed.stats();
        assert_eq!(s.catastrophic_faults(), 0);
        assert_eq!(s.shed_bulkhead(), s.bulkhead_rejections);
        assert_eq!(s.shed_stale(), s.stale_epochs_rejected);
    }

    #[test]
    fn test_snapshot_round_trips_and_board_scans() {
        let node = SnubberNode::builder("test_board_node").build();
        let g = node.enter(1, 1, || {}).expect("admit");
        g.commit();
        // Codec: encode -> decode == identity.
        let snap = node.snapshot_v1_in(7);
        assert_eq!(snap.generation, 7);
        let bytes = snap.encode();
        assert_eq!(bytes.len(), snubber::SNAPSHOT_LEN);
        let back = snubber::SnapshotV1::decode(&bytes).expect("decode own frame");
        assert_eq!(back, snap);
        // Board: explicit handles, cold scan, silence when healthy.
        let mut board = snubber::NodeSet::new();
        board.register("test_board_node", &node);
        assert_eq!(board.len(), 1);
        let hits = board.find_all_anomalies(&snubber::AnomalyPolicy::default());
        assert!(hits.is_empty(), "healthy node must not fire: {hits:?}");
    }

    #[test]
    fn test_satellite_node_serializes_overlapping_toggles() {
        // max_in_flight(1): a second toggle while one is inside must be
        // refused (BulkheadFull), never double-build a WebView2 window.
        let node = SnubberNode::builder("test_satellite_serial")
            .max_in_flight(1)
            .build();
        let first = node.enter_auto(|| {});
        assert!(first.is_ok(), "first toggle admitted");
        let second = node.enter_auto(|| {});
        assert_eq!(
            second.err(),
            Some(snubber::EnterError::BulkheadFull),
            "overlap must be refused"
        );
        // Refusal is shedding, not a catastrophic fault.
        assert_eq!(node.stats().catastrophic_faults(), 0);
        assert_eq!(node.stats().shed_bulkhead(), 1);
    }

    #[test]
    fn test_satellite_node_aborts_never_isolate() {
        // Honest bulkhead (SATELLITE_PLAN §4): planned aborts must NOT
        // isolate the node \u2014 toggles retry instead of tripping a dead
        // breaker. Bulkhead serialization still holds (see serial test).
        let node = SnubberNode::builder("test_satellite_honest")
            .max_in_flight(1)
            .build();
        for _ in 0..5 {
            let g = node.enter_auto(|| {}).expect("admit");
            g.abort("toggle failed");
        }
        let entered = node.enter_auto(|| {});
        assert!(entered.is_ok(), "aborts must not isolate: node stays open");
        assert_eq!(node.stats().breaker_trips, 0);
        assert_eq!(node.stats().explicit_aborts, 5);
    }

    #[test]
    fn test_config_node_serializes_parallel_saves() {
        // The 5 config writers race on load→save: the second parallel save
        // must be refused (BulkheadFull → "config busy — retry" on the real
        // path), never interleave. Refusal is shedding, not catastrophic.
        let node = SnubberNode::builder("test_config_serial")
            .max_in_flight(1)
            .build();
        let first = node.enter_auto(|| {});
        assert!(first.is_ok(), "first save admitted");
        let second = node.enter_auto(|| {});
        assert_eq!(
            second.err(),
            Some(snubber::EnterError::BulkheadFull),
            "parallel save must be refused, the front retries"
        );
        assert_eq!(node.stats().catastrophic_faults(), 0);
        assert_eq!(node.stats().shed_bulkhead(), 1);
    }
}
