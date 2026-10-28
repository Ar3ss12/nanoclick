//! Session stats aggregator — the counting half of `stats.js`.
//!
//! The page kept run/session/all-time counters, the exactly-once finalize,
//! the junk-run filter and the 50-entry ring in JS, fed by the 66 ms
//! telemetry ticks. That logic is pure arithmetic over `(active,
//! clicks_done, cps, now_ms)` — no DOM, no Canvas — so it lives here now as
//! a dependency-free state machine. The page keeps only the Canvas chart and
//! the DOM paint. `StatsConfig` (totals + history) stays the persisted shape.
//!
//! `stats_tick` is the hot path: the page calls it on every telemetry tick
//! (~15/s while a run is alive). Totals therefore live HERE, exact at all
//! times, and a disk flush is coalesced — run start and run end always
//! persist, mid-run deltas at most once per `STATS_PERSIST_INTERVAL_MS`. The
//! previous shape (reload config from disk + save per tick) wrote config.json,
//! stats.json and the LKG snapshot ~15x/s during a run and is what lagged
//! the interface while the autoclicker was enabled.

use crate::config_manager::{StatHistoryPoint, StatsConfig};

/// Mid-run flush cadence in milliseconds. Mid-run counters stay exact in
/// memory; only the disk write waits. A crash mid-run loses at most this
/// window of totals, never the finalized run row.
const STATS_PERSIST_INTERVAL_MS: u64 = 2_000;

/// Live counters for the current process session (never persisted as-is).
#[derive(Debug, Clone, Default)]
pub struct SessionStats {
    pub session_clicks: u64,
    pub run_clicks: u64,
    pub session_active_ms: u64,
    pub active_now: bool,
    pub last_clicks_done: u64,
    pub last_update_ms: Option<u64>,
    pub last_finalized_clicks: i64,
    /// Always-exact mirror of the persisted totals. Kept here (not reloaded
    /// from disk per tick) so the 66 ms hot path performs no I/O at all.
    pub totals: StatsConfig,
    /// Stamp of the last tick that returned `StatsFlush::Save`.
    last_persist_ms: Option<u64>,
}

/// What the tick wants the caller to persist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatsFlush {
    /// Nothing worth writing (idle echo, no delta, or inside the coalescing window).
    None,
    /// Totals changed — persist them now.
    Save,
}

impl SessionStats {
    pub fn new() -> Self {
        Self {
            last_finalized_clicks: -1,
            ..Self::default()
        }
    }

    /// Boot constructor: inherit the totals restored from disk so the first
    /// tick continues the all-time counters instead of resetting them.
    pub fn with_totals(totals: StatsConfig) -> Self {
        Self {
            totals,
            last_finalized_clicks: -1,
            ..Self::default()
        }
    }

    fn persist_due(&self, now_ms: u64) -> bool {
        self.last_persist_ms
            .is_none_or(|t| now_ms.saturating_sub(t) >= STATS_PERSIST_INTERVAL_MS)
    }

    fn persist(&mut self, now_ms: u64) -> StatsFlush {
        self.last_persist_ms = Some(now_ms);
        StatsFlush::Save
    }

    /// Fold one telemetry tick into session counters + the totals mirror.
    /// Pure function of the arguments — no clock reads, no I/O. Returns
    /// `Save` when the caller should write the totals to disk: run start,
    /// run end (the finalize), or a mid-run delta outside the coalescing
    /// window. Counters are updated on EVERY tick regardless of the answer.
    pub fn tick(&mut self, active: bool, clicks_done: u64, cps: f64, now_ms: u64) -> StatsFlush {
        let cur_cps = if cps.is_finite() && cps > 0.0 {
            cps
        } else {
            0.0
        };
        if active {
            if !self.active_now {
                self.active_now = true;
                self.run_clicks = 0;
                self.last_clicks_done = 0;
                self.last_update_ms = Some(now_ms);
                self.totals.total_sessions = self.totals.total_sessions.saturating_add(1);
                return self.persist(now_ms);
            }
            let mut changed = false;
            if clicks_done > self.last_clicks_done {
                let delta = clicks_done - self.last_clicks_done;
                self.last_clicks_done = clicks_done;
                self.run_clicks = self.run_clicks.saturating_add(delta);
                self.session_clicks = self.session_clicks.saturating_add(delta);
                self.totals.total_clicks = self.totals.total_clicks.saturating_add(delta);
                changed = true;
            }
            if let Some(last) = self.last_update_ms {
                if now_ms > last {
                    let delta_ms = now_ms - last;
                    if delta_ms < 5_000 {
                        self.session_active_ms = self.session_active_ms.saturating_add(delta_ms);
                        self.totals.total_active_ms =
                            self.totals.total_active_ms.saturating_add(delta_ms);
                    }
                }
            }
            self.last_update_ms = Some(now_ms);
            if cur_cps > self.totals.max_cps {
                self.totals.max_cps = (cur_cps * 10.0).round() / 10.0;
                changed = true;
            }
            // Mid-run coalescing: the counters above are exact in memory;
            // only the disk write waits for the interval.
            if changed && self.persist_due(now_ms) {
                self.persist(now_ms)
            } else {
                StatsFlush::None
            }
        } else {
            // Late echo after the finalize already ran — pure no-op.
            if !self.active_now {
                return StatsFlush::None;
            }
            if clicks_done > self.last_clicks_done {
                let delta = clicks_done - self.last_clicks_done;
                self.run_clicks = self.run_clicks.saturating_add(delta);
                self.session_clicks = self.session_clicks.saturating_add(delta);
                self.totals.total_clicks = self.totals.total_clicks.saturating_add(delta);
            }
            self.active_now = false;
            self.last_update_ms = None;
            self.last_clicks_done = 0;
            // Junk filter: accidental taps grow counters, never the chart.
            let is_junk = self.run_clicks < 5 && self.session_active_ms < 1_000;
            if !is_junk && (self.run_clicks > 0 || self.session_active_ms > 1_000) {
                let avg = if self.session_active_ms > 0 {
                    self.run_clicks as f64 / (self.session_active_ms as f64 / 1000.0)
                } else {
                    0.0
                };
                self.totals.history.push(StatHistoryPoint {
                    timestamp: now_ms,
                    clicks: self.run_clicks,
                    active_ms: self.session_active_ms,
                    avg_cps: (avg * 10.0).round() / 10.0,
                });
                if self.totals.history.len() > 50 {
                    let overflow = self.totals.history.len() - 50;
                    self.totals.history.drain(..overflow);
                }
                self.last_finalized_clicks = clicks_done as i64;
            }
            // Run end: the row the user checks right after Stop — never wait.
            self.persist(now_ms)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finalize_runs_exactly_once_for_late_echoes() {
        let mut s = SessionStats::new();
        assert_eq!(s.tick(true, 0, 10.0, 1_000), StatsFlush::Save);
        // Mid-run delta inside the coalescing window: no disk write.
        assert_eq!(s.tick(true, 100, 10.0, 2_000), StatsFlush::None);
        assert_eq!(s.totals.total_clicks, 100, "counters stay exact without a flush");
        assert_eq!(s.tick(false, 100, 0.0, 3_000), StatsFlush::Save);
        assert_eq!(s.totals.history.len(), 1);
        // Late 66 ms worker echo — must be a no-op, never a twin entry.
        assert_eq!(s.tick(false, 101, 0.0, 3_040), StatsFlush::None);
        assert_eq!(s.totals.history.len(), 1);
    }

    #[test]
    fn junk_runs_grow_counters_but_not_history() {
        let mut s = SessionStats::new();
        s.tick(true, 0, 5.0, 1_000);
        s.tick(true, 3, 5.0, 1_500);
        s.tick(false, 3, 0.0, 1_800);
        assert_eq!(s.totals.total_clicks, 3);
        assert!(
            s.totals.history.is_empty(),
            "junk tap must not pollute the chart"
        );
    }

    #[test]
    fn history_ring_never_exceeds_fifty() {
        // Single aggregator, many runs — how the app actually accumulates.
        let mut s = SessionStats::new();
        for run in 0..60u64 {
            let base = run * 10_000;
            s.tick(true, 0, 10.0, base);
            s.tick(true, 10, 10.0, base + 2_000);
            s.tick(false, 10, 0.0, base + 3_000);
        }
        assert_eq!(s.totals.history.len(), 50);
    }

    #[test]
    fn delta_guards_against_click_counter_rewind() {
        let mut s = SessionStats::new();
        s.tick(true, 0, 10.0, 1_000);
        s.tick(true, 100, 10.0, 2_000);
        // Counter rewound — never subtract, and no change means no flush.
        assert_eq!(s.tick(true, 10, 10.0, 3_000), StatsFlush::None);
        assert_eq!(s.totals.total_clicks, 100);
    }

    #[test]
    fn mid_run_ticks_coalesce_disk_writes() {
        let mut s = SessionStats::new();
        assert_eq!(s.tick(true, 0, 20.0, 1_000), StatsFlush::Save);
        // 20 ticks at the real 66 ms cadence, +2 clicks each: the exact
        // shape of a live run. All land inside the coalescing window.
        for i in 1..=20u64 {
            let now = 1_000 + i * 66;
            assert_eq!(
                s.tick(true, i * 2, 20.0, now),
                StatsFlush::None,
                "tick {i} must coalesce"
            );
        }
        assert_eq!(
            s.totals.total_clicks, 40,
            "in-memory totals stay exact while the disk write waits"
        );
        // 2.1 s after the last persist the accumulated deltas flush at once.
        assert_eq!(s.tick(true, 42, 20.0, 3_100), StatsFlush::Save);
        assert_eq!(s.totals.total_clicks, 42);
    }

    #[test]
    fn run_end_flushes_pending_deltas_immediately() {
        let mut s = SessionStats::new();
        s.tick(true, 0, 20.0, 1_000);
        s.tick(true, 50, 20.0, 1_500); // coalesced mid-run
        assert_eq!(s.totals.total_clicks, 50);
        assert_eq!(s.tick(false, 50, 0.0, 1_600), StatsFlush::Save);
        assert_eq!(s.totals.total_clicks, 50);
        assert_eq!(s.totals.history.len(), 1, "finalized run row is written");
    }

    #[test]
    fn boot_totals_are_inherited_not_reset() {
        let mut totals = StatsConfig::default();
        totals.total_clicks = 1234;
        totals.total_sessions = 7;
        let mut s = SessionStats::with_totals(totals);
        s.tick(true, 0, 10.0, 1_000); // first run of the session
        assert_eq!(s.totals.total_sessions, 8);
        s.tick(true, 6, 10.0, 1_100);
        assert_eq!(s.totals.total_clicks, 1240);
    }
}
