// NanoClick floating HUD — listens for click-count updates from the engine.
const { listen } = window.__TAURI__.event;

const el = document.getElementById("hud");

listen("hud-clicks", (event) => {
  const n = Number(event.payload) || 0;
  el.textContent = n.toLocaleString();
});

listen("hud-hide", () => {
  // Parent closes the window itself; this is just a safety net.
  document.getElementById("hud").style.opacity = "0";
});
