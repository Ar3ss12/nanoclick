// Frontend tests for src/stats.js — the page half of the 66 ms telemetry
// hot path. While a run is alive the page calls StatsEngine.recordSessionTick
// ~15x/s: these tests pin the contract that keeps that cheap — one stats_tick
// IPC per call, totals/session mirroring, the 1 Hz / 60-point CPS ring, the
// 4 Hz render throttle, and the offline (no-IPC) fallback.
import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, "../..");
const statsJsPath = path.join(repoRoot, "src", "stats.js");

function loadStatsEngine({ offline = false } = {}) {
  const statsTickCalls = [];
  let clockMs = 1_000;
  let perfMs = 0;

  const store = new Map();
  const localStorage = {
    getItem: (k) => (store.has(k) ? store.get(k) : null),
    setItem: (k, v) => store.set(k, String(v)),
    removeItem: (k) => store.delete(k),
  };

  const window = {
    devicePixelRatio: 1,
    __TAURI__: offline
      ? undefined
      : {
          core: {
            invoke: (cmd, args) => {
              statsTickCalls.push({ cmd, args });
              if (cmd !== "stats_tick") return Promise.resolve(null);
              return Promise.resolve({
                saved: true,
                totals: { total_clicks: 999, total_sessions: 9, max_cps: 23.6 },
                session: {
                  session_clicks: 42,
                  run_clicks: 7,
                  session_active_ms: 3_500,
                  active_now: true,
                },
              });
            },
          },
        },
  };

  // Counting element: every renderStats pass writes sessionClicks once.
  const scEl = {
    _writes: 0,
    set textContent(v) {
      this._writes += 1;
    },
    get textContent() {
      return "";
    },
  };

  const elements = new Map();
  elements.set("statSessionClicks", scEl);
  const document = {
    getElementById: (id) => elements.get(id) || null,
  };

  const context = vm.createContext({
    window,
    document,
    localStorage,
    console: { log() {}, warn() {}, error() {} },
    Date: { now: () => clockMs },
    performance: { now: () => perfMs },
    JSON,
    Math,
    Number,
    String,
    Array,
    Boolean,
    Object,
    Promise,
  });
  vm.runInContext(fs.readFileSync(statsJsPath, "utf-8"), context);

  return {
    engine: window.StatsEngine,
    statsTickCalls,
    scWrites: () => scEl._writes,
    advanceClock: (ms) => {
      clockMs += ms;
    },
    advancePerf: (ms) => {
      perfMs += ms;
    },
    setViewStatsActive: () => {
      elements.set("viewStats", {
        classList: { contains: (c) => c === "active", add() {}, remove() {} },
      });
    },
  };
}

test("recordSessionTick: one stats_tick IPC per call; totals and session mirror into state", async () => {
  const h = loadStatsEngine();
  const config = { stats: {} };
  await Promise.resolve(h.engine.recordSessionTick(true, 10, 23.6, config));

  assert.equal(h.statsTickCalls.length, 1, "exactly one IPC per telemetry tick");
  const call = h.statsTickCalls[0];
  assert.equal(call.cmd, "stats_tick");
  assert.equal(call.args.active, true);
  assert.equal(call.args.clicksDone, 10);
  assert.equal(call.args.cps, 23.6);
  assert.equal(call.args.nowMs, 1_000, "nowMs comes from Date.now()");

  assert.ok(config.stats && config.stats.total_clicks === 999,
    "the page config receives the backend totals object");
  assert.equal(h.engine.state.sessionClicks, 42);
  assert.equal(h.engine.state.runClicks, 7);
  assert.equal(h.engine.state.sessionActiveMs, 3_500);
  assert.equal(h.engine.state.activeNow, true);
});

test("recordCpsHistoryPoint: 1 Hz sampling, ring caps at 60 points", () => {
  const h = loadStatsEngine();
  h.engine.recordCpsHistoryPoint(10, true);
  // Same-second samples are dropped once the ring is non-empty.
  h.engine.recordCpsHistoryPoint(10, true);
  assert.equal(h.engine.state.liveCpsHistory.length, 1);

  for (let i = 1; i <= 70; i++) {
    h.advanceClock(1_000);
    h.engine.recordCpsHistoryPoint(10, true);
  }
  assert.equal(h.engine.state.liveCpsHistory.length, 60,
    "the rolling window for the Canvas chart never grows past 60");
});

test("renderStats is throttled to 4 Hz when ticks arrive at the 66 ms cadence", async () => {
  const h = loadStatsEngine();
  h.setViewStatsActive();
  const config = { stats: {} };

  // 10 live-run ticks at the real 66 ms cadence (660 ms of wall time):
  // renders are allowed only at >= 250 ms since the previous one — at
  // perf 264 ms and 528 ms, i.e. exactly 2 DOM paints, not 10.
  for (let i = 0; i < 10; i++) {
    h.advancePerf(66);
    await Promise.resolve(h.engine.recordSessionTick(true, i, 20, config));
  }
  assert.equal(h.scWrites(), 2, "15 Hz ticks must not translate into 15 Hz DOM writes");
});

test("offline path (no __TAURI__): tick still records and never throws", async () => {
  const h = loadStatsEngine({ offline: true });
  const config = { stats: {} };
  await Promise.resolve(h.engine.recordSessionTick(true, 5, 10, config));

  assert.equal(h.statsTickCalls.length, 0, "no IPC attempted without the bridge");
  assert.equal(h.engine.state.liveCpsHistory.length, 1,
    "CPS ring still records for the chart");
});

test("formatters: thousands separators and duration units", () => {
  const h = loadStatsEngine();
  assert.equal(h.engine.fmtNum(999), "999");
  assert.equal(h.engine.fmtNum(1_000), "1,000");
  assert.equal(h.engine.fmtNum(1_250_000), "1,250,000");
  assert.equal(h.engine.fmtDuration(59_000), "59s");
  assert.equal(h.engine.fmtDuration(60_000), "1m 0s");
  assert.equal(h.engine.fmtDuration(3_723_000), "1h 2m");
});
