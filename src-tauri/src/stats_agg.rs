//! Session stats aggregator — the counting half of `stats.js`.
//!
//! The page kept run/session/all-time counters, the exactly-once finalize,
//! the junk-run filter and the 50-entry ring in JS, fed by the 66 ms
//! telemetry ticks. That logic is pure arithmetic over `(active,
//! clicks_done, cps, now_ms)` — no DOM, no Canvas — so it lives here now as
//! a dependency-free state machine. The page keeps only the Canvas chart and
//! the DOM paint. `StatsConfig` (totals + history) stays the persisted shape.

use crate::config_manager::{StatHistoryPoint, StatsConfig};

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
}

/// What the tick wants the caller to persist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatsFlush {
    /// Nothing worth writing (idle echo, no delta).
    None,
    /// Totals changed — schedule the throttled config save.
    Save,
}

impl SessionStats {
    pub fn new() -> Self {
        Self {
            last_finalized_clicks: -1,
            ..Self::default()
        }
    }

    /// Fold one telemetry tick into session counters + persisted totals.
    /// Pure function of the arguments — no clock reads, no I/O.
    pub fn tick(
        &mut self,
        totals: &mut StatsConfig,
        active: bool,
        clicks_done: u64,
        cps: f64,
        now_ms: u64,
    ) -> StatsFlush {
        let cur_cps = if cps.is_finite() && cps > 0.0 { cps } else { 0.0 };
        if active {
            if !self.active_now {
                self.active_now = true;
                self.run_clicks = 0;
                self.last_clicks_done = 0;
                self.last_update_ms = Some(now_ms);
                totals.total_sessions = totals.total_sessions.saturating_add(1);
                return StatsFlush::Save;
            }
            let mut flush = StatsFlush::None;
            if clicks_done > self.last_clicks_done {
                let delta = clicks_done - self.last_clicks_done;
                self.last_clicks_done = clicks_done;
                self.run_clicks = self.run_clicks.saturating_add(delta);
                self.session_clicks = self.session_clicks.saturating_add(delta);
                totals.total_clicks = totals.total_clicks.saturating_add(delta);
                flush = StatsFlush::Save;
            }
            if let Some(last) = self.last_update_ms {
                if now_ms > last {
                    let delta_ms = now_ms - last;
                    if delta_ms < 5_000 {
                        self.session_active_ms =
                            self.session_active_ms.saturating_add(delta_ms);
                        totals.total_active_ms =
                            totals.total_active_ms.saturating_add(delta_ms);
                    }
                }
            }
            self.last_update_ms = Some(now_ms);
            if cur_cps > totals.max_cps {
                totals.max_cps = (cur_cps * 10.0).round() / 10.0;
                flush = StatsFlush::Save;
            }
            flush
        } else {
            // Late echo after the finalize already ran — pure no-op.
            if !self.active_now {
                return StatsFlush::None;
            }
            if clicks_done > self.last_clicks_done {
                let delta = clicks_done - self.last_clicks_done;
                self.run_clicks = self.run_clicks.saturating_add(delta);
                self.session_clicks = self.session_clicks.saturating_add(delta);
                totals.total_clicks = totals.total_clicks.saturating_add(delta);
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
                totals.history.push(StatHistoryPoint {
                    timestamp: now_ms,
                    clicks: self.run_clicks,
                    active_ms: self.session_active_ms,
                    avg_cps: (avg * 10.0).round() / 10.0,
                });
                if totals.history.len() > 50 {
                    let overflow = totals.history.len() - 50;
                    totals.history.drain(..overflow);
                }
                self.last_finalized_clicks = clicks_done as i64;
            }
            StatsFlush::Save
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finalize_runs_exactly_once_for_late_echoes() {
        let mut s = SessionStats::new();
        let mut totals = StatsConfig::default();
        assert_eq!(s.tick(&mut totals, true, 0, 10.0, 1_000), StatsFlush::Save);
        assert_eq!(s.tick(&mut totals, true, 100, 10.0, 2_000), StatsFlush::Save);
        assert_eq!(s.tick(&mut totals, false, 100, 0.0, 3_000), StatsFlush::Save);
        assert_eq!(totals.history.len(), 1);
        // Late 66 ms worker echo — must be a no-op, never a twin entry.
        assert_eq!(s.tick(&mut totals, false, 101, 0.0, 3_040), StatsFlush::None);
        assert_eq!(totals.history.len(), 1);
    }

    #[test]
    fn junk_runs_grow_counters_but_not_history() {
        let mut s = SessionStats::new();
        let mut totals = StatsConfig::default();
        s.tick(&mut totals, true, 0, 5.0, 1_000);
        s.tick(&mut totals, true, 3, 5.0, 1_500);
        s.tick(&mut totals, false, 3, 0.0, 1_800);
        assert_eq!(totals.total_clicks, 3);
        assert!(totals.history.is_empty(), "junk tap must not pollute the chart");
    }

    #[test]
    fn history_ring_never_exceeds_fifty() {
        let mut totals = StatsConfig::default();
        for run in 0..60u64 {
            let mut s = SessionStats::new();
            let base = run * 10_000;
            s.tick(&mut totals, true, 0, 10.0, base);
            s.tick(&mut totals, true, 10, 10.0, base + 2_000);
            s.tick(&mut totals, false, 10, 0.0, base + 3_000);
        }
        assert_eq!(totals.history.len(), 50);
    }

    #[test]
    fn delta_guards_against_click_counter_rewind() {
        let mut s = SessionStats::new();
        let mut totals = StatsConfig::default();
        s.tick(&mut totals, true, 0, 10.0, 1_000);
        s.tick(&mut totals, true, 100, 10.0, 2_000);
        // Counter rewound — never subtract.
        assert_eq!(s.tick(&mut totals, true, 10, 10.0, 3_000), StatsFlush::None);
        assert_eq!(totals.total_clicks, 100);
    }
}

