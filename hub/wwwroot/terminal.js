// xterm.js glue for the pane view (ADR 0011, C). The terminal always has the
// pane's real size (pane_size from the machine) and is never fit() to the
// window: it is scaled down with CSS to the container's width, and scrolls
// sideways below a minimum scale, so TUIs keep their exact layout.
window.klarkTerm = (() => {
  const terms = new Map();
  const MIN_SCALE = 0.55;

  function decode(b64) {
    const bin = atob(b64);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    return bytes;
  }

  // Only http(s) links open, in a new tab, without an opener.
  function openLink(_event, uri) {
    if (/^https?:\/\//i.test(uri)) window.open(uri, "_blank", "noopener,noreferrer");
  }

  function scale(t) {
    const screen = t.inner.querySelector(".xterm-screen");
    if (!screen || !t.wrap.clientWidth) return;
    const w = screen.offsetWidth, h = screen.offsetHeight;
    if (!w || !h) return;
    let s = Math.min(1, (t.wrap.clientWidth - 2) / w);
    if (s < MIN_SCALE) s = MIN_SCALE;
    t.inner.style.transform = `scale(${s})`;
    t.sizer.style.width = `${Math.ceil(w * s)}px`;
    t.sizer.style.height = `${Math.ceil(h * s)}px`;
  }

  // xterm (300 KB) loads only when a terminal is first opened, never with the chat.
  let loading = null;
  function load() {
    if (window.Terminal) return Promise.resolve();
    const one = (tag, attrs) => new Promise((ok, bad) => {
      const el = Object.assign(document.createElement(tag), attrs);
      el.onload = ok; el.onerror = bad;
      document.head.appendChild(el);
    });
    return loading ??= one("link", { rel: "stylesheet", href: "/lib/xterm.css" })
      .then(() => one("script", { src: "/lib/xterm.js" }))
      .then(() => Promise.all([one("script", { src: "/lib/addon-unicode11.js" }), one("script", { src: "/lib/addon-web-links.js" })]));
  }

  return {
    async open(id, host, cols, rows) {
      await load();
      host.innerHTML = "";
      const sizer = document.createElement("div");
      sizer.className = "term-sizer";
      const inner = document.createElement("div");
      inner.className = "term-inner";
      sizer.appendChild(inner);
      host.appendChild(sizer);
      const term = new Terminal({
        cols, rows,
        convertEol: false,
        disableStdin: true,
        allowProposedApi: true,
        fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
        fontSize: 13,
        scrollback: 5000,
        theme: {
          background: "#0d1117", foreground: "#e6edf3", cursor: "#e6edf3", selectionBackground: "#264f78",
          black: "#484f58", red: "#ff7b72", green: "#3fb950", yellow: "#d29922", blue: "#58a6ff",
          magenta: "#bc8cff", cyan: "#39c5cf", white: "#b1bac4", brightBlack: "#6e7681", brightRed: "#ffa198",
          brightGreen: "#56d364", brightYellow: "#e3b341", brightBlue: "#79c0ff", brightMagenta: "#d2a8ff",
          brightCyan: "#56d4dd", brightWhite: "#f0f6fc",
        },
      });
      term.loadAddon(new Unicode11Addon.Unicode11Addon());
      term.unicode.activeVersion = "11";
      term.loadAddon(new WebLinksAddon.WebLinksAddon(openLink));
      term.open(inner);
      const t = { term, wrap: host, sizer, inner };
      t.onResize = () => scale(t);
      window.addEventListener("resize", t.onResize);
      t.ro = new ResizeObserver(t.onResize);
      t.ro.observe(host);
      terms.set(id, t);
      requestAnimationFrame(() => scale(t));
    },
    resize(id, cols, rows) {
      const t = terms.get(id);
      if (!t || (t.term.cols === cols && t.term.rows === rows)) return;
      t.term.resize(cols, rows);
      requestAnimationFrame(() => scale(t));
    },
    // Raw: the paint is the pane's own bytes, with no newline rewriting.
    write(id, text) {
      const t = terms.get(id);
      if (t) t.term.write(text);
    },
    writeBase64(id, b64) {
      const t = terms.get(id);
      if (t) t.term.write(decode(b64));
    },
    // A viewer that fell behind gets a fresh screen instead of stale output.
    reset(id, text) {
      const t = terms.get(id);
      if (!t) return;
      t.term.reset();
      t.term.write(text);
    },
    rescale(id) {
      const t = terms.get(id);
      if (t) requestAnimationFrame(() => scale(t));
    },
    close(id) {
      const t = terms.get(id);
      if (!t) return;
      window.removeEventListener("resize", t.onResize);
      t.ro.disconnect();
      t.term.dispose();
      terms.delete(id);
    },
  };
})();
