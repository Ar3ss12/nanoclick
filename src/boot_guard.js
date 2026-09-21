// ── NanoClick BOOT GUARD — MUST STAY A CLASSIC SCRIPT ───────────────────────
//
// WHY THIS FILE EXISTS (read before moving it):
//   `main.js` is loaded as `<script type="module">`. In module mode a duplicate
//   top-level declaration (or any parse error) is FATAL: the module body never
//   executes at all, so every `window.addEventListener("error", …)` written
//   inside main.js is never installed — the window renders, and nothing works.
//   That is exactly how this project lost four debugging sessions
//   (see tech.md §Lessons Learned #1 and .notes/ROOT-CAUSE-ANALYSIS.md).
//
//   A *classic* script executes during parsing, i.e. BEFORE the deferred module
//   script, which is the only window in which a crash trap can still be armed.
//   Therefore this file must never become `type="module"` and must be loaded
//   before main.js in every page that loads a module.
//
// WHAT IT DOES
//   1. Traps uncaught errors + unhandled promise rejections and 404s of
//      scripts/styles/images, and forwards them to the Rust log
//      (`%TEMP%\nanoclick_web.log`, command `debug_log`, level "error"/"warn" —
//      the backend always writes those levels, even in release builds).
//   2. Watches for a boot handshake: the page script raises the flag named in
//      this tag's `data-boot-flag`. If the flag is still missing after
//      ~3000 ms the guard reports "MODULE DID NOT EXECUTE (SyntaxError class)"
//      and shows a visible banner — the ONLY detection that works for a dead
//      module, because nothing inside a dead module can run.
//   3. Rate-limits itself (dedupe + hard cap + truncation) so a crash loop can
//      never recreate the log flood that once OOM-killed the WebView2 children.
(function () {
  "use strict";

  if (window.__nanoclick_boot_guard_installed__) return; // idempotent per page load
  window.__nanoclick_boot_guard_installed__ = true;

  var MAX_REPORTS = 20;
  var MAX_MESSAGE = 1500;
  var BOOT_TIMEOUT_MS = 3000;
  var BANNER_ID = "nanoclick-boot-failure";

  var selfScript = document.currentScript;
  var bootFlag = attr("data-boot-flag") || "__nanoclick_boot_ok__";
  var bootLabel = attr("data-boot-label") || "main";
  var timeoutMs = parseInt(attr("data-boot-timeout-ms"), 10) || BOOT_TIMEOUT_MS;

  var reportCount = 0;
  var seen = Object.create(null);
  var suppressedReported = false;

  function attr(name) {
    try {
      return selfScript && selfScript.getAttribute ? selfScript.getAttribute(name) : null;
    } catch (_) {
      return null;
    }
  }

  function consoleError() {
    try {
      var c = window.console;
      if (c && typeof c.error === "function") c.error.apply(c, arguments);
    } catch (_) {
      /* a logger must never throw */
    }
  }

  function invokeLog(level, message) {
    try {
      var internals = window.__TAURI_INTERNALS__;
      var legacy = window.__TAURI__ && window.__TAURI__.core;
      var inv = (internals && internals.invoke) || (legacy && legacy.invoke) || null;
      if (!inv) return; // no IPC yet: the console line above is all we can do
      var pending = inv("debug_log", { level: level, message: message });
      if (pending && typeof pending.catch === "function") pending.catch(function () {});
    } catch (_) {
      /* never let reporting failure mask the original error */
    }
  }

  // Rate-limited report. Dedupe keeps a repeating error from flooding the log;
  // the hard cap is the OOM protection (ROOT-CAUSE-ANALYSIS.md §6).
  function report(level, message) {
    try {
      var text = message === null || message === undefined ? "" : String(message);
      if (text.length > MAX_MESSAGE) text = text.slice(0, MAX_MESSAGE) + " …[truncated]";
      var key = text.slice(0, 120);
      if (seen[key]) return;
      seen[key] = true;
      if (reportCount >= MAX_REPORTS) {
        if (!suppressedReported) {
          suppressedReported = true;
          consoleError("[nanoclick-boot-guard] further reports suppressed (cap " + MAX_REPORTS + ")");
        }
        return;
      }
      reportCount++;
      consoleError("[nanoclick-boot-guard]", text);
      invokeLog(level === "warn" ? "warn" : "error", "[JS] " + text);
    } catch (_) {
      /* never throw from the guard itself */
    }
  }

  // ── 1. Crash / resource traps (capture phase, so 404s are seen) ───────────
  window.addEventListener("error", function (e) {
    try {
      var target = e && e.target;
      if (target && target !== window && target.tagName) {
        var tag = String(target.tagName).toLowerCase();
        if (tag === "script" || tag === "link" || tag === "img") {
          report("error", "[JS RESOURCE FAIL] <" + tag + "> " + (target.src || target.href || "(unknown url)"));
          return;
        }
      }
      var host = (e && e.filename ? e.filename : "unknown") + ":" +
                 (e && e.lineno ? e.lineno : 0) + ":" + (e && e.colno ? e.colno : 0);
      var stack = e && e.error && e.error.stack ? "\n" + e.error.stack : "";
      report("error", "[JS CRASH] " + (e && e.message ? e.message : "Unknown error") + " at " + host + stack);
    } catch (_) {}
  }, true);

  window.addEventListener("unhandledrejection", function (e) {
    try {
      var reason = e && e.reason;
      var text = reason instanceof Error ? (reason.stack || reason.message) : String(reason);
      report("error", "[JS PROMISE REJECT] " + text);
    } catch (_) {}
  });

  // ── 2. Boot watchdog: detects a module that never executed ────────────────
  function showBanner(detail) {
    try {
      if (document.getElementById(BANNER_ID)) return;
      var el = document.createElement("div");
      el.id = BANNER_ID;
      el.setAttribute("style", [
        "position:fixed", "left:0", "right:0", "top:0", "z-index:2147483647",
        "padding:10px 14px", "font:600 13px/1.45 Consolas,monospace",
        "color:#ffd7d7", "background:#3a0d0d", "border-bottom:2px solid #ff4d4d",
        "text-align:center", "pointer-events:none", "white-space:pre-wrap"
      ].join(";"));
      el.textContent =
        "NanoClick UI failed to boot (" + bootLabel + "): " + detail +
        "\nDiagnostics: %TEMP%\\nanoclick_web.log";
      var attach = function () {
        try {
          (document.body || document.documentElement).appendChild(el);
        } catch (_) {}
      };
      if (document.readyState === "loading") {
        document.addEventListener("DOMContentLoaded", attach);
      } else {
        attach();
      }
    } catch (_) {}
  }

  setTimeout(function () {
    try {
      if (window[bootFlag]) return; // the page reported a healthy boot
      var detail = "no boot signal within " + timeoutMs + "ms";
      report("error", "[BOOT] MODULE DID NOT EXECUTE (SyntaxError class) [" + bootLabel + "] — " + detail);
      showBanner(detail);
    } catch (_) {}
  }, timeoutMs);

  // Exposed so page scripts (or `safeStep`) can push their own diagnostics
  // through the same rate-limited channel.
  window.__nanoclickBootReport = report;
})();
