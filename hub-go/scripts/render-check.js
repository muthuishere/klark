// Rendering check for one hub chat page, run in the browser (the body of an async function; returns a report).
// Used by hub-go/scripts/render-check.sh through chrome-agent evalAsync, one pane after another in one tab.
// The chat is windowed (only rows on screen are in the DOM), so it scrolls .chat-scroll top to bottom and checks
// each row (.vrow, keyed by data-key) once. Same rules as chat/render_real_test.go on what the browser shows,
// plus layout: content wider than the chat column, and empty bubbles.
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
// The runner sets EXPECT_PANE: wait until the page shows that pane and its agent (the pane list has loaded).
const want = typeof EXPECT_PANE === "string" ? EXPECT_PANE : "";
for (let i = 0; i < 80; i++) {
  const here = decodeURIComponent(location.pathname);
  const badge = document.querySelector(".pane-bar .badge-a");
  if ((!want || here.endsWith("/p/" + want)) && badge && badge.dataset.agent !== "shell") break;
  await sleep(250);
}
let scroller = null;
for (let i = 0; i < 60 && !(scroller = document.querySelector(".chat-scroll")); i++) await sleep(250);
for (let i = 0; i < 40 && !document.querySelector(".vrow"); i++) await sleep(250);
if (!scroller) return { url: location.pathname, ok: false, error: "no .chat-scroll" };
const tags = ["<system-reminder", "<ref_file", "<ref_snippet", "<truncation_notice", "<system_info",
  "<system_guidance", "<user-prompt-submit-hook", "<local-command-stdout", "(klark hub:", "[REDACTED]"];
const chrome = ["ctrl+p commands", "ctrl+v to paste image", "esc twice to interrupt", "Ask Devin to build features",
  "Ask Codex to do anything", "╹▀▀", "⠀⣴⣾⣶⡄", "Free plan, use /upgrade"];
const seen = new Map();
const totals = { tables: 0, headings: 0, codeBlocks: 0, lists: 0, toolGroups: 0, fileLinks: 0, user: 0, asst: 0 };
const problems = [];
const inspect = (row) => {
  const key = row.dataset.key || row.dataset.id || row.innerText.slice(0, 80);
  if (seen.has(key)) return;
  seen.set(key, true);
  const colW = scroller.clientWidth;
  if (row.querySelector(".user")) totals.user++;
  if (row.querySelector(".asst")) totals.asst++;
  totals.tables += row.querySelectorAll(".md table").length;
  totals.headings += row.querySelectorAll(".md h1, .md h2, .md h3, .md h4, .md h5, .md h6").length;
  totals.codeBlocks += row.querySelectorAll("pre").length;
  totals.lists += row.querySelectorAll(".md ul, .md ol").length;
  totals.toolGroups += row.querySelectorAll(".tool").length;
  totals.fileLinks += row.querySelectorAll("a.fpath").length;
  const bubble = row.querySelector(".bubble");
  if (bubble) {
    const c = bubble.cloneNode(true);
    c.querySelectorAll("pre, code, .tool, details").forEach((x) => x.remove());
    const text = (c.innerText || "").replace(/"[^"\n]*"|“[^”\n]*”/g, "");
    for (const t of tags.concat(chrome)) if (text.includes(t)) problems.push({ key, kind: "leak", what: t });
    for (const el of bubble.querySelectorAll(".md p, .md li, .md td, .md th")) {
      if (el.closest("pre, code")) continue;
      const t = el.innerText || "";
      if (/^#{1,6} \S/m.test(t)) problems.push({ key, kind: "raw heading", what: t.slice(0, 60) });
      if (/\|\s*-{3,}\s*\|/.test(t)) problems.push({ key, kind: "raw table", what: t.slice(0, 60) });
      if (/\*\*[^*\s][^*]*\*\*/.test(t)) problems.push({ key, kind: "raw bold", what: t.slice(0, 60) });
    }
    if (!(bubble.innerText || "").trim() && !bubble.querySelector("pre, table, .tool, img"))
      problems.push({ key, kind: "empty bubble" });
  }
  for (const e of row.querySelectorAll("script, iframe, img[src], a[href^='javascript:'], [onclick], [onerror]"))
    problems.push({ key, kind: "unsafe", what: e.outerHTML.slice(0, 80) });
  for (const e of row.querySelectorAll(".md p, .md li, .md h1, .md h2, .md h3, .md blockquote, .md table")) {
    if (e.closest("pre, [style*=overflow]")) continue;
    let p = e.parentElement, clipped = false;
    while (p && p !== row) { const o = getComputedStyle(p).overflowX; if (o === "auto" || o === "scroll") { clipped = true; break; } p = p.parentElement; }
    if (!clipped && e.getBoundingClientRect().width > colW + 4) problems.push({ key, kind: "overflow", what: e.tagName + " " + Math.round(e.getBoundingClientRect().width) + ">" + colW });
  }
};
scroller.scrollTop = 0;
await sleep(600);
let last = -1;
for (let step = 0; step < 4000; step++) {
  document.querySelectorAll(".vrow").forEach(inspect);
  if (scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 2 && scroller.scrollTop === last) break;
  last = scroller.scrollTop;
  scroller.scrollTop += Math.max(200, scroller.clientHeight * 0.8);
  await sleep(120);
}
document.querySelectorAll(".vrow").forEach(inspect);
// Structured agents (their own log, markdown) are prose, not terminal text.
const reader = ([...document.querySelector("main").classList].find((c) => c.startsWith("reader-")) || "").slice(7);
const prose = document.querySelector(".msg.asst .md p, .msg.asst .md li");
const font = prose ? getComputedStyle(prose).fontFamily : "";
const agentName = (document.querySelector(".pane-bar .badge-a") || {}).dataset?.agent || "";
// The theme picks the page font (some themes are monospace); a reader must not force monospace on top of it for
// agents read from their own log.
const base = getComputedStyle(document.querySelector(".chat-col") || document.body).fontFamily;
if (["claude", "devin", "opencode"].includes(agentName) && font && font !== base)
  problems.push({ kind: "prose font differs from the theme", what: reader + ": " + font.slice(0, 50) + " vs " + base.slice(0, 50) });
return {
  url: decodeURIComponent(location.pathname),
  reader, font: font.split(",")[0],
  agent: (document.querySelector(".pane-bar .badge-a") || {}).dataset?.agent || "",
  rows: seen.size, ...totals,
  problems: problems.slice(0, 12),
  problemCount: problems.length, ok: seen.size > 0 && problems.length === 0,
};
