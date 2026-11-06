# E-09: xterm.js on a phone

- Status: **to do**

**Question:** Does a mid-range phone keep up rendering a pane at 20 KB/s?

**How:** Pane view on Android Chrome or iOS Safari via the hub. Measure frame time with performance.now() around term.write.

**Pass / decision rule:** No dropped input and < 16 ms per write. If over: batch writes in requestAnimationFrame.
