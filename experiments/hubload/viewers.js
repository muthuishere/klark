// Viewer harness for the real-hub load test (E-06). Injected into a signed-in
// hub page with `chrome-agent evalAsync`. Every iframe is a separate Blazor
// Server circuit (its own WebSocket and server-side component tree).
//   KL.open(kind, path, n)   kind: list | chat | term
//   KL.close(kind)           remove that kind's iframes (circuits close)
//   KL.stats()               per-kind counts, terminal bytes/frames received
//   KL.typeLatency(i, n)     keystroke -> echo round trips through the hub, ms
//   KL.chatLatency(i, text)  composer send -> the person's turn visible in chat, ms
window.KL = window.KL || (() => {
  const frames = [];
  let seq = 0;
  // The wrappers must be created inside the iframe's own realm: Blazor checks
  // `instanceof Function` against the iframe's Function.
  const HOOK = `(function (f) {
    const t = window.klarkTerm;
    const dec = (b64) => { try { return atob(b64); } catch (e) { return ""; } };
    const wrap = (name, toText) => {
      const orig = t[name].bind(t);
      t[name] = function (id, x) {
        const s = toText(x);
        f.frames++; f.bytes += s.length; f.last = f.now();
        for (const [m, w] of f.waiters) if (m === "*" || s.includes(m)) { f.waiters.delete(m); w(f.now()); }
        return orig.apply(null, arguments);
      };
    };
    wrap("writeBase64", dec);
    wrap("write", (x) => x);
    wrap("reset", (x) => { f.resets++; return x; });
    t.__kl = true;
  })`;
  function hook(f) {
    const w = f.el.contentWindow;
    if (!w || !w.klarkTerm || w.klarkTerm.__kl) return false;
    w.eval(HOOK)(f);
    return true;
  }
  // Hook before the first Blazor call: Blazor caches the JS function it resolves.
  setInterval(() => frames.forEach((f) => f.kind === "term" && !f.hooked && (f.hooked = hook(f))), 5);
  return {
    open(kind, path, n = 1) {
      for (let i = 0; i < n; i++) {
        const el = document.createElement("iframe");
        el.style.cssText = "width:1200px;height:800px;border:0;display:block";
        el.src = path;
        document.body.appendChild(el);
        frames.push({ id: ++seq, kind, path, el, frames: 0, bytes: 0, resets: 0, waiters: new Map(), hooked: false, now: () => performance.now() });
      }
      return frames.length;
    },
    close(kind) {
      for (let i = frames.length - 1; i >= 0; i--) {
        if (!kind || frames[i].kind === kind) { frames[i].el.remove(); frames.splice(i, 1); }
      }
      return frames.length;
    },
    stats() {
      const by = {};
      for (const f of frames) {
        const b = (by[f.kind] ||= { n: 0, hooked: 0, frames: 0, bytes: 0, resets: 0, disconnected: 0 });
        b.n++; b.hooked += f.hooked ? 1 : 0; b.frames += f.frames; b.bytes += f.bytes; b.resets += f.resets;
        const d = f.el.contentDocument;
        if (d && d.querySelector("#components-reconnect-modal.components-reconnect-show, .components-reconnect-show")) b.disconnected++;
      }
      return by;
    },
    _frames: frames,
    terms() { return frames.filter((f) => f.kind === "term").map((f) => f.path); },
    async send(f, text) {
      const d = f.el.contentDocument;
      const ta = d.querySelector(".composer textarea");
      const btn = d.querySelector(".composer .send");
      if (!ta || !btn) throw new Error("no composer in " + f.path);
      ta.value = text;
      ta.dispatchEvent(new f.el.contentWindow.Event("input", { bubbles: true }));
      await new Promise((r) => setTimeout(r, 30));
      const t0 = performance.now();
      btn.click();
      return t0;
    },
    async typeLatency(i, n = 20, gap = 300) {
      const f = frames.filter((x) => x.kind === "term")[i];
      const out = [];
      for (let k = 0; k < n; k++) {
        const m = "KL" + Date.now().toString(36) + k;
        // The observe stream sends screen diffs, so the marker may not arrive verbatim:
        // on an idle shell pane the first frame after sending IS the echo.
        await new Promise((r) => setTimeout(r, 150));
        const seen = new Promise((r) => f.waiters.set("*", r));
        const t0 = await this.send(f, "echo " + m);
        const t1 = await Promise.race([seen, new Promise((r) => setTimeout(() => r(null), 5000))]);
        out.push(t1 === null ? null : Math.round(t1 - t0));
        await new Promise((r) => setTimeout(r, gap));
      }
      return out;
    },
    async chatLatency(i, text) {
      const f = frames.filter((x) => x.kind === "chat")[i];
      const m = "KLC" + Date.now().toString(36);
      const t0 = await this.send(f, text + " (" + m + ")");
      for (;;) {
        if ((f.el.contentDocument.body.innerText || "").includes(m)) return Math.round(performance.now() - t0);
        if (performance.now() - t0 > 60000) return null;
        await new Promise((r) => setTimeout(r, 50));
      }
    },
  };
})();
return "KL ready";
