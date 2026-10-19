//! # NanoClick stability layer (`snubber` integration).
//!
//! Transaction admission (bulkheads), failure classification (catastrophic
//! vs. shedding), circuit breaking, scoped rollback execution, task
//! steering (`TaskConduit` / `TaskHandle` / `Directive`) and cold
//! diagnostics (`SnapshotV1`, `NodeRegistry`, `[DIAG 0x...]` lines).
//!
//! Layering: `snubber` provides admission math, guard lifecycle and the
//! 512 B snapshot codec. This file owns the node set, the worker
//! steering handles, the snapshot file paths and the log emission.

use snubber::supervisor::ProcessToken;
use snubber::*;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

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

/// Emit hardware arming token to external supervisor if running under supervision.
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

/// Clicker State Transition Node (Phase 7):
/// - Guards ClickerState transitions (Idle <-> Running <-> Paused)
/// - Short stack-local transactions (5 us), never blocks on long-running workers.
pub fn click_state_node() -> &'static SnubberNode {
    CLICK_STATE_NODE.get_or_init(|| SnubberNode::builder("click_state").max_in_flight(8).build())
}

/// Click Worker Node:
/// - Max 1 in flight (bulkhead prevents racing worker threads)
/// - Trip breaker after 5 repeated failures in a 10s window
pub fn click_node() -> &'static SnubberNode {
    CLICK_NODE.get_or_init(|| {
        SnubberNode::builder("click_worker")
            .max_in_flight(1)
            .trip_after_failures(5)
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

/// Hotkey Hook Node:
/// - Guards keyboard/mouse LL hook event processing
pub fn hotkey_node() -> &'static SnubberNode {
    HOTKEY_NODE.get_or_init(|| {
        SnubberNode::builder("hotkey_hook")
            .max_in_flight(1)
            .trip_after_failures(3)
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
            .build()
    })
}

/// Profile Switch Node:
/// - Guards app-profile auto-switch background loop
pub fn profile_switch_node() -> &'static SnubberNode {
    PROFILE_SWITCH_NODE.get_or_init(|| {
        SnubberNode::builder("profile_switch")
            .max_in_flight(1)
            .build()
    })
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
                crate::debug_log_internal(
                    "error",
                    &format!(
                        "[Stability:satellite_window] Fault: kind={:?} drops={}",
                        record.kind, stats.implicit_drops
                    ),
                );
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
                crate::debug_log_internal(
                    "error",
                    &format!(
                        "[Stability:config_save] Fault: kind={:?} drops={}",
                        record.kind, stats.implicit_drops
                    ),
                );
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
///
/// The local `safe_lock` free function and `SafeLockExt` trait are kept as
/// thin shims over the `snubber` implementation so existing call sites keep
/// compiling. New code: `use snubber::SafeLockExt; mutex.safe_lock(node)`.
///
/// Recovery semantic: poisoned mutex recovers via `into_inner()` and ticks
/// one `poison_recoveries` on the bound node. The poison flag is sticky —
/// each recovery on an already-poisoned mutex ticks again.
pub fn safe_lock<'a, T>(mutex: &'a Mutex<T>, node: &SnubberNode) -> std::sync::MutexGuard<'a, T> {
    snubber::SafeLockExt::safe_lock(mutex, node)
}

/// Zero-arg poison-tolerant lock bound to the click worker ledger.
///
/// The 98 hot-path call sites (`scheduler.rs`, hooks, recorder) cannot name
/// a node per lock: they share one ledger (`click_node()`), the same node
/// that owns the worker admission bulkhead. Recovery semantic (from
/// `snubber::SafeLockExt`): poisoned mutex recovers via `into_inner()` and
/// ticks one `poison_recoveries` on the click node. Explicit-node form stays
/// available as `stability::safe_lock(&m, node)`.
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

/// Combined backend state in one struct: clicker, macro, breaker gauges.
/// The frontend reconciles against this on focus/restore so a backgrounded
/// WebView cannot show a stale running state.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct FullStateSnapshot {
    pub active: bool,
    pub mode: String,
    pub clicks_done: u32,
    pub cps: f64,
    pub status_text: String,
    pub breaker_tripped: bool,
    pub total_faults: u64,
    pub is_recording: bool,
    pub is_macro_running: bool,
    pub timestamp_ms: u64,
}

pub fn is_breaker_tripped() -> bool {
    click_node().stats().breaker_trips > 0 || hotkey_node().stats().breaker_trips > 0
}

pub fn total_system_faults() -> u64 {
    let nodes = [
        click_node(),
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

/// Catastrophic fault sum across worker nodes (panic/timeout/process-died/
/// implicit-drop). Non-zero means a code defect — inspect the fault ledger.
/// Name kept (`total_nuclear_breaches`) for the status-bar/test call sites.
/// Aggregation delegates to `snubber::NodeSet` (saturating in-memory sums).
pub fn total_nuclear_breaches() -> u64 {
    let mut set = snubber::NodeSet::new();
    set.register("click_worker", click_node());
    set.register("hotkey_hook", hotkey_node());
    set.register("macro_executor", macro_node());
    set.register("file_watcher", watcher_node());
    set.register("profile_switch", profile_switch_node());
    set.register("satellite_window", satellite_node());
    set.register("config_save", config_node());
    set.total_catastrophic_faults()
}

/// Shedding sum across worker nodes: `(bulkhead, rate, stale)` stay separate
/// columns (saturated worker vs. input spam vs. stale epoch) — never merged.
/// Name kept (`total_hydraulic_shed`) for the status-bar/test call sites.
/// Aggregation delegates to `snubber::NodeSet` (saturating in-memory sums).
pub fn total_hydraulic_shed() -> (u64, u64, u64) {
    let mut set = snubber::NodeSet::new();
    set.register("click_worker", click_node());
    set.register("hotkey_hook", hotkey_node());
    set.register("macro_executor", macro_node());
    set.register("file_watcher", watcher_node());
    set.register("profile_switch", profile_switch_node());
    set.register("satellite_window", satellite_node());
    set.register("config_save", config_node());
    set.total_hydraulic_shed()
}

// ── 7. Cold persistence (snapshot files) + board + diagnosis ───────────────
// The crate gives `SnapshotV1::encode() -> [u8; 512]`; THIS file owns the
// path, the tmp+rename write and the boot diagnosis. No fs in snubber.

/// Path of the cold snapshot (`%TEMP%/nanoclick_app.snub` on Windows).
pub fn cold_snapshot_path() -> std::path::PathBuf {
    std::env::temp_dir().join("nanoclick_app.snub")
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

/// Flush all seven nodes (one 512 B frame each, `<name>.snub`).
pub fn flush_all_snapshots(generation: u64) {
    let nodes = [
        ("click_worker", click_node()),
        ("hotkey_hook", hotkey_node()),
        ("macro_executor", macro_node()),
        ("file_watcher", watcher_node()),
        ("profile_switch", profile_switch_node()),
        ("satellite_window", satellite_node()),
        ("config_save", config_node()),
    ];
    for (name, node) in nodes {
        flush_node_snapshot(node, generation, &format!("nanoclick_{name}.snub"));
    }
}

/// Boot diagnosis: decode `<name>.snub` files, log one `[DIAG]` line per
/// anomaly found in the previous run. Silence when the shutdown was clean.
pub fn boot_diagnosis_from_snapshots() {
    let names = [
        "click_worker",
        "hotkey_hook",
        "macro_executor",
        "file_watcher",
        "profile_switch",
        "satellite_window",
        "config_save",
    ];
    let policy = snubber::AnomalyPolicy::default();
    let mut clean = true;
    for name in names {
        let path = std::env::temp_dir().join(format!("nanoclick_{name}.snub"));
        let Ok(bytes) = std::fs::read(&path) else {
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
                if hits.is_empty() {
                    crate::debug_log_internal(
                        "warn",
                        &format!(
                            "[Stability] previous shutdown clean for {name} (gen {})",
                            snap.generation
                        ),
                    );
                } else {
                    clean = false;
                    for h in &hits {
                        crate::debug_log_internal(
                            "warn",
                            &format!("[Stability] {}", snubber::diagnostic_line(name, h, &snap)),
                        );
                    }
                }
            }
        }
    }
    if clean {
        crate::debug_log_internal("warn", "[Stability] previous shutdown clean (all nodes)");
    }
}

/// Explicit multi-node board for diagnostics (never a `static` inside
/// the crate — the app builds it on demand from its own handles).
pub fn board_all_nodes() -> snubber::NodeRegistry<'static> {
    let mut board = snubber::NodeRegistry::new();
    board.register("click_worker", click_node());
    board.register("hotkey_hook", hotkey_node());
    board.register("macro_executor", macro_node());
    board.register("file_watcher", watcher_node());
    board.register("profile_switch", profile_switch_node());
    board.register("satellite_window", satellite_node());
    board.register("config_save", config_node());
    board
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

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
        let val = safe_lock(&m, &node);
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
        let mut board = snubber::NodeRegistry::new();
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
