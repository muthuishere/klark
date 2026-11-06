// xterm.js glue for the pane view. One terminal per open pane view, keyed by id.
window.klarkTerm = (() => {
  const terms = new Map();

  function decode(b64) {
    const bin = atob(b64);
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
    return bytes;
  }

  return {
    open(id, host) {
      const term = new Terminal({
        convertEol: false,
        disableStdin: true,
        fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
        fontSize: 13,
        scrollback: 5000,
      });
      const fit = new FitAddon.FitAddon();
      term.loadAddon(fit);
      term.open(host);
      fit.fit();
      const onResize = () => fit.fit();
      window.addEventListener("resize", onResize);
      terms.set(id, { term, onResize });
    },
    writeText(id, text) {
      const t = terms.get(id);
      if (t) t.term.write(text.replace(/\r?\n/g, "\r\n"));
    },
    writeBase64(id, b64) {
      const t = terms.get(id);
      if (t) t.term.write(decode(b64));
    },
    close(id) {
      const t = terms.get(id);
      if (!t) return;
      window.removeEventListener("resize", t.onResize);
      t.term.dispose();
      terms.delete(id);
    },
  };
})();
