// The file viewer (ADR 0011, G): a static page, no Blazor circuit. It asks the hub for the
// file's metadata, then draws it the way GitHub would: code with line numbers and
// highlighting (in a worker, one language at a time), markdown preview, CSV/TSV tables,
// a JSON tree, images, PDFs, diffs, or "binary" with a download. Every text is inserted
// as text or as HTML the highlighter or the hub's Markdig pipeline already escaped.
const $ = (id) => document.getElementById(id);
const esc = (s) => s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const machine = decodeURIComponent(location.pathname.split("/")[2] || "");
const q = new URLSearchParams(location.search);
const pane = q.get("pane") || "";
const path = q.get("path") || "";
const body = $("body");
const ROW = 20;

// Back: to the chat this came from (history keeps its scroll), else that pane's chat.
const chat = `/m/${encodeURIComponent(machine)}/p/${encodeURIComponent(pane)}`;
$("back").href = chat;
$("back").addEventListener("click", (e) => {
  if (document.referrer && new URL(document.referrer).origin === location.origin && history.length > 1) { e.preventDefault(); history.back(); }
});

function crumbs(p) {
  const parts = p.split("/").filter(Boolean);
  $("crumbs").innerHTML = parts.map((s, i) => (i === parts.length - 1 ? `<b>${esc(s)}</b>` : `<span>${esc(s)}</span><span> / </span>`)).join("");
  document.title = (parts[parts.length - 1] || "File") + " · klark";
}
crumbs(path);

function fail(msg) { body.innerHTML = `<div class="msg bad">${esc(msg)}</div>`; }

let meta, text = "", lines = [], html = null, view = "code", wrap = false, found = new Set(), sel = 0;

async function main() {
  let r;
  try { r = await fetch(`/api/files/${encodeURIComponent(machine)}?pane=${encodeURIComponent(pane)}&path=${encodeURIComponent(path)}`, { credentials: "same-origin" }); }
  catch { return fail("The hub did not answer."); }
  if (r.status === 401) { location.href = "/login?ReturnUrl=" + encodeURIComponent(location.pathname + location.search + location.hash); return; }
  const j = await r.json().catch(() => ({}));
  if (!r.ok) return fail(j.error || `Error ${r.status}`);
  meta = j;
  crumbs(meta.path);
  $("head").hidden = false;
  $("raw").href = meta.raw;
  $("dl").href = meta.download;
  const parts = [meta.size_label];
  if (meta.lines) parts.push(`${meta.lines.toLocaleString()} lines`);
  if (meta.lang) parts.push(meta.lang);
  $("meta").innerHTML = parts.map((p) => `<span>${esc(String(p))}</span>`).join("");
  if (meta.view === "binary") {
    hideText();
    body.innerHTML = `<div class="msg">Binary file, ${esc(meta.size_label)}. <a class="btn primary" href="${esc(meta.download)}">Download</a></div>`;
    return;
  }
  if (meta.view === "image") {
    hideText();
    body.innerHTML = `<div class="img"><img alt="${esc(meta.name)}" src="${esc(meta.raw)}" /></div>`;
    const box = body.firstChild;
    box.querySelector("img").addEventListener("click", () => box.classList.toggle("zoom"));
    return;
  }
  if (meta.view === "pdf") {
    hideText();
    body.innerHTML = `<iframe class="pdf" title="${esc(meta.name)}" src="${esc(meta.raw)}"></iframe>`;
    return;
  }
  text = await (await fetch(meta.raw, { credentials: "same-origin" })).text();
  lines = text.split("\n");
  if (lines.length > 1 && lines[lines.length - 1] === "") lines.pop();
  const tabs = { markdown: ["Preview", "Code"], csv: ["Table", "Code"], tsv: ["Table", "Code"], json: ["Tree", "Code"] }[meta.view];
  if (tabs) {
    $("tabs").innerHTML = tabs.map((t, i) => `<button type="button" data-i="${i}" class="${i === 0 ? "on" : ""}">${t}</button>`).join("");
    $("tabs").addEventListener("click", (e) => {
      const b = e.target.closest("button");
      if (!b) return;
      $("tabs").querySelectorAll("button").forEach((x) => x.classList.toggle("on", x === b));
      b.dataset.i === "0" ? special() : code();
    });
  }
  // A #L42 link opens the code view on that line.
  if (tabs && !/^#L\d+/.test(location.hash)) special(); else code();
  if (tabs && /^#L\d+/.test(location.hash)) $("tabs").querySelectorAll("button").forEach((x, i) => x.classList.toggle("on", i === 1));
}

function hideText() { for (const id of ["find", "findn", "copy"]) $(id).hidden = true; $("wrap").parentElement.hidden = true; }

async function special() {
  view = meta.view;
  if (meta.view === "markdown") {
    body.innerHTML = `<div class="loading">Rendering…</div>`;
    const r = await fetch(`/api/files/${encodeURIComponent(machine)}/markdown?pane=${encodeURIComponent(pane)}&path=${encodeURIComponent(path)}`, { credentials: "same-origin" });
    // The hub renders with raw HTML disabled and only http/https/mailto links kept.
    body.innerHTML = `<article class="markdown">${r.ok ? await r.text() : esc("Could not render")}</article>`;
    body.querySelectorAll("pre code[class*=language-]").forEach(async (el) => {
      const lang = (el.className.match(/language-([\w+#-]+)/) || [])[1];
      const w = worker();
      w.onmessage = (e) => { if (e.data.lines) el.innerHTML = e.data.lines.join("\n"); w.terminate(); };
      w.postMessage({ text: el.textContent, lang: alias(lang) });
    });
  } else if (meta.view === "csv" || meta.view === "tsv") {
    const rows = parseDelimited(text, meta.view === "tsv" ? "\t" : ",");
    const t = document.createElement("table");
    t.className = "grid";
    const max = Math.min(rows.length, 5000);
    for (let i = 0; i < max; i++) {
      const tr = t.insertRow();
      const n = document.createElement(i === 0 ? "th" : "td");
      n.className = "n";
      n.textContent = i === 0 ? "" : String(i);
      tr.appendChild(n);
      for (const cell of rows[i]) {
        const c = document.createElement(i === 0 ? "th" : "td");
        c.textContent = cell;
        c.title = cell;
        tr.appendChild(c);
      }
    }
    body.replaceChildren(t);
    if (rows.length > max) body.insertAdjacentHTML("beforeend", `<div class="msg">Showing ${max} of ${rows.length} rows. Use Code or Download for all.</div>`);
  } else if (meta.view === "json") {
    let v;
    try { v = JSON.parse(text); } catch (e) { body.innerHTML = `<div class="msg">Not valid JSON (${esc(String(e.message))}); showing the code.</div>`; return code(); }
    const root = document.createElement("div");
    root.className = "json";
    root.appendChild(jsonNode(null, v, 0));
    body.replaceChildren(root);
  }
}

function parseDelimited(s, d) {
  const rows = [];
  let row = [], cell = "", q = false;
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (q) {
      if (c === '"') { if (s[i + 1] === '"') { cell += '"'; i++; } else q = false; }
      else cell += c;
    } else if (c === '"' && cell === "") q = true;
    else if (c === d) { row.push(cell); cell = ""; }
    else if (c === "\n") { row.push(cell.replace(/\r$/, "")); rows.push(row); row = []; cell = ""; }
    else cell += c;
  }
  if (cell !== "" || row.length) { row.push(cell); rows.push(row); }
  return rows;
}

function jsonNode(key, v, depth) {
  const k = key === null ? "" : `<span class="k">${esc(JSON.stringify(key))}</span>: `;
  if (v === null || typeof v !== "object") {
    const d = document.createElement("div");
    d.className = "leaf";
    const cls = typeof v === "string" ? "s" : typeof v === "number" ? "n" : "b";
    d.innerHTML = `${k}<span class="${cls}">${esc(JSON.stringify(v))}</span>`;
    return d;
  }
  const arr = Array.isArray(v);
  const entries = arr ? v.map((x, i) => [i, x]) : Object.entries(v);
  const det = document.createElement("details");
  det.open = depth < 2;
  const sum = document.createElement("summary");
  sum.innerHTML = `${k}${arr ? "[" : "{"}<span class="c"> ${entries.length} ${arr ? "items" : "keys"} </span>${arr ? "]" : "}"}`;
  det.appendChild(sum);
  // Children are built when opened, so a 10 MB document opens at once.
  const fill = () => { if (det.dataset.f) return; det.dataset.f = "1"; for (const [ck, cv] of entries.slice(0, 5000)) det.appendChild(jsonNode(arr ? null : ck, cv, depth + 1)); };
  if (det.open) fill();
  det.addEventListener("toggle", fill);
  return det;
}

const ALIAS = { js: "javascript", ts: "typescript", py: "python", rb: "ruby", rs: "rust", sh: "bash", shell: "bash", zsh: "bash", yml: "yaml",
  md: "markdown", cs: "csharp", html: "xml", toml: "ini", golang: "go", "c++": "cpp", kt: "kotlin" };
const alias = (l) => (l ? ALIAS[l.toLowerCase()] || l.toLowerCase() : "");
const worker = () => new Worker("/viewer-hl.js", { type: "module" });

function code() {
  view = "code";
  const digits = String(lines.length).length;
  body.style.setProperty("--gutter", `${Math.max(3, digits) * 8 + 24}px`);
  body.style.setProperty("--gutter-sm", `${Math.max(2, digits) * 8 + 14}px`);
  const box = document.createElement("div");
  box.className = "code" + (wrap ? " wrap" : "");
  body.replaceChildren(box);
  const diff = meta.view === "diff" || meta.lang === "diff";
  // Highlight in a worker; files over 2 MB stay plain so they open instantly.
  if (!html && text.length < 2_000_000 && meta.lang !== "plaintext") {
    const w = worker();
    w.onmessage = (e) => { w.terminate(); if (e.data.lines && e.data.lines.length >= lines.length) { html = e.data.lines; paint(true); } };
    w.postMessage({ text, lang: alias(meta.lang) });
  }
  let first = -1, last = -1;
  function rowHtml(i) {
    const cls = ["row"];
    if (i + 1 === sel) cls.push("sel");
    if (found.has(i)) cls.push("found");
    if (diff) { const c = lines[i][0]; if (c === "+" && !lines[i].startsWith("+++")) cls.push("d-add"); else if (c === "-" && !lines[i].startsWith("---")) cls.push("d-del"); else if (lines[i].startsWith("@@")) cls.push("d-hunk"); }
    const content = html ? html[i] : esc(lines[i]);
    return `<div class="${cls.join(" ")}" data-l="${i + 1}"><a class="ln" href="#L${i + 1}">${i + 1}</a><span class="lc">${content || " "}</span></div>`;
  }
  // Wrapped lines have no fixed height, so wrap draws everything (and is off for huge files).
  function paint(force) {
    if (wrap) {
      if (force || !box.childElementCount) box.innerHTML = lines.map((_, i) => rowHtml(i)).join("");
      return;
    }
    const top = body.scrollTop, h = body.clientHeight || 800;
    const a = Math.max(0, Math.floor(top / ROW) - 40), b = Math.min(lines.length, Math.ceil((top + h) / ROW) + 40);
    if (!force && a === first && b === last) return;
    first = a; last = b;
    let s = "";
    for (let i = a; i < b; i++) s += rowHtml(i);
    box.style.height = lines.length * ROW + "px";
    box.innerHTML = `<div style="height:${a * ROW}px"></div>` + s;
  }
  code.paint = paint;
  let f = 0;
  body.onscroll = () => { if (!f) f = requestAnimationFrame(() => { f = 0; paint(false); }); };
  paint(true);
  goHash();
}

function goHash() {
  const m = /^#L(\d+)/.exec(location.hash);
  if (!m || view !== "code") return;
  sel = Number(m[1]);
  body.scrollTop = Math.max(0, (sel - 1) * ROW - body.clientHeight / 3);
  if (wrap) body.querySelector(`[data-l="${sel}"]`)?.scrollIntoView({ block: "center" });
  code.paint?.(true);
}
window.addEventListener("hashchange", goHash);
body.addEventListener("click", (e) => {
  const a = e.target.closest("a.ln");
  if (!a) return;
  e.preventDefault();
  history.replaceState(null, "", a.getAttribute("href"));
  sel = Number(a.textContent);
  code.paint?.(true);
});

$("wrap").addEventListener("change", (e) => {
  wrap = e.target.checked && lines.length <= 20000;
  e.target.checked = wrap;
  if (view === "code") code();
});
$("copy").addEventListener("click", async () => {
  try { await navigator.clipboard.writeText(text); }
  catch {
    const ta = Object.assign(document.createElement("textarea"), { value: text });
    document.body.appendChild(ta); ta.select(); document.execCommand("copy"); ta.remove();
  }
  $("copy").textContent = "Copied";
  setTimeout(() => ($("copy").textContent = "Copy"), 1200);
});
let ft = 0, hits = [], at = -1;
$("find").addEventListener("input", (e) => {
  clearTimeout(ft);
  ft = setTimeout(() => {
    const needle = e.target.value.toLowerCase();
    hits = [];
    found = new Set();
    if (needle) for (let i = 0; i < lines.length; i++) if (lines[i].toLowerCase().includes(needle)) { hits.push(i); found.add(i); }
    at = -1;
    $("findn").textContent = needle ? `${hits.length} lines` : "";
    if (view !== "code") { $("tabs").querySelectorAll("button").forEach((x, i, all) => x.classList.toggle("on", i === all.length - 1)); code(); }
    step(1);
  }, 120);
});
$("find").addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); step(e.shiftKey ? -1 : 1); } });
function step(d) {
  if (!hits.length) { code.paint?.(true); return; }
  at = (at + d + hits.length) % hits.length;
  $("findn").textContent = `${at + 1}/${hits.length}`;
  history.replaceState(null, "", `#L${hits[at] + 1}`);
  goHash();
}

main().catch((e) => fail("Viewer error: " + (e && e.message || e)));
