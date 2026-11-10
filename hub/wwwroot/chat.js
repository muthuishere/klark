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
    // The window's scroll for a page. Saved only while the page's own content is in
    // the DOM (leaving shrinks the page and would otherwise save 0), restored once the
    // content is tall enough; with nothing saved, the item last opened is brought into view.
    trackScroll(key, sel, lastSel) {
      try { history.scrollRestoration = "manual"; } catch { }
      // Leaving (a tap on an item) freezes the saved value: navigation scrolls to the top before the list goes.
      window.klarkLeaving = false;
      const alive = () => !window.klarkLeaving && (!sel || !!document.querySelector(sel));
      let frame = 0, restoring = true;
      const save = () => { frame = 0; if (!restoring && alive()) api.set("scroll:" + key, String(Math.round(window.scrollY)), "s"); };
      const onScroll = () => { if (!frame) frame = requestAnimationFrame(save); };
      const stop = () => { restoring = false; };
      window.addEventListener("scroll", onScroll, { passive: true });
      ["wheel", "touchstart", "keydown"].forEach((e) => window.addEventListener(e, stop, { passive: true, once: true }));
      const y = Number(api.get("scroll:" + key, "s") || 0);
      const t0 = performance.now();
      const step = () => {
        if (!restoring) return;
        if (y > 0) {
          if (document.documentElement.scrollHeight - window.innerHeight >= y || performance.now() - t0 > 3000) { window.scrollTo(0, y); restoring = false; }
          else requestAnimationFrame(step);
          return;
        }
        restoring = false;
        const last = lastSel && document.querySelector(lastSel);
        if (last) {
          const r = last.getBoundingClientRect();
          if (r.top < 0 || r.bottom > window.innerHeight) last.scrollIntoView({ block: "center" });
        }
      };
      step();
      return { dispose() { save(); window.removeEventListener("scroll", onScroll); restoring = false; } };
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

  // highlight.js core (20 KB) and each language load only when a code block in that
  // language is on the page (ADR 0011, G); hljs escapes the source text.
  const pending = new Set();
  function highlight(root) {
    root.querySelectorAll("pre code[data-lang]:not([data-hl])").forEach((el) => {
      const lang = (el.getAttribute("data-lang") || "").toLowerCase().split(/\s+/)[0];
      if (!lang) { el.setAttribute("data-hl", "0"); return; }
      if (pending.has(el)) return;
      pending.add(el);
      klarkHl.load(lang).then((hl) => {
        pending.delete(el);
        if (!el.isConnected || el.hasAttribute("data-hl")) return;
        el.setAttribute("data-hl", "1");
        if (!hl || !hl.getLanguage(lang)) return; // no auto-detect in chat: slow and often wrong
        try {
          el.innerHTML = hl.highlight(el.textContent, { language: lang, ignoreIllegals: true }).value;
          el.classList.add("hljs");
        } catch { }
      });
    });
  }

  // Row heights measured while drawn, per pane, kept after leaving so coming back lays out exactly.
  const hcaches = new Map();
  const heightsFor = (key) => {
    let m = hcaches.get(key);
    if (m) hcaches.delete(key); else m = new Map();
    hcaches.set(key, m);
    if (hcaches.size > 6) hcaches.delete(hcaches.keys().next().value);
    return m;
  };
  // Long chats draw only the rows near the viewport (ADR 0011, D): Blazor renders rows [lo, hi) and two
  // empty pads standing for the rest, sized here from measured row heights (guesses until drawn).
  const WINDOW_MIN = 150; // chats up to this many rows are always drawn whole (ChatWindow.Always)

  function attach(key, scroller, pill, dn) {
    const v = { key, scroller, pill, dn, stuck: true, open: new Set(), matches: [], at: -1, keys: [], est: [], idx: new Map(), hm: heightsFor(key), off: null, anchor: null, req: null, hold: false };
    const col = scroller.querySelector(".chat-col");
    try { JSON.parse(klarkState.get("open:" + key) || "[]").forEach((k) => v.open.add(k)); } catch { }
    const saved = klarkState.get("chatscroll:" + key, "s");
    v.restoring = !!(saved && saved !== "latest");

    const nearBottom = () => scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 80;
    const toBottom = () => { scroller.scrollTop = scroller.scrollHeight; };
    const showPill = () => pill.classList.toggle("show", !v.stuck);
    // ---- windowing -------------------------------------------------------
    const gap = () => parseFloat(getComputedStyle(col).rowGap) || 0;
    const rowEls = () => col.querySelectorAll(":scope > [data-id]");
    const win = () => [Number(col.dataset.lo || 0), Number(col.dataset.hi || 0)];
    const hOf = (i) => v.hm.get(v.keys[i]) ?? v.est[i] ?? 100;
    const absTop = (el) => el.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop;
    const elFor = (k) => { for (const e of rowEls()) if (e.dataset.id === k) return e; return null; };
    function offsets() {
      if (v.off) return v.off;
      const g = gap(), n = v.keys.length, o = new Float64Array(n + 1);
      for (let i = 0; i < n; i++) o[i + 1] = o[i] + hOf(i) + g;
      return (v.off = o);
    }
    function applyPads() {
      const n = v.keys.length, [lo, hi] = win();
      if (!n || hi > n) return;
      const o = offsets(), g = gap();
      const top = col.querySelector(":scope > [data-pad=top]"), bot = col.querySelector(":scope > [data-pad=bottom]");
      if (top) top.style.height = Math.max(0, o[lo] - g) + "px";
      if (bot) bot.style.height = Math.max(0, o[n] - o[hi] - g) + "px";
    }
    function firstVisible() {
      const top = scroller.getBoundingClientRect().top;
      for (const el of rowEls()) {
        const r = el.getBoundingClientRect();
        if (r.bottom > top + 1) return { el, top: r.top - top };
      }
      return null;
    }
    // The row at the top of the view stays where it is while rows above it are measured, drawn or dropped.
    function recordAnchor() {
      const a = firstVisible();
      v.anchor = a ? { key: a.el.dataset.id, abs: absTop(a.el) } : null;
    }
    function correct() {
      if (v.restoring) return;
      if (v.stuck) { toBottom(); v.anchor = null; return; }
      const el = v.anchor && elFor(v.anchor.key);
      if (!el) return recordAnchor();
      const now = absTop(el), d = now - v.anchor.abs;
      if (Math.abs(d) > 0.5) scroller.scrollTop += d;
      v.anchor.abs = now;
    }
    // Ask Blazor for another window when the rows around the view are not all drawn,
    // or when far more than needed are.
    function want() {
      const n = v.keys.length, [lo, hi] = win(), els = rowEls();
      if (n <= WINDOW_MIN || v.restoring || v.hold || !v.dn || !els.length) return;
      if (v.req) {
        if (v.req === lo + "-" + hi || performance.now() - v.reqAt > 1500) v.req = null; else return;
      }
      const i0 = v.idx.get(els[0].dataset.id);
      if (i0 === undefined) return;
      const o = offsets(), base = absTop(els[0]) - o[i0];
      const vt = scroller.scrollTop, vb = vt + scroller.clientHeight, ch = Math.max(scroller.clientHeight, 900);
      const at = (y) => { let a = 0, b = n - 1; while (a < b) { const m = (a + b + 1) >> 1; if (base + o[m] <= y) a = m; else b = m - 1; } return a; };
      const grow = (lo > 0 && lo > at(vt - 0.75 * ch)) || (hi < n && hi < at(vb + 0.75 * ch) + 1);
      const far = lo < at(vt - 3 * ch) || hi > at(vb + 3 * ch) + 1;
      if (!grow && !far) return;
      const nl = at(vt - 1.5 * ch), nh = v.stuck ? n : at(vb + 1.5 * ch) + 1;
      if (nl === lo && nh === hi) return;
      request(nl, nh);
    }
    function request(nl, nh) {
      v.req = nl + "-" + nh;
      v.reqAt = performance.now();
      v.dn.invokeMethodAsync("SetWindow", nl, nh).catch(() => { v.req = null; });
      // A window that arrives after the reader stopped (or never arrives) must not leave the view empty.
      clearTimeout(v.rt);
      v.rt = setTimeout(() => { v.req = null; want(); }, 1600);
    }
    function relayout() {
      if (!v.keys.length) return;
      applyPads();
      correct();
      want();
    }
    v.relayout = relayout;
    v.request = request;
    v.recordAnchor = recordAnchor;
    // Rows report their real height once drawn and whenever it changes (code highlighting, a folded tool opened).
    v.ro = new ResizeObserver((entries) => {
      let changed = false;
      for (const e of entries) {
        const k = e.target.dataset.id;
        if (!k) continue;
        const h = e.borderBoxSize?.[0]?.blockSize ?? e.contentRect.height;
        if (Math.abs((v.hm.get(k) ?? -1) - h) > 0.5) { v.hm.set(k, h); changed = true; }
      }
      if (changed) { v.off = null; relayout(); }
    });
    const watchRows = (nodes, on) => nodes.forEach((n) => { if (n.nodeType === 1 && n.dataset?.id) on ? v.ro.observe(n) : v.ro.unobserve(n); });
    v.cmo = new MutationObserver((muts) => {
      for (const m of muts) { watchRows(m.removedNodes, false); watchRows(m.addedNodes, true); }
      relayout();
    });
    v.cmo.observe(col, { childList: true });
    watchRows(rowEls(), true);
    v.setRows = (keys, est) => {
      v.keys = keys; v.est = est; v.off = null;
      v.idx = new Map(keys.map((k, i) => [k, i]));
      relayout();
    };
    // ---------------------------------------------------------------------
    let frame = 0;
    v.save = () => {
      frame = 0;
      // A detached or still-restoring scroller says nothing about where the reader was.
      if (!scroller.isConnected || v.restoring || scroller.clientHeight === 0) return;
      klarkState.set("chatscroll:" + key, v.stuck ? "latest" : String(Math.round(scroller.scrollTop)), "s");
      // The first row in view and where it sat: restores exactly even when row heights were guesses.
      const a = !v.stuck && firstVisible();
      klarkState.set("chatanchor:" + key, a ? a.el.dataset.id + "\t" + Math.round(a.top) : "", "s");
    };
    v.onScroll = () => {
      if (v.restoring) return;
      v.stuck = nearBottom();
      showPill();
      recordAnchor();
      want();
      if (!frame) frame = requestAnimationFrame(v.save);
    };
    v.onHide = () => { if (document.visibilityState === "hidden") v.save(); };
    document.addEventListener("visibilitychange", v.onHide);
    window.addEventListener("pagehide", v.save);
    v.stopRestore = () => { v.restoring = false; };
    ["wheel", "touchstart", "keydown"].forEach((e) => scroller.addEventListener(e, v.stopRestore, { passive: true }));
    scroller.addEventListener("scroll", v.onScroll, { passive: true });
    v.onPill = () => { v.stuck = true; v.hold = false; toBottom(); showPill(); want(); };
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
      const f = e.target.closest("a.fpath");
      if (f && e.button === 0 && !e.metaKey && !e.ctrlKey && !e.shiftKey) {
        // A file opens in the viewer in this tab; back comes to this exact spot.
        e.preventDefault();
        e.stopPropagation();
        v.save();
        location.href = f.href;
        return;
      }
      const b = e.target.closest("button.copy");
      if (!b) return;
      const code = b.closest(".codeblk, .code")?.querySelector("pre code");
      if (!code) return;
      copyText(code.textContent).then(() => { b.textContent = "Copied"; setTimeout(() => (b.textContent = "Copy"), 1200); });
    };
    scroller.addEventListener("click", v.onClick);

    // New items arrive as appended DOM; follow them only if the reader is at the bottom.
    v.mo = new MutationObserver(() => {
      highlight(scroller);
      restoreOpen(scroller);
      if (v.restoring) return;
      if (v.stuck) toBottom();
      else showPill();
    });
    v.mo.observe(scroller, { childList: true, subtree: true, characterData: true });
    highlight(scroller);
    restoreOpen(scroller);
    if (v.restoring) {
      // Wait until the rows that were there are drawn again (a reload paints them a moment later).
      v.stuck = false;
      const y = Number(saved), t0 = performance.now();
      const [ak, ao] = (klarkState.get("chatanchor:" + key, "s") || "").split("\t");
      const done = () => { v.restoring = false; v.stuck = nearBottom(); showPill(); recordAnchor(); want(); };
      const step = () => {
        if (!v.restoring || !scroller.isConnected) return;
        const late = performance.now() - t0 > 3000;
        if (ak && !late && (!v.keys.length || v.idx.has(ak))) {
          // By the row that was first in view: the rows around it are drawn first, then it is put back where it sat.
          if (!v.keys.length) return void requestAnimationFrame(step);
          const i = v.idx.get(ak);
          const [lo, hi] = win();
          if (i < lo || i >= hi) {
            if (!v.req || performance.now() - v.reqAt > 1500) request(Math.max(0, i - 20), Math.min(v.keys.length, i + 40));
            return void requestAnimationFrame(step);
          }
          const el = elFor(ak);
          if (!el) return void requestAnimationFrame(step);
          scroller.scrollTop += el.getBoundingClientRect().top - scroller.getBoundingClientRect().top - Number(ao || 0);
          return done();
        }
        if (scroller.scrollHeight - scroller.clientHeight >= y || late) { scroller.scrollTop = y; done(); }
        else requestAnimationFrame(step);
      };
      step();
    } else {
      toBottom();
      requestAnimationFrame(toBottom);
    }
    showPill();
    views.set(key, v);
  }

  function detach(key) {
    const v = views.get(key);
    if (!v) return;
    v.mo.disconnect();
    v.cmo.disconnect();
    clearTimeout(v.rt);
    v.ro.disconnect();
    v.restoring = false;
    document.removeEventListener("visibilitychange", v.onHide);
    window.removeEventListener("pagehide", v.save);
    v.scroller.removeEventListener("scroll", v.onScroll);
    v.scroller.removeEventListener("toggle", v.onToggle, true);
    v.scroller.removeEventListener("click", v.onClick);
    v.pill.removeEventListener("click", v.onPill);
    views.delete(key);
  }

  // Search in what is loaded, with the CSS Highlight API (no DOM rewriting,
  // so Blazor's DOM stays untouched). Returns the number of matches.
  function search(key, query, keep) {
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
    if (v.matches.length) {
      const at = keep ? Number(klarkState.get("searchat:" + key, "s") || 0) : 0;
      if (at > 0 && at <= v.matches.length) {
        // Coming back: the same match is current, but the reader's own scroll position wins.
        v.at = at - 1;
        if (window.CSS && CSS.highlights && window.Highlight) CSS.highlights.set("klark-cur", new Highlight(v.matches[v.at]));
      }
      else step(key, -1);
    }
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
    klarkState.set("searchat:" + key, String(v.at + 1), "s");
    return v.at + 1;
  }

  // Bring a search hit into view (sidebar message search: ?hit=<item id>).
  function reveal(key, id) {
    const v = views.get(key);
    if (!v) return false;
    const el = v.scroller.querySelector(`[data-id="${CSS.escape(id)}"]`);
    if (!el) return false;
    v.restoring = false;
    v.hold = false;
    v.stuck = false;
    el.scrollIntoView({ block: "center" });
    v.recordAnchor();
    el.classList.add("hit");
    setTimeout(() => el.classList.remove("hit"), 2400);
    v.pill.classList.add("show");
    return true;
  }

  // Blazor sends every row's key and height guess whenever the rows change.
  function rows(key, keys, est) { views.get(key)?.setRows(keys, est); }
  // The window is about to move to a search hit: do not pull it back until the hit is shown.
  function pin(key) { const v = views.get(key); if (v) { v.hold = true; v.restoring = false; } }

  // The composer is client-side: typing never round-trips; Send takes the text.
  function take(textarea) {
    const t = textarea.value;
    textarea.value = "";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    return t;
  }
  const peek = (textarea) => textarea?.value ?? "";

  // Composer: Enter sends, Shift+Enter is a newline, and the draft is kept per pane.
  function composer(key, textarea, send, commands) {
    const draftKey = "draft:" + key;
    const draft = klarkState.get(draftKey);
    if (draft && !textarea.value) {
      textarea.value = draft;
      textarea.dispatchEvent(new Event("input", { bubbles: true }));
    }
    const grow = () => { textarea.style.height = "auto"; textarea.style.height = Math.min(textarea.scrollHeight, 160) + "px"; };
    // Slash-command popup: shown while the text is "/word" with no space yet.
    const pop = textarea.closest(".composer-wrap")?.querySelector(".slash-pop");
    const cmds = Array.isArray(commands) ? commands : [];
    let rows = [], sel = 0;
    const popOpen = () => !!pop && !pop.hidden && rows.length > 0;
    const popClose = () => {
      if (!pop) return;
      pop.hidden = true; rows = [];
      textarea.removeAttribute("aria-activedescendant"); textarea.setAttribute("aria-expanded", "false");
    };
    const paint = () => {
      pop.replaceChildren(...rows.map((c, i) => {
        const el = document.createElement("div");
        el.id = pop.id + "-" + i; el.setAttribute("role", "option");
        el.setAttribute("aria-selected", i === sel ? "true" : "false");
        const n = document.createElement("span"); n.className = "slash-name"; n.textContent = c.name;
        const d = document.createElement("span"); d.className = "slash-desc"; d.textContent = c.description;
        el.append(n, d);
        el.addEventListener("mousedown", (e) => e.preventDefault()); // keep the caret in the box
        el.addEventListener("click", () => pick(i));
        return el;
      }));
      textarea.setAttribute("aria-activedescendant", pop.id + "-" + sel);
      pop.children[sel]?.scrollIntoView({ block: "nearest" });
    };
    const refresh = () => {
      const v = textarea.value;
      if (!pop || !cmds.length || !/^\/[^\s]*$/.test(v)) return popClose();
      const q = v.toLowerCase();
      const pre = cmds.filter((c) => c.name.startsWith(q));
      const sub = cmds.filter((c) => !c.name.startsWith(q) && c.name.includes(q.slice(1)));
      rows = [...pre, ...sub];
      if (!rows.length || (rows.length === 1 && rows[0].name === v)) return popClose();
      sel = 0; pop.hidden = false; textarea.setAttribute("aria-expanded", "true"); paint();
    };
    const pick = (i) => {
      const c = rows[i];
      if (!c) return;
      textarea.value = c.name + " ";
      popClose();
      textarea.focus();
      textarea.dispatchEvent(new Event("input", { bubbles: true }));
    };
    if (pop) { textarea.setAttribute("role", "combobox"); textarea.setAttribute("aria-expanded", "false"); }
    textarea.addEventListener("input", () => { klarkState.set(draftKey, textarea.value); grow(); refresh(); });
    textarea.addEventListener("blur", () => setTimeout(popClose, 150));
    textarea.addEventListener("keydown", (e) => {
      if (popOpen() && !e.isComposing) {
        if (e.key === "ArrowDown" || e.key === "ArrowUp") {
          e.preventDefault();
          sel = (sel + (e.key === "ArrowDown" ? 1 : rows.length - 1)) % rows.length; paint(); return;
        }
        if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey)) { e.preventDefault(); pick(sel); return; }
        if (e.key === "Escape") { e.preventDefault(); popClose(); return; }
      }
      if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
        e.preventDefault();
        if (textarea.value.trim() || textarea.dataset.attached === "1") { klarkState.set(draftKey, ""); send.click(); setTimeout(grow, 50); }
      }
    });
    send.addEventListener("click", () => { setTimeout(() => { if (!textarea.value) klarkState.set(draftKey, ""); grow(); }, 50); });
    grow();
    refresh();
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

  const current = (key) => { const v = views.get(key); return v ? v.at + 1 : 0; };
  return { attach, detach, rows, pin, search, step, current, composer, uploads, reveal, take, peek, highlight, focus: (el) => el?.focus() };
})();

// Navigation that never leaves the app (ADR 0011, E). The header arrow goes back
// when this chat was opened from the list in this tab, so the list comes back as it
// was; opened any other way (a link, a bookmark, after login) it replaces the entry
// with the list, so a second back does not bounce into the chat again.
window.klarkNav = (() => {
  const go = (url, replace) => {
    if (window.Blazor && Blazor.navigateTo) Blazor.navigateTo(url, { replaceHistoryEntry: replace });
    else if (replace) location.replace(url);
    else location.href = url;
  };
  function back(e) {
    e?.preventDefault?.();
    // The entry before this one is the list only if this chat was opened from it and
    // nothing was pushed since (a reload keeps the entry; a typed URL or link pushes one).
    const from = klarkState.get("from", "s"), len = Number(klarkState.get("fromlen", "s") || 0);
    if (from && from === location.pathname && history.length === len) history.back();
    else go("/", true);
    return false;
  }
  // Opening a chat from the list: remember where the list was and which item it was.
  document.addEventListener("click", (e) => {
    const a = e.target.closest?.("a[data-pane-link]");
    if (!a || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey) return;
    const href = a.getAttribute("href");
    klarkState.set("from", href, "s");
    klarkState.set("fromlen", String(history.length + 1), "s");
    klarkState.set("home:last", href, "s");
    klarkState.set("scroll:home", String(Math.round(window.scrollY)), "s");
    window.klarkLeaving = true;
  }, true);
  return { back, go };
})();

// Small page chrome: overflow menus close when an item is picked or on an outside tap;
// the sign-in form replaces its own history entry, so back never returns to it.
window.klarkUi = (() => {
  const closeMenu = (el) => { if (el) el.open = false; };
  document.addEventListener("click", (e) => {
    document.querySelectorAll("details.menu[open]").forEach((d) => {
      if (!d.contains(e.target) || e.target.closest(".menu-item, .menu-pop a")) d.open = false;
    });
  });
  document.addEventListener("submit", async (e) => {
    const form = e.target;
    if (!(form instanceof HTMLFormElement) || !form.hasAttribute("data-klark-login")) return;
    e.preventDefault();
    const button = form.querySelector("button[type=submit]");
    if (button) button.disabled = true;
    try {
      const r = await fetch(form.action, { method: "POST", body: new FormData(form), credentials: "same-origin" });
      location.replace(r.url || "/");
    } catch {
      form.removeAttribute("data-klark-login");
      form.submit();
    }
  });
  return { closeMenu, isOpen: (el) => !!el?.open };
})();

// Lazy highlight.js (shared with the file viewer's own copy of this loader).
window.klarkHl = (() => {
  const base = "/lib/hljs-11.11.1/";
  const alias = { js: "javascript", jsx: "javascript", ts: "typescript", tsx: "typescript", py: "python", rb: "ruby", rs: "rust",
    sh: "bash", shell: "bash", zsh: "bash", console: "bash", yml: "yaml", md: "markdown", cs: "csharp", "c#": "csharp", kt: "kotlin",
    html: "xml", svg: "xml", vue: "xml", toml: "ini", golang: "go", "c++": "cpp", hpp: "cpp", h: "c", ps1: "powershell", tf: "hcl",
    dockerfile: "dockerfile", docker: "dockerfile", make: "makefile", jsonc: "json", text: "plaintext", txt: "plaintext", diff: "diff", patch: "diff" };
  let core = null;
  const langs = new Map();
  const getCore = () => (core ??= import(base + "core.min.js").then((m) => m.default));
  function load(lang) {
    const name = alias[lang] || lang;
    if (!/^[a-z0-9-]+$/.test(name) || name === "plaintext") return Promise.resolve(null);
    if (!langs.has(name)) {
      langs.set(name, getCore().then((hl) => hl.getLanguage(name) ? hl
        : import(base + "languages/" + name + ".min.js").then((m) => { hl.registerLanguage(name, m.default); return hl; }).catch(() => hl)));
    }
    return langs.get(name);
  }
  return { load, alias };
})();

// The sidebar (ADR 0011, E): collapsible on desktop, a drawer on phones, Cmd/Ctrl+K searches.
window.klarkSide = (() => {
  const narrow = () => window.matchMedia("(max-width: 860px)").matches;
  function toggle() {
    const html = document.documentElement;
    if (narrow()) { html.classList.toggle("drawer-open"); return false; }
    const closed = html.classList.toggle("side-closed");
    klarkState.set("side", closed ? "closed" : "");
    return false;
  }
  function close() { document.documentElement.classList.remove("drawer-open"); }
  document.addEventListener("keydown", (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
      const box = document.querySelector(".side-search input");
      if (!box) return;
      e.preventDefault();
      if (narrow()) document.documentElement.classList.add("drawer-open");
      else document.documentElement.classList.remove("side-closed");
      box.focus();
      box.select();
    } else if (e.key === "Escape") close();
  });
  document.addEventListener("click", (e) => {
    if (e.target.closest(".side a[href]") || e.target.closest(".scrim")) close();
  });
  // The list's own scroll, per tab.
  function track(el, key) {
    if (!el) return;
    const y = Number(klarkState.get("sidescroll:" + key, "s") || 0);
    if (y > 0) el.scrollTop = y;
    else el.querySelector(".cur")?.scrollIntoView({ block: "center" });
    let f = 0;
    el.addEventListener("scroll", () => {
      if (!f) f = requestAnimationFrame(() => { f = 0; klarkState.set("sidescroll:" + key, String(Math.round(el.scrollTop)), "s"); });
    }, { passive: true });
  }
  // Named themes (app.css). No argument: step to the next one.
  const THEMES = ["paper", "teal", "indigo", "forest", "dusk", "light", "dark"];
  function theme(name) {
    const html = document.documentElement;
    if (!THEMES.includes(name)) name = THEMES[(THEMES.indexOf(html.dataset.theme || "paper") + 1) % THEMES.length];
    html.dataset.theme = name;
    klarkState.set("theme", name);
    document.querySelectorAll("[data-theme-pick]").forEach((b) => b.setAttribute("aria-pressed", String(b.dataset.themePick === name)));
  }
  // The page frame is re-rendered by in-page navigation (picking another agent), which drops
  // the attribute theme() set; put the saved theme back whenever that happens.
  const saved = () => { try { return localStorage.getItem("klark:theme") || "paper"; } catch { return "paper"; } };
  new MutationObserver(() => {
    const want = saved();
    if (THEMES.includes(want) && document.documentElement.dataset.theme !== want) document.documentElement.dataset.theme = want;
  }).observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
  return { toggle, close, track, theme, narrow };
})();
