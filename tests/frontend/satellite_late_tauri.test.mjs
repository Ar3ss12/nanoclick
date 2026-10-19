import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(__dirname, "../..");
const overlayJsPath = path.join(repoRoot, "src", "overlay.js");
const hudJsPath = path.join(repoRoot, "src", "hud.js");

function createMockEnvironment() {
  const listeners = new Map();
  const timeouts = [];
  let currentTime = 0;

  const mockCtx = {
    setTransform: () => {},
    clearRect: () => {},
    beginPath: () => {},
    arc: () => {},
    fill: () => {},
    stroke: () => {},
    fillStyle: "",
    strokeStyle: "",
    lineWidth: 1,
  };

  const mockCanvas = {
    getContext: () => mockCtx,
    width: 1920,
    height: 1080,
  };

  const mockHudEl = {
    textContent: "",
  };

  const window = {
    innerWidth: 1920,
    innerHeight: 1080,
    devicePixelRatio: 1,
    addEventListener: (event, handler) => {
      listeners.set(event, handler);
    },
    removeEventListener: () => {},
    setTimeout: (fn, delay) => {
      const id = timeouts.length + 1;
      timeouts.push({ id, fn, triggerAt: currentTime + delay, executed: false });
      return id;
    },
    clearTimeout: (id) => {
      const item = timeouts.find((t) => t.id === id);
      if (item) item.executed = true;
    },
    requestAnimationFrame: (fn) => {
      return window.setTimeout(() => fn(currentTime), 16);
    },
    console: {
      warn: () => {},
      log: () => {},
      error: () => {},
    },
    __TAURI__: undefined,
  };

  const document = {
    readyState: "complete",
    addEventListener: (event, handler) => {
      listeners.set(event, handler);
    },
    getElementById: (id) => {
      if (id === "rippleCanvas") return mockCanvas;
      if (id === "hud") return mockHudEl;
      return null;
    },
  };

  function advanceTimers(ms) {
    currentTime += ms;
    // Process due timers in order
    for (const item of timeouts) {
      if (!item.executed && item.triggerAt <= currentTime) {
        item.executed = true;
        item.fn();
      }
    }
  }

  return { window, document, mockCanvas, mockHudEl, advanceTimers };
}

test("Level 1: overlay.js waits for late window.__TAURI__ injection without crashing", () => {
  const env = createMockEnvironment();
  const code = fs.readFileSync(overlayJsPath, "utf-8");

  const context = vm.createContext({
    window: env.window,
    document: env.document,
    setTimeout: env.window.setTimeout,
    clearTimeout: env.window.clearTimeout,
    requestAnimationFrame: env.window.requestAnimationFrame,
    console: env.window.console,
    performance: { now: () => Date.now() },
    Number,
    Math,
    Array,
    String,
    Boolean,
    Date,
  });

  // Execute overlay.js with __TAURI__ === undefined
  vm.runInContext(code, context);

  assert.equal(env.window.__nanoclick_overlay_boot_ok__, true, "boot guard flag must be set");

  // Advance 120 ms (2 retry ticks: 50ms each)
  env.advanceTimers(120);

  // Now inject __TAURI__ late
  const listenCalls = [];
  const invokeCalls = [];
  let rippleListener = null;

  env.window.__TAURI__ = {
    event: {
      listen: (name, cb) => {
        listenCalls.push(name);
        if (name === "spawn-ripple") {
          rippleListener = cb;
        }
        return Promise.resolve(() => {});
      },
    },
    core: {
      invoke: (cmd) => {
        invokeCalls.push(cmd);
        return Promise.resolve();
      },
    },
  };

  // Advance another 60 ms to allow the next retry tick to discover __TAURI__
  env.advanceTimers(60);

  assert.equal(listenCalls.length, 1, "listen() should be called once");
  assert.equal(listenCalls[0], "spawn-ripple", "must listen to spawn-ripple");
  assert.equal(invokeCalls.length, 1, "invoke() should be called once");
  assert.equal(invokeCalls[0], "overlay_ready", "must invoke overlay_ready to eliminate white flash");

  // Test payload handling: batch array [[x1, y1], [x2, y2]]
  assert.equal(typeof rippleListener, "function", "spawn-ripple listener registered");
  assert.doesNotThrow(() => {
    rippleListener({ payload: [[100, 200], [300, 400]] });
    rippleListener({ payload: [500, 600] });
    rippleListener({ payload: { x: 700, y: 800 } });
  }, "rippleListener should parse batch arrays, single tuples, and object payloads safely");
});

test("Level 1: hud.js waits for late window.__TAURI__ injection and updates click counter", () => {
  const env = createMockEnvironment();
  const code = fs.readFileSync(hudJsPath, "utf-8");

  const context = vm.createContext({
    window: env.window,
    document: env.document,
    setTimeout: env.window.setTimeout,
    clearTimeout: env.window.clearTimeout,
    requestAnimationFrame: env.window.requestAnimationFrame,
    console: env.window.console,
    Number,
    Math,
    Array,
    String,
    Boolean,
    Date,
  });

  // Execute hud.js with __TAURI__ === undefined
  vm.runInContext(code, context);

  assert.equal(env.window.__nanoclick_hud_boot_ok__, true, "boot guard flag must be set");

  // Advance 100 ms
  env.advanceTimers(100);

  // Inject __TAURI__
  const listenCalls = [];
  const invokeCalls = [];
  let clicksListener = null;

  env.window.__TAURI__ = {
    event: {
      listen: (name, cb) => {
        listenCalls.push(name);
        if (name === "hud-clicks") {
          clicksListener = cb;
        }
        return Promise.resolve(() => {});
      },
    },
    core: {
      invoke: (cmd) => {
        invokeCalls.push(cmd);
        return Promise.resolve();
      },
    },
  };

  // Advance 60 ms
  env.advanceTimers(60);

  assert.equal(listenCalls.length, 1, "hud listen() should be called once");
  assert.equal(listenCalls[0], "hud-clicks", "must listen to hud-clicks");
  assert.equal(invokeCalls.length, 1, "hud invoke() should be called once");
  assert.equal(invokeCalls[0], "hud_ready", "must invoke hud_ready to show window");

  // Verify click update formatting
  assert.equal(typeof clicksListener, "function", "hud-clicks listener registered");
  clicksListener({ payload: 42 });
  assert.equal(env.mockHudEl.textContent, "42");

  clicksListener({ payload: 1250000 });
  assert.equal(env.mockHudEl.textContent, "1,250,000", "large counts should have comma separators");
});
