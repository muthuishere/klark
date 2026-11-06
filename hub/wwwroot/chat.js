// Browser-side behaviour for the chat view and the machine list (ADR 0011, D).
// Everything here is per viewer: scroll position, open tool rows, drafts,
// the chosen tab and the list filter live in this browser's storage, so
// leaving a page and coming back (or a reloaded circuit) puts things back.
window.klarkState = (() => {
  const store = (kind) => { try { return kind === "s" ? window.sessionStorage : window.localStorage; } catch { return null; } };
  const api = {
    get(key, kind) { try { return store(kind)?.getItem("klark:" + key) ?? null; } catch { return null; } },
    set(key, value, kind) {
      try { if (value === null || value === undefined || value === "") store(kind)?.removeItem("klark:" + key); else store(kind)?.setItem("klark:" + key, value); } catch { }
    },
    isNarrow() { return window.matchMedia("(max-width: 760px)").matches; },
    // The window's scroll for a page, restored once its content is tall enough.
    trackScroll(key) {
      const save = () => api.set("scroll:" + key, String(Math.round(window.scrollY)), "s");
      let pending = 0;
      const onScroll = () => { if (!pending) pending = setTimeout(() => { pending = 0; save(); }, 150); };
      window.addEventListener("scroll", onScroll, { passive: true });
      const y = Number(api.get("scroll:" + key, "s") || 0);
      if (y > 0) {
        let tries = 0;
        const step = () => {
          if (document.documentElement.scrollHeight - window.innerHeight >= y || tries++ > 60) window.scrollTo(0, y);
          else requestAnimationFrame(step);
        };
        step();
      }
      return { dispose() { window.removeEventListener("scroll", onScroll); } };
    },
  };
  return api;
})();

window.klarkChat = (() => {
  const views = new Map();

  function copyText(text) {
    if (navigator.clipboard && window.isSecureContext) return navigator.clipboard.writeText(text);
    // Plain-HTTP LAN hubs have no clipboard API: fall back to a selection copy.
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    try { document.execCommand("copy"); } finally { ta.remove(); }
    return Promise.resolve();
  }

  function highlight(root) {
    if (!window.hljs) return;
    root.querySelectorAll("pre code[data-lang]:not([data-hl])").forEach((el) => {
      el.setAttribute("data-hl", "1");
      const lang = (el.getAttribute("data-lang") || "").toLowerCase().split(/\s+/)[0];
      if (!lang || !hljs.getLanguage(lang)) return; // no auto-detect: it is slow and often wrong
      try {
        const out = hljs.highlight(el.textContent, { language: lang, ignoreIllegals: true });
        el.innerHTML = out.value; // hljs escapes the source text
        el.classList.add("hljs");
      } catch { }
    });
  }

  function attach(key, scroller, pill) {
    const v = { key, scroller, pill, stuck: true, open: new Set(), matches: [], at: -1 };
    try { JSON.parse(klarkState.get("open:" + key) || "[]").forEach((k) => v.open.add(k)); } catch { }
    const saved = klarkState.get("chatscroll:" + key, "s");

    const nearBottom = () => scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 80;
    const toBottom = () => { scroller.scrollTop = scroller.scrollHeight; };
    const showPill = () => pill.classList.toggle("show", !v.stuck);
    let saveTimer = 0;
    v.onScroll = () => {
      v.stuck = nearBottom();
      showPill();
      if (!saveTimer) saveTimer = setTimeout(() => {
        saveTimer = 0;
        klarkState.set("chatscroll:" + key, v.stuck ? "latest" : String(Math.round(scroller.scrollTop)), "s");
      }, 200);
    };
    scroller.addEventListener("scroll", v.onScroll, { passive: true });
    v.onPill = () => { v.stuck = true; toBottom(); showPill(); };
    pill.addEventListener("click", v.onPill);

    const restoreOpen = (root) => root.querySelectorAll("details[data-key]").forEach((d) => {
      if (v.open.has(d.getAttribute("data-key"))) d.open = true;
    });
    v.onToggle = (e) => {
      const d = e.target;
      if (!(d instanceof HTMLDetailsElement) || !d.dataset.key) return;
      if (d.open) v.open.add(d.dataset.key); else v.open.delete(d.dataset.key);
      klarkState.set("open:" + key, JSON.stringify([...v.open].slice(-300)));
    };
    scroller.addEventListener("toggle", v.onToggle, true);
    v.onClick = (e) => {
      const b = e.target.closest("button.copy");
      if (!b) return;
      const code = b.closest(".code")?.querySelector("pre code");
      if (!code) return;
      copyText(code.textContent).then(() => { b.textContent = "Copied"; setTimeout(() => (b.textContent = "Copy"), 1200); });
    };
    scroller.addEventListener("click", v.onClick);

    // New items arrive as appended DOM; follow them only if the reader is at the bottom.
    v.mo = new MutationObserver(() => {
      highlight(scroller);
      restoreOpen(scroller);
      if (v.stuck) toBottom();
      else showPill();
    });
    v.mo.observe(scroller, { childList: true, subtree: true, characterData: true });
    highlight(scroller);
    restoreOpen(scroller);
    if (saved && saved !== "latest") {
      v.stuck = false;
      scroller.scrollTop = Number(saved);
    } else {
      toBottom();
    }
    showPill();
    views.set(key, v);
  }

  function detach(key) {
    const v = views.get(key);
    if (!v) return;
    v.mo.disconnect();
    v.scroller.removeEventListener("scroll", v.onScroll);
    v.scroller.removeEventListener("toggle", v.onToggle, true);
    v.scroller.removeEventListener("click", v.onClick);
    v.pill.removeEventListener("click", v.onPill);
    views.delete(key);
  }

  // Search in what is loaded, with the CSS Highlight API (no DOM rewriting,
  // so Blazor's DOM stays untouched). Returns the number of matches.
  function search(key, query) {
    const v = views.get(key);
    if (!v) return 0;
    v.matches = [];
    v.at = -1;
    if (window.CSS && CSS.highlights) { CSS.highlights.delete("klark-hit"); CSS.highlights.delete("klark-cur"); }
    const q = (query || "").trim().toLowerCase();
    if (!q) return 0;
    const walker = document.createTreeWalker(v.scroller, NodeFilter.SHOW_TEXT);
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
      const text = n.nodeValue.toLowerCase();
      for (let i = text.indexOf(q); i >= 0; i = text.indexOf(q, i + q.length)) {
        const r = new Range();
        r.setStart(n, i);
        r.setEnd(n, i + q.length);
        v.matches.push(r);
      }
    }
    if (window.CSS && CSS.highlights && window.Highlight) CSS.highlights.set("klark-hit", new Highlight(...v.matches));
    if (v.matches.length) step(key, -1);
    return v.matches.length;
  }

  // Move to the next (+1) or previous (-1) match; returns its 1-based index.
  function step(key, dir) {
    const v = views.get(key);
    if (!v || !v.matches.length) return 0;
    v.at = v.at < 0 ? v.matches.length - 1 : (v.at + dir + v.matches.length) % v.matches.length;
    const r = v.matches[v.at];
    const el = r.startContainer.parentElement;
    el?.closest("details")?.setAttribute("open", "");
    for (let d = el?.closest("details")?.parentElement?.closest("details"); d; d = d.parentElement?.closest("details")) d.open = true;
    el?.scrollIntoView({ block: "center" });
    v.stuck = false;
    v.pill.classList.add("show");
    if (window.CSS && CSS.highlights && window.Highlight) CSS.highlights.set("klark-cur", new Highlight(r));
    return v.at + 1;
  }

  // Composer: Enter sends, Shift+Enter is a newline, and the draft is kept per pane.
  function composer(key, textarea, send) {
    const draftKey = "draft:" + key;
    const draft = klarkState.get(draftKey);
    if (draft && !textarea.value) {
      textarea.value = draft;
      textarea.dispatchEvent(new Event("input", { bubbles: true }));
    }
    const grow = () => { textarea.style.height = "auto"; textarea.style.height = Math.min(textarea.scrollHeight, 160) + "px"; };
    textarea.addEventListener("input", () => { klarkState.set(draftKey, textarea.value); grow(); });
    textarea.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
        e.preventDefault();
        if (textarea.value.trim()) { klarkState.set(draftKey, ""); send.click(); setTimeout(grow, 50); }
      }
    });
    send.addEventListener("click", () => { klarkState.set(draftKey, ""); setTimeout(grow, 50); });
    grow();
  }

  // Paste or drop an image anywhere on the pane page: hand it to the InputFile.
  function uploads(root, input) {
    const give = (files) => {
      const images = [...files].filter((f) => f.type.startsWith("image/") || f.type === "");
      if (!images.length) return false;
      const dt = new DataTransfer();
      dt.items.add(images[0]);
      input.files = dt.files;
      input.dispatchEvent(new Event("change", { bubbles: true }));
      return true;
    };
    root.addEventListener("paste", (e) => { if (e.clipboardData?.files?.length && give(e.clipboardData.files)) e.preventDefault(); });
    root.addEventListener("dragover", (e) => { if (e.dataTransfer?.types?.includes("Files")) { e.preventDefault(); root.classList.add("dropping"); } });
    root.addEventListener("dragleave", (e) => { if (e.target === root) root.classList.remove("dropping"); });
    root.addEventListener("drop", (e) => {
      root.classList.remove("dropping");
      if (e.dataTransfer?.files?.length) { e.preventDefault(); give(e.dataTransfer.files); }
    });
  }

  return { attach, detach, search, step, composer, uploads, focus: (el) => el?.focus() };
})();
