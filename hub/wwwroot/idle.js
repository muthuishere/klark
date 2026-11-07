// Idle pause: after IDLE_MS with no mouse, touch, key or scroll, release the
// server circuit (Blazor.pauseCircuit), so every poll and pane stream stops.
// On return, say how old the page is, then reload; view state lives in
// local/sessionStorage, so the reload lands where the person left off.
(() => {
  const IDLE_MS = 60_000;
  const EVENTS = ["mousemove", "pointerdown", "keydown", "wheel", "touchstart", "scroll"];
  let last = Date.now(), paused = 0, banner = null, ticker = null;

  const ago = ms => {
    const s = Math.max(1, Math.round(ms / 1000));
    if (s < 60) return `${s} second${s === 1 ? "" : "s"} ago`;
    const m = Math.round(s / 60);
    if (m < 60) return `${m} minute${m === 1 ? "" : "s"} ago`;
    const h = Math.round(m / 60);
    return `${h} hour${h === 1 ? "" : "s"} ago`;
  };

  const show = text => {
    if (!banner) {
      banner = document.createElement("div");
      banner.className = "idle-banner";
      banner.setAttribute("role", "status");
      document.body.appendChild(banner);
    }
    banner.textContent = text;
  };

  const live = () => !!window.Blazor && typeof Blazor.pauseCircuit === "function"
    && !location.pathname.startsWith("/login");

  async function pause() {
    if (paused || !live()) return;
    paused = last;
    document.documentElement.classList.add("klark-paused");
    show(`Paused while you're away · last updated ${ago(Date.now() - paused)}`);
    ticker = setInterval(() => show(`Paused while you're away · last updated ${ago(Date.now() - paused)}`), 10_000);
    try { await Blazor.pauseCircuit(); } catch { /* already gone: the reload below still recovers */ }
  }

  function wake() {
    last = Date.now();
    if (!paused) return;
    const age = Date.now() - paused;
    paused = 0;
    clearInterval(ticker);
    EVENTS.forEach(e => removeEventListener(e, wake, true));
    show(`Last updated ${ago(age)} · refreshing…`);
    setTimeout(() => location.reload(), 400);
  }

  EVENTS.forEach(e => addEventListener(e, wake, { capture: true, passive: true }));
  document.addEventListener("visibilitychange", () => { if (!document.hidden) wake(); });
  setInterval(() => { if (!paused && Date.now() - last >= IDLE_MS) pause(); }, 5_000);
  window.klarkIdle = { pause, wake, ago };
})();
