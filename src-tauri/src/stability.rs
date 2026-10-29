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

use snubber::*;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
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

/// Pump threat mark. Windows removes a `WH_KEYBOARD_LL` hook whose owning
/// thread stops pumping for `LowLevelHooksTimeout` (200–1000 ms, machine
/// dependent) and tells nobody. 500 ms is inside that window, so the hook is
/// re-registered before the OS can orphan it.
pub const HOOK_PUMP_THREAT_MS: u64 = 500;

/// Admission cadence of the maintenance thread: pulses are observed at 200 ms,
/// rollback + memory at 1 Hz (every fifth tick), snapshots every 30 s.
pub const MAINTENANCE_TICK_MS: u64 = 200;

/// RSS limits for the engine node, KiB. Soft refuses new admissions while a
/// run drains; hard is the supervisor's "terminate without draining" mark.
/// A long tray session that leaks past 1 GiB is a bug, not normal load (the
/// measured idle working set is ~120 MB).
pub const MEMORY_SOFT_KB: u64 = 1_048_576;
pub const MEMORY_HARD_KB: u64 = 2_097_152;

/// One RSS sample every 30 s; ten of them span five minutes.
pub const RSS_SAMPLE_MS: u64 = 30_000;
pub const RSS_RING_LEN: usize = 10;

/// Growth over a full monotonic window that counts as a leak. The ceilings
/// above are a backstop for a process about to die; a leak is named by its
/// SHAPE (strictly rising, never resetting) once it has moved 120 MB.
pub const MEMORY_GROWTH_TRIP_KB: u64 = 122_880;

static NEXT_TASK_ID: AtomicU64 = AtomicU64::new(1);

#[inline]
pub fn next_task_id() -> u64 {
    NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed)
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

// ── 1b. Hardware arbiter (one indivisible resource) ────────────────────────
// `SendInput` is a single process-wide resource: two holders inject into the
// same cursor and neither can release the other's held button. Every path that
// arms hardware therefore has to take the SAME token, and the token has to be a
// physical gate (a node admission), not a flag someone remembers to check.
//
// Two layers, both required:
//   * one process-wide claim word (`INPUT_CLAIM`) — a click run and a macro
//     playback are admitted by different arbiters, so their exclusion has to be
//     one atomic: a `compare_exchange` decides every cross-class race, which a
//     check-then-set on separate flags cannot (it leaves a window where both
//     observe a free slot and both write their own flag);
//   * a per-arbiter `hardware_arbiter` node with `max_in_flight(1)` — the slot
//     is a HOLDING erased guard, booked for the whole run, so a second start on
//     the same owner is refused with `BulkheadFull` while the first lives.
// The claim counts click leases (the clicker's own exclusivity is the holding
// slot) and reserves one bit for the macro lease. The release itself is the
// slot's OWN armed rollback: a holding erased guard runs the body before it
// frees the lane, publishes the body's phase into the node, and catches a
// panicking body on drop. That is the whole reason there is no second guard:
// a release living outside the node made the `hardware_arbiter` row of
// `ROLLBACK_WATCHED` watch an empty closure that could never overrun.

/// Which subsystem holds the hardware. Exactly one class may be active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardOwner {
    ClickRun,
    Macro,
}

impl HardOwner {
    fn name(self) -> &'static str {
        match self {
            HardOwner::ClickRun => "click_run",
            HardOwner::Macro => "macro",
        }
    }

    /// Claim the class on the one shared word. The load and the write are a
    /// single `compare_exchange`: a rival that wins the race first is observed
    /// by this call, and a rival that loses is refused by its own.
    fn claim(self) -> Result<(), HardwareBusy> {
        loop {
            let word = INPUT_CLAIM.load(Ordering::Acquire);
            let macro_held = word & MACRO_CLAIM_BIT != 0;
            let next = match self {
                HardOwner::ClickRun => {
                    if macro_held {
                        return Err(HardwareBusy::Held { owner: HardOwner::Macro.name() });
                    }
                    if word >= CLICK_CLAIM_MAX {
                        return Err(HardwareBusy::Refused);
                    }
                    word + 1
                }
                HardOwner::Macro => {
                    if macro_held {
                        return Err(HardwareBusy::Held { owner: HardOwner::Macro.name() });
                    }
                    if word != 0 {
                        return Err(HardwareBusy::Held { owner: HardOwner::ClickRun.name() });
                    }
                    MACRO_CLAIM_BIT
                }
            };
            if INPUT_CLAIM
                .compare_exchange_weak(word, next, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                return Ok(());
            }
        }
    }

    /// Give the class back. Called from `OwnerFlag::drop`, so a panic inside a
    /// run frees the claim on the way out.
    fn release(self) {
        if self == HardOwner::Macro {
            INPUT_CLAIM.fetch_and(!MACRO_CLAIM_BIT, Ordering::Release);
        } else {
            INPUT_CLAIM.fetch_sub(1, Ordering::Release);
        }
    }
}

/// The whole cross-subsystem arbiter, in one word: low bits count live click
/// leases, the top bit is the macro lease. One `compare_exchange` on this word
/// is what makes "both owners at once" unrepresentable rather than merely
/// unlikely.
static INPUT_CLAIM: AtomicU32 = AtomicU32::new(0);
const MACRO_CLAIM_BIT: u32 = 1 << 31;
const CLICK_CLAIM_MAX: u32 = MACRO_CLAIM_BIT - 1;

/// One lease on the process-wide input hardware.
///
/// Drop order is the invariant: the holding erased guard runs its rollback —
/// `emergency_release_all_hardware` — before it frees the admission, so a
/// competitor admitted right after the drop can never inject into a
/// still-held button. The same body is what `ROLLBACK_WATCHED` times: one
/// release path, one phase, no separate guard to drift from it.
#[must_use = "dropping the token releases hardware immediately; keep it for the whole run"]
pub struct HardwareToken {
    _slot: snubber::ErasedGuard,
    _owner: OwnerFlag,
}

/// Releases the claim on every exit path, including an implicit drop after a
/// panic inside a run.
struct OwnerFlag(HardOwner);

impl Drop for OwnerFlag {
    fn drop(&mut self) {
        self.0.release();
    }
}

impl HardwareToken {
    /// Clean end of a run: disarm the slot's rollback and free it without
    /// calling `SendInput` again (the run's own epilogue already released).
    pub fn commit(self) {
        self._slot.commit();
        // `_owner` drops here, clearing the latch after the slot settles.
    }
}

/// Why a hardware acquisition was refused. `Held` names the rival class;
/// `Refused` means the ledger itself refused (bookkeeping failure).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwareBusy {
    /// The other subsystem holds the hardware right now.
    Held { owner: &'static str },
    /// The node refused for a non-bulkhead reason (bookkeeping failure).
    Refused,
}

/// Admission for one class of hardware holder.
pub struct HardwareArbiter {
    node: SnubberNode,
}

impl HardwareArbiter {
    #[must_use]
    pub fn new(node_name: &'static str) -> Self {
        Self {
            node: SnubberNode::builder(node_name)
                .max_in_flight(1)
                .trip_after_failures(0)
                .rollback_budget_ms(ROLLBACK_BUDGET_MS)
                .rollback_reentrancy(RollbackReentrancy::Refuse)
                .build(),
        }
    }

    pub fn node(&self) -> &SnubberNode {
        &self.node
    }

    /// Take the hardware lease for `owner`, or name who holds it.
    pub fn acquire(&self, owner: HardOwner) -> Result<HardwareToken, HardwareBusy> {
        // Claim first: once this returns, no rival can be admitted, so an
        // admission that follows is the only failure path and it gives the
        // claim straight back.
        owner.claim()?;
        // The armed rollback IS the release: the node's phase window covers
        // the `SendInput` calls, so a wedged release is what the 1 Hz latch
        // reads on this very node.
        let slot = match self
            .node
            .enter_erased_holding_auto(emergency_release_all_hardware)
        {
            Ok(g) => g,
            Err(EnterError::BulkheadFull) => {
                owner.release();
                return Err(HardwareBusy::Refused);
            }
            Err(e) => {
                owner.release();
                crate::debug_log_internal(
                    "error",
                    &format!("[Stability] hardware arbiter refused admission: {e:?}"),
                );
                return Err(HardwareBusy::Refused);
            }
        };
        Ok(HardwareToken {
            _slot: slot,
            _owner: OwnerFlag(owner),
        })
    }
}

/// The process-wide macro arbiter. Its node is the one the snapshot board and
/// the rollback watcher read.
pub fn macro_arbiter() -> &'static HardwareArbiter {
    static ARBITER: OnceLock<HardwareArbiter> = OnceLock::new();
    ARBITER.get_or_init(|| HardwareArbiter::new("hardware_arbiter"))
}

/// Is a macro playback holding the input hardware right now? Lock-free: the
/// hotkey thread's refusal check reads this without touching the node.
pub fn hardware_held_by_macro() -> bool {
    INPUT_CLAIM.load(Ordering::Acquire) & MACRO_CLAIM_BIT != 0
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

/// Hardware Arbiter Node:
/// - Holds the macro side's single input slot (see [`HardwareToken`]).
/// - `trip_after_failures(0)`: a breaker here would refuse input for the rest
///   of the session with no way back — the node guards a resource, not work.
/// - The rollback body is the same `SendInput` release as the workers', so its
///   duration is published and watched like theirs (`ROLLBACK_WATCHED`).
pub fn hardware_node() -> &'static SnubberNode {
    macro_arbiter().node()
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

/// TID of the thread running the current click worker (0 = none). Published by
/// the worker before it arms the pulse and cleared by the guard that owns the
/// run generation, so the sampler never measures a recycled or foreign thread.
#[cfg(target_os = "windows")]
static CLICK_WORKER_TID: AtomicU32 = AtomicU32::new(0);

/// Publish the calling thread as the click worker (worker thread only).
pub fn note_click_worker_thread() {
    #[cfg(target_os = "windows")]
    {
        let tid = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
        CLICK_WORKER_TID.store(tid, Ordering::Release);
    }
}

/// The owning run is leaving: stop sampling its thread.
pub fn clear_click_worker_thread() {
    #[cfg(target_os = "windows")]
    {
        CLICK_WORKER_TID.store(0, Ordering::Release);
    }
}

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

/// Rollback-body owners polled by the 1 Hz tick. One table, so a node that
/// publishes `rollback_budget_ms` cannot stay unread: the hook carried a
/// budget nothing observed, which is the same dead-knob class the satellite
/// node was cleaned of.
///
/// `holds_hardware` is what the release latch reads. A wedged release matters
/// only when the body that hangs is the one touching `SendInput`; before this
/// column existed the latch watched `click_worker` alone, so a wedged
/// `macro_executor` release (same body, same budget) was logged and ignored.
struct RollbackWatched {
    name: &'static str,
    node: fn() -> &'static SnubberNode,
    holds_hardware: bool,
}

const ROLLBACK_WATCHED: [RollbackWatched; 5] = [
    RollbackWatched {
        name: "click_worker",
        node: click_node,
        holds_hardware: true,
    },
    RollbackWatched {
        name: "hotkey_hook",
        node: hotkey_node,
        holds_hardware: false,
    },
    RollbackWatched {
        name: "macro_executor",
        node: macro_node,
        holds_hardware: true,
    },
    RollbackWatched {
        name: "hardware_arbiter",
        node: hardware_node,
        holds_hardware: true,
    },
    RollbackWatched {
        name: "file_watcher",
        node: watcher_node,
        holds_hardware: false,
    },
];

fn maintenance_loop() {
    let mut click_watch = PulseWatch::new(CLICK_PULSE_LEASE_MS);
    let mut hook_watch = PulseWatch::new(HOOK_PULSE_LEASE_MS);
    let mut last_cpu = process_cpu_ms();
    let mut click_cpu = ThreadCpuSampler::new();
    let mut hook_cpu = ThreadCpuSampler::new();
    let mut rss = RssWindow::new(
        MEMORY_SOFT_KB,
        MEMORY_HARD_KB,
        MEMORY_GROWTH_TRIP_KB,
        RSS_SAMPLE_MS,
    );
    // One line per wedged body, not one per second.
    let mut reported_rollback_epoch = [u64::MAX; ROLLBACK_WATCHED.len()];
    // Edge state of the input desktop: `None` until the first sample, so a
    // process that starts on a secure desktop does not fire a false edge.
    let mut desktop_was_lost: Option<bool> = None;
    let mut last_reinstall_ms: u64 = 0;
    let mut tick: u64 = 0;
    loop {
        std::thread::sleep(Duration::from_millis(MAINTENANCE_TICK_MS));
        tick = tick.wrapping_add(1);
        let now = snubber::now_ms();
        // ONE process sample per tick, shared by both stall branches. Calling
        // `cpu_delta_since` twice made the second branch read a ~0 delta and
        // silently classify the hook from nothing.
        let process_delta = cpu_delta_since(&mut last_cpu);

        // ── Pulses: a frozen counter names a frozen thread ──────────────
        if click_pulse_armed() {
            let landed = click_watch.observe(click_pulse().tick(), now);
            if !landed {
                let idle_ms = click_watch.idle_ms(now);
                // Only the worker's own CPU may turn silence into a spin; a
                // busy UI thread must not supply the delta. The process sample
                // is logged as a fallback when the thread cannot be opened,
                // but it never latches (F-2: `None` means "unknown", not "0").
                let thread_delta = click_cpu.observe();
                let cpu_delta = thread_delta.unwrap_or(process_delta);
                let kind = classify_stall_ceiling(
                    idle_ms,
                    cpu_delta,
                    CLICK_PULSE_LEASE_MS,
                    CLICK_CPU_BUDGET_MS,
                    CLICK_STALL_CEILING_MS,
                );
                click_node().note_pulse_stall(idle_ms);
                if hard_stall_latches(kind, thread_delta)
                    && !ENGINE_STUCK.swap(true, Ordering::AcqRel)
                {
                    crate::debug_log_internal(
                        "error",
                        &format!("[Stability] engine_stuck latched (kind={kind:?})"),
                    );
                }
                crate::debug_log_internal(
                    "error",
                    &format!(
                        "[Stability] click_worker pulse stalled {idle_ms}ms (kind={kind:?}, cpu_delta={cpu_delta}ms, thread_sample={})",
                        thread_delta.is_some()
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
            let idle_ms = hook_watch.idle_ms(now);
            if !landed {
                // Same thread-scoped sample as the click worker: the hook is
                // this process's only other thread that must stay live, and a
                // process-wide delta cannot tell it apart from the UI (F-3).
                let thread_delta = hook_cpu.observe_tid(hook_thread_tid());
                let cpu_delta = thread_delta.unwrap_or(process_delta);
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
                        "[Stability] hotkey_hook pulse stalled {idle_ms}ms (kind={kind:?}, cpu_delta={cpu_delta}ms, thread_sample={})",
                        thread_delta.is_some()
                    ),
                );
            }
            // ── Pump threat: re-register BEFORE the OS drops the hook ────
            // Windows silently unhooks a low-level hook whose owner stops
            // pumping for `LowLevelHooksTimeout` (200–1000 ms, machine
            // dependent). Re-registering on our own 500 ms mark turns a
            // silent deaf-hook into a bounded, logged outage.
            if idle_ms > HOOK_PUMP_THREAT_MS && now.saturating_sub(last_reinstall_ms) >= HOOK_PUMP_THREAT_MS {
                last_reinstall_ms = now;
                hotkey_node().report(snubber::FaultKind::Timeout, 1, 0);
                if request_hook_reinstall() {
                    crate::debug_log_internal(
                        "error",
                        &format!(
                            "[Stability] hotkey_hook pump frozen {idle_ms}ms — re-registering LL hooks"
                        ),
                    );
                }
            }
        }

        // ── Input desktop: a secure desktop swallows SendInput ──────────
        // Sampled only while something could be injecting: an idle tray
        // process has no buttons to unstick, and I16 forbids "just in case"
        // background reads. `desktop_was_lost` resets between runs, so a run
        // that begins on a locked/uac desktop is caught on its first sample.
        if tick.is_multiple_of(5) {
            if click_pulse_armed() || hardware_held_by_macro() {
                let lost = secure_desktop_active();
                let edge = desktop_edge(desktop_was_lost.replace(lost), lost);
                match edge {
                    DesktopEdge::Lost => {
                        crate::debug_log_internal(
                            "error",
                            "[Stability] input desktop lost (UAC / lock screen) — releasing hardware and stopping the run",
                        );
                        emergency_release_all_hardware();
                        notify_desktop_lost();
                    }
                    DesktopEdge::Restored => {
                        crate::debug_log_internal("warn", "[Stability] input desktop restored");
                    }
                    DesktopEdge::None => {}
                }
            } else {
                desktop_was_lost = None;
            }
        }

        // ── 1 Hz: rollback bodies + memory ─────────────────────────────
        if tick.is_multiple_of(5) {
            for (idx, entry) in ROLLBACK_WATCHED.iter().enumerate() {
                // The budget comes from each node's own builder, not from a
                // constant here: a row cannot be watched against a number its
                // node never armed.
                if let Some(stall) = snubber::watchdog::poll_rollback_node((entry.node)()) {
                    if reported_rollback_epoch[idx] != stall.epoch {
                        reported_rollback_epoch[idx] = stall.epoch;
                        crate::debug_log_internal(
                            "error",
                            &format!(
                                "[Stability] {} rollback body wedged: epoch={} task={} elapsed={}ms budget={}ms",
                                entry.name, stall.epoch, stall.task_id, stall.elapsed_ms, stall.budget_ms
                            ),
                        );
                    }
                }
            }

            // ── Release latch: a rollback body may still hold hardware ──
            // Sampled from the phase mirror (atomics only), not from the log
            // path above: a body that finished between reads must clear the
            // latch on this very tick, or Start stays refused forever.
            //
            // Every hardware-holding body votes: the click run and macro
            // playback run the SAME release, so watching only `click_worker`
            // left a wedged macro release logged but not refusing (F-1).
            let mut release_pending = false;
            let mut worst_elapsed = 0;
            let mut worst_budget = 0;
            for entry in ROLLBACK_WATCHED.iter().filter(|e| e.holds_hardware) {
                let phase = (entry.node)().rollback_phase();
                if phase.active && phase.elapsed_ms > phase.budget_ms {
                    release_pending = true;
                    worst_elapsed = worst_elapsed.max(phase.elapsed_ms);
                    worst_budget = worst_budget.max(phase.budget_ms);
                }
            }
            if release_pending != ROLLBACK_PENDING.load(Ordering::Acquire) {
                ROLLBACK_PENDING.store(release_pending, Ordering::Release);
                crate::debug_log_internal(
                    if release_pending { "error" } else { "warn" },
                    &format!(
                        "[Stability] release_pending -> {release_pending} (elapsed={worst_elapsed}ms budget={worst_budget}ms)"
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
                // The 1/2 GiB marks are a BACKSTOP for a process about to die
                // with the desktop. A leak has a signature long before that:
                // a strictly rising working set. Sample every 30 s and name
                // the gradient, not the ceiling.
                if tick.is_multiple_of(RSS_SAMPLE_MS / MAINTENANCE_TICK_MS) {
                    // `sample` feeds both halves; the verdict is read from
                    // `classification()` so a Soft level cannot mask a rising
                    // trend (the level stays the node's admission gate).
                    let _ = rss.sample(used_kb);
                    if let snubber::LeakClassification::Rising {
                        growth_kb,
                        rate_kb_sec,
                    } = rss.classification()
                    {
                        let rate_kb_sec = rate_kb_sec.unwrap_or(0);
                        if !MEMORY_LEAK.swap(true, Ordering::AcqRel) {
                            crate::debug_log_internal(
                                "error",
                                &format!(
                                    "[Stability] memory leak: rss grew {growth_kb}kb over {RSS_RING_LEN} samples with no drop (~{rate_kb_sec}kb/s, rss={used_kb}kb)"
                                ),
                            );
                        }
                        let hit = snubber::Anomaly {
                            kind: snubber::AnomalyKind::MemoryGrowth,
                            node: "click_worker",
                            pct: 100,
                            count: used_kb,
                        };
                        crate::debug_log_internal(
                            "warn",
                            &format!(
                                "[Stability] {}",
                                snubber::diagnostic_line(
                                    "click_worker",
                                    &hit,
                                    &click_node().snapshot_v1()
                                )
                            ),
                        );
                    } else {
                        // The verdict is the derivative, so it has to be
                        // revocable: a working set that stopped climbing
                        // (or dipped, which clears the window) is not a
                        // leak, and a permanent refusal would be a lie.
                        if MEMORY_LEAK.swap(false, Ordering::AcqRel) {
                            crate::debug_log_internal(
                                "warn",
                                "[Stability] memory leak cleared — the working set stopped climbing",
                            );
                        }
                    }
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

/// The RSS gradient detector tripped: ten consecutive 30 s samples rose, and
/// the window grew past `MEMORY_GROWTH_TRIP_KB`. Latched for the session —
/// unlike a pressure level, a leak does not clear when a run ends.
static MEMORY_LEAK: AtomicBool = AtomicBool::new(false);

/// Test-only leak latch control (the gradient needs five real minutes).
#[cfg(test)]
pub(crate) fn test_latch_memory_leak(v: bool) {
    MEMORY_LEAK.store(v, Ordering::Release);
}
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
    // A macro playback owns the only hardware slot: a click run would either
    // be refused by the arbiter or interleave `SendInput` with it.
    if hardware_held_by_macro() {
        return Some("hardware_busy");
    }
    if MEMORY_LEAK.load(Ordering::Acquire) {
        return Some("memory_leak");
    }
    refusal_from(
        ROLLBACK_PENDING.load(Ordering::Acquire),
        ENGINE_STUCK.load(Ordering::Acquire),
        MEMORY_LEVEL.load(Ordering::Acquire),
    )
}

/// Status text a surface must show INSTEAD of the click loop's own text.
pub fn status_text_override(current: &str) -> Option<&'static str> {
    if current == "RUNNING" && MEMORY_LEAK.load(Ordering::Acquire) {
        return Some("MEMORY_LEAK");
    }
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
    if hardware_held_by_macro() {
        return Some("HARDWARE_BUSY");
    }
    if MEMORY_LEAK.load(Ordering::Acquire) {
        return Some("MEMORY_LEAK");
    }
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

/// Test-only latch control: the scheduler interlock tests arm and clear
/// `ENGINE_STUCK` without a real wedged worker. Never compiled into the
/// shipped binary.
#[cfg(test)]
pub(crate) fn test_latch_engine_stuck(v: bool) {
    ENGINE_STUCK.store(v, Ordering::Release);
}

/// CPU-time delta (ms) of this process since the previous call.
///
/// Both stall branches sample their own thread first; this process-wide delta
/// is only the logged fallback when that thread cannot be opened. A spinning
/// worker always grows process CPU, but so does a busy UI in the same window —
/// which is why neither verdict latches on this number (see
/// [`hard_stall_latches`]).
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

/// Thread-scoped CPU sample, owned by one watcher per observed thread.
///
/// The process-wide sample cannot separate a spinning worker (or a wedged hook
/// pump) from a busy UI thread: once a pulse is frozen, a repainting WebView
/// supplies the CPU delta and `classify_stall_ceiling` answers `CpuSpin`. Each
/// watched thread publishes its TID; this sampler opens that thread and reads
/// `GetThreadTimes`, so only the watched thread's own CPU can justify a
/// verdict. `None` (no id, open failure, first contact) means "no sample", and
/// the caller must not latch on it.
#[cfg(target_os = "windows")]
struct ThreadCpuSampler {
    tid: u32,
    handle: Option<windows::Win32::Foundation::HANDLE>,
    last_ms: Option<u64>,
}

#[cfg(target_os = "windows")]
impl ThreadCpuSampler {
    fn new() -> Self {
        Self {
            tid: 0,
            handle: None,
            last_ms: None,
        }
    }

    fn observe(&mut self) -> Option<u64> {
        self.observe_tid(CLICK_WORKER_TID.load(Ordering::Acquire))
    }

    fn observe_tid(&mut self, tid: u32) -> Option<u64> {
        if tid == 0 {
            self.release();
            return None;
        }
        if tid != self.tid {
            self.release();
            self.tid = tid;
            self.handle = unsafe {
                windows::Win32::System::Threading::OpenThread(
                    windows::Win32::System::Threading::THREAD_QUERY_LIMITED_INFORMATION,
                    false,
                    tid,
                )
            }
            .ok();
            self.last_ms = self.handle.and_then(thread_cpu_ms);
            // First contact is a baseline, not a measurement.
            return None;
        }
        let now = thread_cpu_ms(self.handle?);
        let delta = match (self.last_ms, now) {
            (Some(prev), Some(cur)) => Some(cur.saturating_sub(prev)),
            _ => None,
        };
        self.last_ms = now;
        delta
    }

    fn release(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = unsafe { windows::Win32::Foundation::CloseHandle(handle) };
        }
        self.tid = 0;
        self.last_ms = None;
    }
}

#[cfg(target_os = "windows")]
impl Drop for ThreadCpuSampler {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(target_os = "windows")]
fn thread_cpu_ms(handle: windows::Win32::Foundation::HANDLE) -> Option<u64> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::GetThreadTimes;
    let (mut create, mut exit, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    unsafe { GetThreadTimes(handle, &mut create, &mut exit, &mut kernel, &mut user) }.ok()?;
    let ms = |ft: FILETIME| {
        ((u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)) / 10_000
    };
    Some(ms(kernel) + ms(user))
}

#[cfg(not(target_os = "windows"))]
struct ThreadCpuSampler;

#[cfg(not(target_os = "windows"))]
impl ThreadCpuSampler {
    fn new() -> Self {
        Self
    }

    fn observe(&mut self) -> Option<u64> {
        None
    }

    fn observe_tid(&mut self, _tid: u32) -> Option<u64> {
        None
    }
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

/// The verdict is a DERIVATIVE, not a level: a working set that rises on every
/// one of `RSS_RING_LEN` clean 30 s samples and never resets is a leak even at
/// 2 MB per step. One sample below its predecessor (a GC, a tray collapse)
/// clears the window — a healthy process is allowed to oscillate. The rule
/// lives in the crate (`snubber::MemoryTrend`): ring, shape, trip-mark
/// comparison and rate all come from it; the app owns only the limits, the
/// window length and the sampling interval.
type RssWindow = snubber::MemoryTrend<RSS_RING_LEN>;

/// A hard stall class may latch a refusal ONLY when this tick has a real
/// thread-scoped CPU sample. The process-wide delta is a diagnostic fallback:
/// a repainting UI can supply any growth it likes, and latching `ENGINE_STUCK`
/// on that would refuse Start for a reason nobody can reproduce (F-2).
fn hard_stall_latches(kind: StallKind, thread_sample: Option<u64>) -> bool {
    thread_sample.is_some() && matches!(kind, StallKind::CpuSpin | StallKind::DeadlockStall)
}

// ── 2d. Secure desktop watcher ─────────────────────────────────────────────

type DesktopLostHandler = Arc<dyn Fn() + Send + Sync>;

static DESKTOP_LOST_HANDLER: OnceLock<Mutex<Option<DesktopLostHandler>>> = OnceLock::new();

/// Register the reaction to losing the input desktop (UAC prompt, Win+L).
///
/// The caller is the scheduler: a secure desktop swallows `SendInput`, so the
/// run must be stopped and its held buttons released — the click thread cannot
/// see the transition itself.
pub fn set_desktop_lost_handler(handler: DesktopLostHandler) {
    if let Ok(mut slot) = DESKTOP_LOST_HANDLER
        .get_or_init(|| Mutex::new(None))
        .lock()
    {
        *slot = Some(handler);
    }
}

fn notify_desktop_lost() {
    let handler = DESKTOP_LOST_HANDLER
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()
        .and_then(|g| g.clone());
    if let Some(handler) = handler {
        // The maintenance thread is the only thing watching every other
        // subsystem: a panicking handler must not take it down with the loop.
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler())).is_err() {
            crate::debug_log_internal(
                "error",
                "[Stability] desktop-lost handler panicked; the run may still hold buttons",
            );
        }
    }
}

/// What changed about the input desktop since the previous sample. Pure, so
/// the reaction is testable without a UAC prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DesktopEdge {
    None,
    Lost,
    Restored,
}

fn desktop_edge(previous: Option<bool>, lost: bool) -> DesktopEdge {
    if !lost {
        return match previous {
            Some(true) => DesktopEdge::Restored,
            _ => DesktopEdge::None,
        };
    }
    // A persistent lock screen fires once, not every second. The first sample
    // of a run does fire: a run that somehow starts while the desktop is
    // secure has buttons it can no longer release.
    match previous {
        Some(true) => DesktopEdge::None,
        Some(false) | None => DesktopEdge::Lost,
    }
}

/// Is the thread's input desktop unreachable because another desktop (UAC,
/// lock screen, Ctrl+Alt+Del) owns input?
///
/// `OpenInputDesktop` succeeds on the normal desktop and fails with
/// `ACCESS_DENIED` on a secure one — a process can never open the secure
/// desktop. Any other failure is reported as "unknown" so a transient resource
/// error cannot stop a healthy run.
#[cfg(target_os = "windows")]
fn secure_desktop_active() -> bool {
    use windows::Win32::System::StationsAndDesktops::{
        CloseDesktop, OpenInputDesktop, DESKTOP_ACCESS_FLAGS, DESKTOP_READOBJECTS,
        DESKTOP_SWITCHDESKTOP,
    };
    let opened = unsafe {
        OpenInputDesktop(
            windows::Win32::System::StationsAndDesktops::DESKTOP_CONTROL_FLAGS::default(),
            false,
            DESKTOP_ACCESS_FLAGS(DESKTOP_READOBJECTS.0 | DESKTOP_SWITCHDESKTOP.0),
        )
    };
    match opened {
        Ok(handle) => {
            let _ = unsafe { CloseDesktop(handle) };
            false
        }
        // `from_win32` folds the Win32 code into an HRESULT; the facility bits
        // are not part of the error identity.
        Err(e) => (e.code().0 as u32 & 0xFFFF) == windows::Win32::Foundation::ERROR_ACCESS_DENIED.0,
    }
}

#[cfg(not(target_os = "windows"))]
fn secure_desktop_active() -> bool {
    false
}

/// TID of the LL-hook thread (0 when the listener is not running). Sampled by
/// the maintenance tick so a frozen hook is classified on ITS CPU, not the
/// process total.
#[cfg(target_os = "windows")]
fn hook_thread_tid() -> u32 {
    crate::platform::windows::hook_thread_id()
}

#[cfg(not(target_os = "windows"))]
fn hook_thread_tid() -> u32 {
    0
}

/// Ask the hook thread to unhook and re-install its LL hooks. Returns `false`
/// when no hook thread exists.
#[cfg(target_os = "windows")]
fn request_hook_reinstall() -> bool {
    crate::platform::windows::request_hook_reinstall()
}

#[cfg(not(target_os = "windows"))]
fn request_hook_reinstall() -> bool {
    false
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
        set.register("hardware_arbiter", hardware_node());
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
///
/// FROZEN NAME: pinned by `include_str!` tripwires and by the status surface.
/// The vocabulary is the retired reactor metaphor (agents.md §5), kept because
/// renaming it rewrites stored tripwire strings, not because it describes
/// anything. New code says "catastrophic faults".
pub fn total_nuclear_breaches() -> u64 {
    all_nodes().total_catastrophic_faults()
}

/// Shedding sum across worker nodes: `(bulkhead, rate, stale)` stay separate
/// columns (saturated worker vs. input spam vs. stale epoch) — never merged.
///
/// FROZEN NAME: same reason as [`total_nuclear_breaches`] — tripwire-pinned
/// legacy vocabulary. The columns are shed counts, and that is what callers
/// mean.
pub fn total_hydraulic_shed() -> (u64, u64, u64) {
    all_nodes().total_hydraulic_shed()
}

// ── 7. Cold persistence (snapshot files) + board + diagnosis ───────────────
// The crate gives `SnapshotV1::encode() -> [u8; 512]`; THIS file owns the
// path, the tmp+rename write and the boot diagnosis. No fs in snubber.

/// Role of this process in the shipped topology: `"ui"` for a daemon-owned UI
/// child, `"daemon"` for the engine process, `"app"` for a standalone run.
/// The role is part of every cold-evidence file name: in the daemon topology
/// both processes run a maintenance thread, and an unscoped name let the
/// long-lived daemon overwrite the frames of the UI process that had just
/// crashed — destroying the evidence exactly when it was needed.
pub fn process_role() -> &'static str {
    static ROLE: OnceLock<&'static str> = OnceLock::new();
    ROLE.get_or_init(|| {
        let args: Vec<String> = std::env::args().collect();
        let stem = std::env::current_exe()
            .ok()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()));
        process_role_from(&args, stem.as_deref())
    })
}

fn process_role_from(args: &[String], exe_stem: Option<&str>) -> &'static str {
    if crate::daemon_supervise::is_ui_child(args) {
        "ui"
    } else if exe_stem == Some("nanoclick-daemon") {
        "daemon"
    } else {
        "app"
    }
}

fn snapshot_file_name(role: &str, name: &str) -> String {
    format!("nanoclick_{role}_{name}.snub")
}

fn triage_file_name(role: &str) -> String {
    format!("nanoclick_triage_{role}.txt")
}

/// Generation stamped into every frame this process writes. The supervisor
/// generation seam (`snapshot_v1_in`) is 1 today: one engine per process.
pub const SNAPSHOT_GENERATION: u64 = 1;

/// A `.snub` frame owner: its name and the constructor of its node.
pub type SnapshotNodeEntry = (&'static str, fn() -> &'static SnubberNode);

/// Every node that owns a `.snub` frame, in one place: flush, diagnosis and
/// the diagnostics board all iterate this table, so a new node is added once.
pub const SNAPSHOT_NODES: [SnapshotNodeEntry; 10] = [
    ("click_worker", click_node),
    ("click_state", click_state_node),
    ("click_run", click_run_node),
    ("hardware_arbiter", hardware_node),
    ("hotkey_hook", hotkey_node),
    ("macro_executor", macro_node),
    ("file_watcher", watcher_node),
    ("profile_switch", profile_switch_node),
    ("satellite_window", satellite_node),
    ("config_save", config_node),
];

fn snapshot_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(snapshot_file_name(process_role(), name))
}

fn triage_path() -> std::path::PathBuf {
    std::env::temp_dir().join(triage_file_name(process_role()))
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

/// Flush every node (one 512 B frame each, `nanoclick_<role>_<name>.snub`).
pub fn flush_all_snapshots(generation: u64) {
    let role = process_role();
    for (name, node) in SNAPSHOT_NODES {
        flush_node_snapshot(node(), generation, &snapshot_file_name(role, name));
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
/// `nanoclick_triage_<role>.txt` holds a header (schema, app version, generation,
/// wall clock) plus, per node, the crate's `write_triage` card and the
/// `SnapshotDiff` against the boot baseline — a real file, not a promise in
/// a plan document. The previous triage is kept one write back as
/// `nanoclick_triage_<role>.prev.txt`: a run that crashes BETWEEN rotate and write
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// The arbiter is the only admission path to hardware: two leases at once
    /// must be impossible, and the claim it publishes is what the lock-free
    /// refusal surface reads.
    ///
    /// Serialized on the scheduler's toggle gate because the claim word is
    /// process-wide and every `set_active(true)` test takes it too.
    #[test]
    fn test_hardware_arbiter_admits_one_lease_and_names_the_holder() {
        let _gate = crate::scheduler::test_toggle_gate().lock();
        let click = HardwareArbiter::new("test_click_hardware");
        // A click run may still be unwinding from the previous test's stop.
        let mut first = None;
        for _ in 0..50 {
            if let Ok(token) = macro_arbiter().acquire(HardOwner::Macro) {
                first = Some(token);
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let first = first.expect("a free arbiter must admit");
        assert!(hardware_held_by_macro());
        assert_eq!(start_refusal(), Some("hardware_busy"));
        assert_eq!(refusal_text(), Some("HARDWARE_BUSY"));
        // Physical gate: no second slot, whoever asks.
        assert!(matches!(
            click.acquire(HardOwner::ClickRun),
            Err(HardwareBusy::Held { owner: "macro" })
        ));
        first.commit();
        assert!(!hardware_held_by_macro());
        assert_eq!(start_refusal(), None);
        // The latch came home: the next run can take it.
        let second = click.acquire(HardOwner::ClickRun).expect("slot freed by commit");
        second.commit();
    }

    /// The cross-class exclusion is one word and one CAS. Two flags read and
    /// then written separately leave a window where both owners observe a free
    /// slot and both claim it — the exact interleaving the arbiter exists to
    /// make impossible.
    #[test]
    fn the_input_claim_is_a_single_indivisible_word() {
        let src = include_str!("stability.rs");
        assert!(
            src.contains("static INPUT_CLAIM: AtomicU32 = AtomicU32::new(0);"),
            "the claim must live in one atomic word"
        );
        // Split so this assertion's own text is not the match it looks for.
        let old_click = concat!("CLICK_RUN", "_ACTIVE");
        let old_macro = concat!("MACRO_RUN", "_ACTIVE");
        assert!(
            !src.contains(old_click) && !src.contains(old_macro),
            "two class booleans are the check-then-set design: a rival can be \
             admitted between the load and the store"
        );

        let start = src.find("    fn claim(self)").expect("claim not found");
        let end = src[start..]
            .find("    fn release(self)")
            .map(|i| i + start)
            .expect("end of claim not found");
        let body = &src[start..end];
        assert!(
            body.contains("compare_exchange_weak") && !body.contains(".store("),
            "the claim decision and the claim write must be the same instruction"
        );

        let acquire = src
            .find("    pub fn acquire(&self, owner: HardOwner)")
            .expect("acquire not found");
        let claim = src[acquire..]
            .find("owner.claim()?")
            .map(|i| i + acquire)
            .expect("acquire must claim the word");
        let admit = src[acquire..]
            .find("enter_erased_holding_auto")
            .map(|i| i + acquire)
            .expect("acquire must take a node slot");
        assert!(
            claim < admit,
            "the claim precedes the admission, so a slot refusal cannot leave \
             a rival a window between the two"
        );
    }

    #[test]
    fn test_memory_leak_latch_refuses_a_start_and_names_the_reason() {
        // Global latch: serialized against every other test that reads the
        // reaction surface through the same gate the scheduler tests take.
        let _gate = crate::scheduler::test_toggle_gate().lock();
        test_latch_memory_leak(true);
        assert_eq!(start_refusal(), Some("memory_leak"));
        assert_eq!(refusal_text(), Some("MEMORY_LEAK"));
        assert_eq!(status_text_override("RUNNING"), Some("MEMORY_LEAK"));
        // IDLE keeps its own text: a leak is a start guard, not a run label.
        assert_eq!(status_text_override("IDLE"), None);
        test_latch_memory_leak(false);
        assert_eq!(start_refusal(), None);
        assert_eq!(refusal_text(), None);
        assert_eq!(status_text_override("RUNNING"), None);
    }

    /// The app's real limits and cadence, with the trip mark under test.
    fn trend_with_trip(trip_kb: u64) -> RssWindow {
        RssWindow::new(MEMORY_SOFT_KB, MEMORY_HARD_KB, trip_kb, RSS_SAMPLE_MS)
    }

    #[test]
    fn test_rss_gradient_names_only_a_full_monotonic_window() {
        // Trip mark of 1 KiB so the window SHAPE is what the test reads.
        let mut ring = trend_with_trip(1);
        // A short window is not an answer, however steep.
        for kb in [100u64, 200, 300, 400, 500, 600, 700, 800, 900] {
            let _ = ring.sample(kb);
            assert!(!ring.classification().is_rising());
        }
        // Ten rising samples: growth is newest - oldest.
        let _ = ring.sample(1_000);
        assert!(
            matches!(
                ring.classification(),
                snubber::LeakClassification::Rising {
                    growth_kb: 900,
                    ..
                }
            ),
            "ten rising samples name the gradient: {:?}",
            ring.classification()
        );
        // A single dip clears the window — a healthy process may oscillate.
        let _ = ring.sample(950);
        assert!(!ring.classification().is_rising());
        assert!(matches!(
            ring.classification(),
            snubber::LeakClassification::Sawtooth
        ));
    }

    #[test]
    fn test_memory_leak_trip_mark_is_a_growth_not_a_level() {
        // The user-visible contract: 2 MB per step is monotone but not yet a
        // leak verdict; the mark is 120 MB of growth, not 120 MB of RSS.
        assert_eq!(MEMORY_GROWTH_TRIP_KB, 120 * 1024);
        // Ten 2 MB steps: 18 MB of growth is a shape, not a verdict.
        let mut gentle = trend_with_trip(MEMORY_GROWTH_TRIP_KB);
        for step in 1..=RSS_RING_LEN {
            let _ = gentle.sample(step as u64 * 2 * 1024);
        }
        assert!(
            !gentle.classification().is_rising(),
            "monotone but under the mark: {:?}",
            gentle.classification()
        );
        // Ten 15 MB steps cross the mark on the tenth sample (135 MB).
        let mut steep = trend_with_trip(MEMORY_GROWTH_TRIP_KB);
        for step in 1..=RSS_RING_LEN {
            let _ = steep.sample(step as u64 * 15 * 1024);
        }
        assert!(
            matches!(
                steep.classification(),
                snubber::LeakClassification::Rising {
                    growth_kb,
                    rate_kb_sec: Some(rate),
                } if growth_kb == 135 * 1024 && rate > 0
            ),
            "growth must trip with a rate: {:?}",
            steep.classification()
        );
    }

    #[test]
    fn test_hard_stall_never_latches_without_a_thread_sample() {
        assert!(!hard_stall_latches(StallKind::CpuSpin, None));
        assert!(!hard_stall_latches(StallKind::DeadlockStall, None));
        assert!(hard_stall_latches(StallKind::CpuSpin, Some(0)));
        assert!(hard_stall_latches(StallKind::DeadlockStall, Some(0)));
        // Slow but alive: neither hard class.
        assert!(!hard_stall_latches(StallKind::SleepBlocked, Some(500)));
    }

    #[test]
    fn test_desktop_edge_fires_once_per_transition() {
        // A run's first sample counts: an already-secure desktop must stop it.
        assert_eq!(desktop_edge(None, true), DesktopEdge::Lost);
        assert_eq!(desktop_edge(None, false), DesktopEdge::None);
        assert_eq!(desktop_edge(Some(false), true), DesktopEdge::Lost);
        assert_eq!(desktop_edge(Some(true), true), DesktopEdge::None);
        assert_eq!(desktop_edge(Some(true), false), DesktopEdge::Restored);
        assert_eq!(desktop_edge(Some(false), false), DesktopEdge::None);
    }

    #[test]
    fn test_every_hardware_holder_is_watched_for_a_wedged_release() {
        let holders: Vec<&str> = ROLLBACK_WATCHED
            .iter()
            .filter(|e| e.holds_hardware)
            .map(|e| e.name)
            .collect();
        // A wedged release on either body races a new run for the same buttons.
        assert!(holders.contains(&"click_worker"), "{holders:?}");
        assert!(holders.contains(&"macro_executor"), "{holders:?}");
        assert!(holders.contains(&"hardware_arbiter"), "{holders:?}");
        // The hook's rollback only unhooks; it never touches `SendInput`.
        assert!(!holders.contains(&"hotkey_hook"), "{holders:?}");
    }

    #[test]
    fn test_arbiter_watch_times_the_release_body() {
        let src = include_str!("stability.rs");
        let start = src
            .find("pub fn acquire(&self, owner: HardOwner)")
            .expect("acquire exists");
        let end = src[start..]
            .find("/// The process-wide macro arbiter")
            .map(|i| i + start)
            .expect("end of acquire");
        let body = &src[start..end];
        // The watched node must be the one that runs `SendInput`. An empty
        // rollback here is a latch that can never fire, which is how the
        // `hardware_arbiter` row spent 1.5.0 watching a `|| {}`.
        assert!(
            body.contains("enter_erased_holding_auto(emergency_release_all_hardware)"),
            "the arbiter slot must arm the release body"
        );
        assert!(!body.contains("|| {}"), "an empty rollback cannot overrun");
        // No second release path: a guard outside the node owns no phase, so
        // the latch would time a body nobody runs.
        let protected = concat!("snubber::", "Protected");
        let standalone = concat!("snubber::Snubber", "Guard");
        assert!(!src.contains(protected), "the token owns no second release guard");
        assert!(!src.contains(standalone), "the token owns no second release guard");
    }

    #[test]
    fn test_desktop_loss_stops_every_input_owner() {
        let scheduler = include_str!("scheduler.rs");
        let start = scheduler
            .find("pub fn stop_for_desktop_loss")
            .expect("stop_for_desktop_loss exists");
        let end = scheduler[start..]
            .find("/// Emergency stop")
            .map(|i| i + start)
            .expect("end of stop_for_desktop_loss");
        let body = &scheduler[start..end];
        // A secure desktop is silence on every input front: the click run and
        // the macro playback share one hardware claim, so stopping one of them
        // left the other holding `MACRO_CLAIM_BIT` against a dead desktop.
        assert!(
            body.contains("crate::core::global()"),
            "the macro playback must be stopped too: {body}"
        );
        assert!(body.contains("exec.stop()"), "the executor stop must run: {body}");
        assert!(body.contains("set_active(false"), "the click run must be stopped: {body}");
    }

    #[test]
    fn test_hardware_arbiter_is_the_only_input_path() {
        let scheduler = include_str!("scheduler.rs");
        let executor = include_str!("core/executor.rs");
        // The click run takes the token before it spawns a worker...
        let start = scheduler
            .find("pub fn set_active")
            .expect("set_active exists");
        let acquire = scheduler[start..]
            .find(".acquire(crate::stability::HardOwner::ClickRun)")
            .expect("set_active must take the hardware token");
        let spawn = scheduler[start..]
            .find("self.spawn_worker")
            .expect("set_active spawns the worker");
        assert!(
            acquire < spawn,
            "the token must be taken before the worker can click"
        );
        // ...and macro playback takes the same resource, so the two can never
        // hold the hardware at once.
        assert!(executor.contains("macro_arbiter()"));
        assert!(executor.contains("HardOwner::Macro"));
        // Neither side may bypass the arbiter with a raw node admission.
        assert!(!scheduler.contains("hardware_node().enter"));
        assert!(!executor.contains("hardware_node().enter"));
        // The inert supervisor seam is gone: it emitted nothing without a
        // snubber parent and read as a guarantee.
        assert!(!scheduler.contains("arm_hardware_token"));
        assert!(!executor.contains("arm_hardware_token"));
    }

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
    fn test_process_role_covers_the_three_topologies() {
        let app = ["nanoclick.exe".to_string()];
        assert_eq!(process_role_from(&app, Some("nanoclick")), "app");
        let daemon = ["nanoclick-daemon.exe".to_string()];
        assert_eq!(
            process_role_from(&daemon, Some("nanoclick-daemon")),
            "daemon"
        );
        // The UI child is the same exe as the daemon; the flag is the only
        // evidence of its role.
        let ui = ["nanoclick-daemon.exe".to_string(), "--ui-child".to_string()];
        assert_eq!(process_role_from(&ui, Some("nanoclick-daemon")), "ui");
    }

    #[test]
    fn test_cold_evidence_paths_cannot_collide_across_roles() {
        for (name, _) in SNAPSHOT_NODES {
            let app = snapshot_file_name("app", name);
            let daemon = snapshot_file_name("daemon", name);
            let ui = snapshot_file_name("ui", name);
            assert_eq!(app, format!("nanoclick_app_{name}.snub"));
            assert_ne!(app, daemon);
            assert_ne!(daemon, ui);
            assert_ne!(app, ui);
        }
        assert_eq!(triage_file_name("app"), "nanoclick_triage_app.txt");
        assert_ne!(triage_file_name("daemon"), triage_file_name("ui"));
        // The live paths resolve through the same scoped names in `%TEMP%`.
        assert_eq!(
            snapshot_path("click_worker"),
            std::env::temp_dir().join(snapshot_file_name(process_role(), "click_worker"))
        );
        assert_eq!(
            triage_path(),
            std::env::temp_dir().join(triage_file_name(process_role()))
        );
    }

    #[test]
    fn test_click_worker_publishes_its_thread_for_the_cpu_sample() {
        let src = include_str!("scheduler.rs");
        assert!(src.contains("note_click_worker_thread()"));
        assert!(src.contains("clear_click_worker_thread()"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_worker_cpu_sampler_measures_the_thread_not_the_process() {
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel();
        let spinner = std::thread::spawn(move || {
            let tid = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
            tx.send(tid).unwrap();
            let until = std::time::Instant::now() + Duration::from_millis(200);
            while std::time::Instant::now() < until {
                std::hint::spin_loop();
            }
        });
        let tid = rx.recv().unwrap();
        let mut sampler = ThreadCpuSampler::new();
        // No published worker: no sample, so the caller falls back to process.
        assert_eq!(sampler.observe_tid(0), None);
        // First contact is a baseline, not a delta.
        assert_eq!(sampler.observe_tid(tid), None);
        std::thread::sleep(Duration::from_millis(120));
        let delta = sampler
            .observe_tid(tid)
            .expect("a live thread must be sampled once the baseline exists");
        assert!(delta > 0, "a spinning thread must show CPU growth");
        spinner.join().unwrap();
        // Worker gone: the handle is released, never sampled again.
        assert_eq!(sampler.observe_tid(0), None);
        assert!(sampler.handle.is_none());
    }

    #[test]
    fn test_every_published_rollback_budget_has_a_watcher() {
        let watched: Vec<&str> = ROLLBACK_WATCHED.iter().map(|e| e.name).collect();
        // The hook runs `shutdown_global_hotkey_listener` as its rollback body;
        // its budget used to be published and never read.
        assert!(watched.contains(&"hotkey_hook"));
        for (name, node) in SNAPSHOT_NODES {
            let budget = node().rollback_phase().budget_ms;
            assert!(
                budget == 0 || watched.contains(&name),
                "{name} publishes rollback_budget_ms={budget} that no tick observes"
            );
        }
    }

    #[test]
    fn test_status_aggregates_match_the_board() {
        let st = status();
        assert_eq!(st.catastrophic_faults, total_nuclear_breaches());
        let (bulkhead, rate, stale) = total_hydraulic_shed();
        assert_eq!((st.shed_bulkhead, st.shed_rate, st.shed_stale), (bulkhead, rate, stale));
        assert!(matches!(st.memory, "ok" | "soft" | "hard"));
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
    fn test_inert_supervisor_token_path_stays_buried() {
        // F-5: `ProcessToken::emit_if_supervised` writes only when the process
        // runs under `SNUBBER_SUPERVISED`, so on a normal desktop it released
        // nothing — `input_guardian` is the actual rescue for a held button.
        // The arbiter owns the hardware now; a resurrected token arm would be a
        // second, silently inert owner.
        let lib = include_str!("lib.rs");
        let sched = include_str!("scheduler.rs");
        let exec = include_str!("core/executor.rs");
        for (name, src) in [("lib.rs", lib), ("scheduler.rs", sched), ("executor.rs", exec)] {
            assert!(
                !src.contains("ProcessToken"),
                "{name} must not touch the supervisor token: it is inert outside a supervised run"
            );
            assert!(
                !src.contains("arm_hardware_token"),
                "{name} must not rely on the inert arm; acquire_hardware owns the hardware"
            );
        }
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
