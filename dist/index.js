import { forwardRef, useRef, useImperativeHandle, useEffect, useState, useCallback, memo, createContext, useContext, useMemo, useLayoutEffect, useId } from "react";
import { jsx, jsxs, Fragment } from "react/jsx-runtime";
function backoff(attempt, min = 1e3, max = 3e4) {
  return Math.min(max, min * 2 ** attempt);
}
function connectSSE(href, o) {
  const Ctor = o.EventSourceCtor ?? EventSource;
  let es = null;
  let timer;
  let attempt = 0;
  let opened = 0;
  let stopped = false;
  const open = () => {
    var _a;
    if (stopped) return;
    (_a = o.onState) == null ? void 0 : _a.call(o, "connecting");
    es = new Ctor(href, { withCredentials: true });
    es.onopen = () => {
      var _a2, _b;
      attempt = 0;
      (_a2 = o.onState) == null ? void 0 : _a2.call(o, "open");
      if (++opened > 1) (_b = o.onResync) == null ? void 0 : _b.call(o);
    };
    for (const name of o.events) es.addEventListener(name, (e) => o.onEvent(name, e.data));
    es.onerror = () => {
      var _a2;
      es == null ? void 0 : es.close();
      es = null;
      if (stopped) return;
      (_a2 = o.onState) == null ? void 0 : _a2.call(o, "retrying");
      timer = setTimeout(open, backoff(attempt++, o.minDelay, o.maxDelay));
    };
  };
  open();
  return () => {
    var _a;
    stopped = true;
    clearTimeout(timer);
    es == null ? void 0 : es.close();
    (_a = o.onState) == null ? void 0 : _a.call(o, "closed");
  };
}
class TransportError extends Error {
  constructor(status, message) {
    super(message);
    this.status = status;
  }
}
const enc = encodeURIComponent;
const parse = (s2) => {
  try {
    return JSON.parse(s2);
  } catch {
    return null;
  }
};
function httpTransport(basePath = "/", o = {}) {
  const base = basePath.endsWith("/") ? basePath : basePath + "/";
  const pane = (m, a) => `${base}api/machines/${enc(m)}/panes/${enc(a)}`;
  async function req(path, init = {}) {
    var _a;
    const { json, ...rest } = init;
    const h = new Headers(rest.headers);
    h.set("Accept", "application/json");
    let body = rest.body;
    if (json !== void 0) {
      h.set("Content-Type", "application/json");
      body = JSON.stringify(json);
    }
    let res;
    try {
      res = await (o.fetch ?? fetch)(path, { credentials: "same-origin", ...rest, headers: h, body });
    } catch {
      throw new TransportError(0, "The hub did not answer.");
    }
    (_a = o.onStatus) == null ? void 0 : _a.call(o, res.status);
    if (res.status === 204) return void 0;
    const ct = res.headers.get("content-type") ?? "";
    const data = ct.includes("json") ? await res.json().catch(() => ({})) : await res.text().catch(() => "");
    if (!res.ok) {
      const msg = typeof data === "object" && data && "error" in data ? String(data.error) : `Error ${res.status}`;
      throw new TransportError(res.status, msg);
    }
    return data;
  }
  const t = {
    machines: () => req(`${base}api/machines`),
    tree: (m) => req(`${base}api/machines/${enc(m)}/tree`),
    getChat: (m, a) => req(`${pane(m, a)}/chat`),
    events: (m, a, { frames }, h) => connectSSE(`${pane(m, a)}/events${frames ? "?frames=1" : ""}`, {
      events: ["chat", "frame", "size", "offer"],
      EventSourceCtor: o.EventSource,
      onState: h.state,
      onResync: h.resync,
      onEvent: (name, data) => {
        var _a, _b, _c, _d;
        if (name === "chat") {
          const p = parse(data);
          if (p) (_a = h.chat) == null ? void 0 : _a.call(h, Array.isArray(p) ? { rows: p } : "rows" in p ? p : { rows: [p] });
        } else if (name === "frame") {
          const f = parse(data);
          (_b = h.frame) == null ? void 0 : _b.call(h, f && typeof f === "object" ? f : { data });
        } else if (name === "size") {
          const s2 = parse(data);
          if (s2) (_c = h.size) == null ? void 0 : _c.call(h, s2);
        } else if (name === "offer") {
          const of = parse(data);
          if (of) (_d = h.offer) == null ? void 0 : _d.call(h, of);
        }
      }
    }),
    input: (m, a, text) => req(`${pane(m, a)}/input`, { method: "POST", json: { text } }),
    keys: (m, a, keys) => req(`${pane(m, a)}/keys`, { method: "POST", json: { keys } }),
    send: async (m, a, text) => {
      await t.input(m, a, text);
      await t.keys(m, a, ["enter"]);
    },
    upload: (m, a, file) => {
      const fd = new FormData();
      fd.append("file", file, file.name);
      return req(`${pane(m, a)}/upload`, { method: "POST", body: fd });
    },
    fileUrl: (m, a, path, line) => `${base}m/${enc(m)}/f?path=${enc(path)}&pane=${enc(a)}${line ? "#L" + line : ""}`
  };
  return t;
}
const isRun = (k) => k === "tool_use" || k === "tool_result" || k === "thinking";
const make = (kind, items) => ({
  key: items[0].id,
  kind,
  items,
  // Changes only when the row's content does, so memoised rows skip re-rendering.
  stamp: `${items.length}:${items[items.length - 1].id}:${items.reduce((n, i) => {
    var _a, _b;
    return n + (((_a = i.html) == null ? void 0 : _a.length) ?? 0) + (((_b = i.text) == null ? void 0 : _b.length) ?? 0);
  }, 0)}`
});
function buildRows(items) {
  const rows = [];
  let run = [];
  const flush = () => {
    if (run.length) rows.push(make("tools", run));
    run = [];
  };
  for (let n = 0; n < items.length; n++) {
    const i = items[n];
    if (isRun(i.kind)) {
      run.push(i);
      continue;
    }
    flush();
    if (i.kind === "command") {
      const next = items[n + 1];
      rows.push(make("block", next && next.kind === "output" ? [i, items[++n]] : [i]));
      continue;
    }
    if (i.kind === "output") {
      rows.push(make("block", [i]));
      continue;
    }
    if (!i.text && !i.html) continue;
    rows.push(make(i.role === "user" || i.kind === "user" ? "user" : "text", [i]));
  }
  flush();
  return rows;
}
function mergeItems(cur, incoming) {
  if (!incoming.length) return cur;
  const at = new Map(cur.map((r, i) => [r.id, i]));
  const next = cur.slice();
  for (const r of incoming) {
    const i = at.get(r.id);
    if (i === void 0) {
      at.set(r.id, next.length);
      next.push(r);
    } else next[i] = r;
  }
  return next;
}
function pairCalls(items) {
  const list = [];
  for (const i of items) {
    if (i.kind === "tool_result") {
      let open = i.toolUseId ? list.findLastIndex((c) => {
        var _a;
        return c.kind === "tool" && ((_a = c.use) == null ? void 0 : _a.toolUseId) === i.toolUseId && !c.result;
      }) : -1;
      if (open < 0) open = list.findLastIndex((c) => c.kind === "tool" && c.use && !c.result);
      if (open >= 0) list[open] = { ...list[open], result: i };
      else list.push({ key: i.id, kind: "tool", result: i });
    } else if (i.kind === "thinking") list.push({ key: i.id, kind: "thinking", use: i });
    else list.push({ key: i.id, kind: "tool", use: i });
  }
  return list;
}
function verb(tool) {
  switch (tool) {
    case "Read":
    case "NotebookRead":
      return ["Read", "file"];
    case "Edit":
    case "MultiEdit":
    case "str_replace":
    case "NotebookEdit":
      return ["Edited", "file"];
    case "Write":
      return ["Wrote", "file"];
    case "Bash":
    case "BashOutput":
    case "shell":
    case "exec":
      return ["Ran", "command"];
    case "Grep":
    case "Glob":
    case "search":
      return ["Searched", "pattern"];
    case "WebSearch":
      return ["Searched the web", "query"];
    case "WebFetch":
      return ["Fetched", "page"];
    case "Task":
    case "Agent":
      return ["Ran", "agent"];
    case "TodoWrite":
      return ["Updated", "todo list"];
    default:
      return [tool, "call"];
  }
}
function toolIcon(tool) {
  switch (verb(tool)[0]) {
    case "Ran":
      return tool === "Task" || tool === "Agent" ? "◆" : "$";
    case "Read":
      return "◉";
    case "Edited":
    case "Wrote":
      return "✎";
    case "Searched":
    case "Searched the web":
      return "⌕";
    case "Fetched":
      return "↗";
    case "Updated":
      return "☰";
    default:
      return "•";
  }
}
function phrase(tools, thinking) {
  const groups = /* @__PURE__ */ new Map();
  for (const t of tools) {
    const [v, n] = verb(t);
    const k = v + "|" + n;
    const g = groups.get(k) ?? { v, n, count: 0 };
    g.count++;
    groups.set(k, g);
  }
  const parts = [...groups.values()].map(({ v, n, count }) => {
    if (v === n || n === "call") return count === 1 ? v : `${v} ×${count}`;
    const plural = n.endsWith("y") ? n.slice(0, -1) + "ies" : n + "s";
    return `${v} ${count} ${count === 1 ? n : plural}`;
  });
  if (thinking > 0) parts.push("thought");
  return parts.join(" · ");
}
function offsets(count, heights, key, estimate) {
  const out = new Array(count + 1);
  out[0] = 0;
  for (let i = 0; i < count; i++) {
    const h = typeof heights === "function" ? heights(i) : heights.get(key(i)) ?? estimate;
    out[i + 1] = out[i] + h;
  }
  return out;
}
function computeWindow(offs, scrollTop, viewport, overscan) {
  const count = offs.length - 1;
  const total = offs[count] ?? 0;
  if (count <= 0) return { start: 0, end: 0, padTop: 0, padBottom: 0, total };
  const lo = Math.max(0, scrollTop - overscan);
  const hi = scrollTop + viewport + overscan;
  const find = (y) => {
    let a = 0, b = count;
    while (a < b) {
      const m = a + b >> 1;
      if (offs[m + 1] > y) b = m;
      else a = m + 1;
    }
    return a;
  };
  const start = Math.min(find(lo), count - 1);
  let end = find(hi) + 1;
  end = Math.min(count, Math.max(end, start + 1));
  return { start, end, padTop: offs[start], padBottom: total - offs[end], total };
}
function atBottom(scrollTop, viewport, total, slack = 80) {
  return total - (scrollTop + viewport) <= slack;
}
const paneId = (p) => p.pane_id;
const paneCwd = (p) => p.foreground_cwd || p.cwd || "";
const paneAgent = (p) => p.display_agent || p.agent || "";
const paneStatus = (p) => p.agent_status && p.agent_status !== "unknown" ? p.agent_status : "";
const isAgent = (p) => paneAgent(p).length > 0 || paneStatus(p) !== "";
function agentKey(agent) {
  const a = agent.toLowerCase();
  return a.includes("claude") ? "claude" : a.includes("devin") ? "devin" : a.includes("codex") ? "codex" : a === "" ? "shell" : "other";
}
function agentIcon(agent) {
  switch (agentKey(agent)) {
    case "claude":
      return "✻";
    case "devin":
      return "◆";
    case "codex":
      return "◎";
    case "shell":
      return "$";
    default:
      return agent ? agent[0].toUpperCase() : "•";
  }
}
function folderName(cwd) {
  const s2 = (cwd ?? "").trim().replace(/\/+$/, "");
  if (s2 === "" || s2 === "~" || /^\/(Users|home)\/[^/]+$/.test(s2)) return "~";
  return s2.slice(s2.lastIndexOf("/") + 1);
}
function displayName(p) {
  const title = (p.label || p.title || p.terminal_title_stripped || p.pane_id || "").replace(/^[✳✻✽✶·*⠂⠐ ]+/, "");
  const agent = paneAgent(p);
  const cwd = paneCwd(p);
  const raw = title === "" || title === p.pane_id || /^w?\d+[:/]p?\d+$/.test(title) || title.toLowerCase() === agent.toLowerCase() || title === cwd || ["zsh", "bash", "sh", "fish"].includes(title) || /^(?:[\w.-]+@[\w.-]+:)?(?:~|\.{0,2})?\//.test(title);
  if (!raw) return title;
  const folder = cwd.replace(/\/+$/, "") ? cwd.replace(/\/+$/, "").split("/").pop() ?? "" : "";
  if (agent) return folder ? `${agent} · ${folder}` : agent;
  return folder || title || p.pane_id;
}
function workspaceLabel(t, id) {
  var _a;
  const w = (_a = t == null ? void 0 : t.workspaces) == null ? void 0 : _a.find((x) => x.workspace_id === id);
  return w ? w.label || `workspace ${w.number ?? ""}`.trim() : id ?? "";
}
const statusRank = (s2) => s2 === "blocked" ? 3 : s2 === "working" ? 2 : s2 === "idle" ? 1 : 0;
function matches(r, words) {
  if (!words.length) return true;
  const p = r.pane;
  const hay = `${r.machineName} ${r.session} ${displayName(p)} ${p.title ?? ""} ${paneAgent(p)} ${paneStatus(p)} ${paneCwd(p)} ${folderName(paneCwd(p))} ${p.pane_id}`.toLowerCase();
  return words.every((w) => hay.includes(w.toLowerCase()));
}
function groupRows(rows, filter, showAll) {
  const words = filter.split(/\s+/).filter(Boolean);
  const kept = rows.filter((r) => (showAll || isAgent(r.pane)) && matches(r, words));
  const by = /* @__PURE__ */ new Map();
  for (const r of kept) {
    const k = folderName(paneCwd(r.pane));
    by.set(k, [...by.get(k) ?? [], r]);
  }
  const rank = (rs) => Math.max(...rs.map((r) => statusRank(paneStatus(r.pane))));
  return [...by.entries()].map(([key, rs]) => ({
    key,
    name: key,
    path: paneCwd(rs[0].pane),
    rows: rs.slice().sort((a, b) => statusRank(paneStatus(b.pane)) - statusRank(paneStatus(a.pane)) || displayName(a.pane).localeCompare(displayName(b.pane)))
  })).sort((a, b) => rank(b.rows) - rank(a.rows) || a.name.localeCompare(b.name, void 0, { sensitivity: "base" }));
}
function webStorage(area, prefix = "") {
  const a = area ?? (() => localStorage);
  return {
    get(key) {
      try {
        return a().getItem(prefix + key);
      } catch {
        return null;
      }
    },
    set(key, value) {
      try {
        if (value === null || value === "") a().removeItem(prefix + key);
        else a().setItem(prefix + key, value);
      } catch {
      }
    }
  };
}
function memoryStorage() {
  const m = /* @__PURE__ */ new Map();
  return { get: (k) => m.get(k) ?? null, set: (k, v) => {
    if (v === null || v === "") m.delete(k);
    else m.set(k, v);
  } };
}
const defaultStorage = webStorage();
const MIN_SCALE = 0.55;
function decodeBase64(b64) {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}
const Terminal = forwardRef(function Terminal2({ cols, rows, visible, theme, fontSize = 13 }, ref) {
  const wrap = useRef(null);
  const sizer = useRef(null);
  const inner = useRef(null);
  const term = useRef(null);
  const queue = useRef([]);
  const geom = useRef({ cols, rows });
  geom.current = { cols, rows };
  const scale = () => {
    var _a;
    const screen = (_a = inner.current) == null ? void 0 : _a.querySelector(".xterm-screen");
    const w = wrap.current, sz = sizer.current, inn = inner.current;
    if (!screen || !w || !sz || !inn || !w.clientWidth) return;
    const sw = screen.offsetWidth, sh = screen.offsetHeight;
    if (!sw || !sh) return;
    const s2 = Math.max(MIN_SCALE, Math.min(1, (w.clientWidth - 2) / sw));
    inn.style.transform = `scale(${s2})`;
    sz.style.width = `${Math.ceil(sw * s2)}px`;
    sz.style.height = `${Math.ceil(sh * s2)}px`;
  };
  const run = (f) => {
    if (term.current) f();
    else queue.current.push(f);
  };
  useImperativeHandle(ref, () => ({
    write: (b) => run(() => {
      var _a;
      return (_a = term.current) == null ? void 0 : _a.write(b);
    }),
    reset: (b) => run(() => {
      var _a, _b;
      (_a = term.current) == null ? void 0 : _a.reset();
      (_b = term.current) == null ? void 0 : _b.write(b);
    }),
    resize: (c, r) => run(() => {
      if (term.current && (term.current.cols !== c || term.current.rows !== r)) {
        term.current.resize(c, r);
        requestAnimationFrame(scale);
      }
    })
  }));
  useEffect(() => {
    if (!visible || term.current) return;
    let dead = false;
    (async () => {
      const [{ Terminal: XTerm }, { Unicode11Addon }] = await Promise.all([import("@xterm/xterm"), import("@xterm/addon-unicode11"), import("@xterm/xterm/css/xterm.css")]);
      if (dead || !inner.current) return;
      const t = new XTerm({
        cols: geom.current.cols,
        rows: geom.current.rows,
        convertEol: false,
        disableStdin: true,
        allowProposedApi: true,
        fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
        fontSize,
        scrollback: 5e3,
        theme: theme ?? { background: "#0d1117", foreground: "#e6edf3", cursor: "#e6edf3", selectionBackground: "#264f78" }
      });
      t.loadAddon(new Unicode11Addon());
      t.unicode.activeVersion = "11";
      t.open(inner.current);
      term.current = t;
      queue.current.splice(0).forEach((f) => f());
      requestAnimationFrame(scale);
    })();
    return () => {
      dead = true;
    };
  }, [visible]);
  useEffect(() => {
    const w = wrap.current;
    if (!w || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(scale);
    ro.observe(w);
    return () => ro.disconnect();
  }, []);
  useEffect(() => () => {
    var _a;
    (_a = term.current) == null ? void 0 : _a.dispose();
    term.current = null;
  }, []);
  useEffect(() => {
    if (visible) requestAnimationFrame(scale);
  }, [visible]);
  return /* @__PURE__ */ jsx("div", { className: "terminal-view", style: { display: visible ? void 0 : "none" }, "aria-label": "Terminal", children: /* @__PURE__ */ jsx("div", { className: "term-wrap", ref: wrap, children: /* @__PURE__ */ jsx("div", { className: "term-sizer", ref: sizer, children: /* @__PURE__ */ jsx("div", { className: "term-inner", ref: inner }) }) }) });
});
function useChat(transport, machine, agent, frames = false, term) {
  const [items, setItems] = useState([]);
  const [offers, setOffers] = useState([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);
  const [live, setLive] = useState("connecting");
  const [size, setSize] = useState({ cols: 120, rows: 40 });
  const [blocked, setBlocked] = useState(null);
  const load = useCallback(() => transport.getChat(machine, agent).then((p) => {
    setBlocked(p.blocked ?? null);
    setItems((cur) => cur.length ? mergeItems(cur, p.rows ?? []) : p.rows ?? []);
    setOffers(p.offers ?? []);
    setError(null);
  }).catch((e) => setError(e.message)).finally(() => setLoading(false)), [transport, machine, agent]);
  useEffect(() => {
    setItems([]);
    setOffers([]);
    setBlocked(null);
    setLoading(true);
    void load();
  }, [load]);
  useEffect(() => transport.events(machine, agent, { frames }, {
    state: setLive,
    resync: () => void load(),
    chat: (p) => {
      setBlocked(p.blocked ?? null);
      setItems((cur) => mergeItems(cur, p.rows ?? []));
    },
    frame: (f) => {
      var _a, _b;
      const b = decodeBase64(f.data);
      if (f.reset) (_a = term == null ? void 0 : term.current) == null ? void 0 : _a.reset(b);
      else (_b = term == null ? void 0 : term.current) == null ? void 0 : _b.write(b);
    },
    size: (s2) => {
      var _a;
      setSize(s2);
      (_a = term == null ? void 0 : term.current) == null ? void 0 : _a.resize(s2.cols, s2.rows);
    },
    offer: (o) => setOffers((cur) => cur.some((x) => x.id === o.id) ? cur : [...cur, o])
  }), [transport, machine, agent, frames, load, term]);
  return { items, offers, loading, error, live, size, blocked, reload: load };
}
const OpenCtx = createContext({ has: () => false, set: () => {
} });
const FileHrefCtx = createContext(null);
const fmt = (t) => t ? new Date(t).toLocaleString() : void 0;
function Details({ k, defaultOpen, className, children }) {
  const o = useContext(OpenCtx);
  const open = o.has("x:" + k) ? o.has(k) : !!defaultOpen;
  return /* @__PURE__ */ jsx("details", { className, open, onToggle: (e) => {
    const v = e.currentTarget.open;
    if (v !== open) {
      o.set("x:" + k, true);
      o.set(k, v);
    }
  }, children: children(open) });
}
function Html({ html, cls }) {
  return /* @__PURE__ */ jsx("div", { className: cls, dangerouslySetInnerHTML: { __html: html ?? "" } });
}
function Call({ c }) {
  var _a, _b, _c, _d;
  const fileHref = useContext(FileHrefCtx);
  if (c.kind === "thinking") {
    return /* @__PURE__ */ jsx(Details, { k: "t:" + c.key, className: "tool", children: () => {
      var _a2;
      return /* @__PURE__ */ jsxs(Fragment, { children: [
        /* @__PURE__ */ jsxs("summary", { children: [
          /* @__PURE__ */ jsx("span", { className: "ico", children: "✦" }),
          /* @__PURE__ */ jsx("span", { className: "t", children: "Thought" })
        ] }),
        /* @__PURE__ */ jsx("pre", { className: "tool-body", children: (_a2 = c.use) == null ? void 0 : _a2.text })
      ] });
    } });
  }
  const name = ((_a = c.use) == null ? void 0 : _a.tool) ?? ((_b = c.result) == null ? void 0 : _b.tool) ?? "Tool";
  const arg = ((_c = c.use) == null ? void 0 : _c.summary) ?? "";
  const [v, noun] = verb(name);
  const isFile = noun === "file" && arg.startsWith("/") && !!fileHref;
  const failed = ((_d = c.result) == null ? void 0 : _d.ok) === false;
  return /* @__PURE__ */ jsx(Details, { k: "t:" + c.key, className: "tool" + (failed ? " failed" : ""), children: () => {
    var _a2, _b2, _c2, _d2;
    return /* @__PURE__ */ jsxs(Fragment, { children: [
      /* @__PURE__ */ jsxs("summary", { title: fmt(((_a2 = c.use) == null ? void 0 : _a2.time) ?? ((_b2 = c.result) == null ? void 0 : _b2.time)), children: [
        /* @__PURE__ */ jsx("span", { className: "ico", children: toolIcon(name) }),
        /* @__PURE__ */ jsx("span", { className: "t", children: v }),
        arg && (isFile ? /* @__PURE__ */ jsx("a", { className: "arg fpath", href: fileHref(arg), children: arg }) : /* @__PURE__ */ jsx("span", { className: "arg", children: arg })),
        !c.result ? /* @__PURE__ */ jsx("span", { className: "state", children: "…" }) : failed ? /* @__PURE__ */ jsx("span", { className: "state bad", "aria-label": "failed", children: "✗" }) : c.result.ok ? /* @__PURE__ */ jsx("span", { className: "state ok", "aria-label": "done", children: "✓" }) : null
      ] }),
      ((_c2 = c.use) == null ? void 0 : _c2.input) && /* @__PURE__ */ jsx("pre", { className: "tool-body", children: c.use.input }),
      ((_d2 = c.result) == null ? void 0 : _d2.text) && /* @__PURE__ */ jsx("pre", { className: "tool-body result", children: c.result.text })
    ] });
  } });
}
function Body({ item }) {
  if (item.kind === "output") return /* @__PURE__ */ jsx("pre", { className: "out", dangerouslySetInnerHTML: { __html: item.html ?? "" } });
  return item.html != null ? /* @__PURE__ */ jsx(Html, { html: item.html, cls: "md" }) : /* @__PURE__ */ jsx("div", { className: "md", children: /* @__PURE__ */ jsx("p", { children: item.text }) });
}
function ChatRowImpl({ row }) {
  const first = row.items[0];
  switch (row.kind) {
    case "user":
      return /* @__PURE__ */ jsx("div", { className: "msg user", "data-id": row.key, title: fmt(first.time), children: /* @__PURE__ */ jsx("div", { className: "bubble", children: first.text ?? stripTags(first.html) }) });
    case "text":
      return /* @__PURE__ */ jsx("div", { className: "msg asst", "data-id": row.key, title: fmt(first.time), children: /* @__PURE__ */ jsx(Body, { item: first }) });
    case "block": {
      const cmd = row.items.find((i) => i.kind === "command");
      const out = row.items.find((i) => i.kind === "output");
      const lines = (out == null ? void 0 : out.lines) ?? ((out == null ? void 0 : out.html) ? out.html.split("\n").length : 0);
      return /* @__PURE__ */ jsx(Details, { k: "b:" + row.key, className: "block", defaultOpen: lines <= 40, children: () => /* @__PURE__ */ jsxs(Fragment, { children: [
        /* @__PURE__ */ jsxs("summary", { children: [
          /* @__PURE__ */ jsx("span", { className: "prompt", children: "$" }),
          /* @__PURE__ */ jsx("span", { className: "cmd", children: (cmd == null ? void 0 : cmd.text) ?? "output" }),
          lines > 0 && /* @__PURE__ */ jsxs("span", { className: "state", children: [
            lines,
            " ",
            lines === 1 ? "line" : "lines"
          ] })
        ] }),
        out && /* @__PURE__ */ jsx(Body, { item: out })
      ] }) });
    }
    default: {
      const calls = pairCalls(row.items);
      if (calls.length === 1) return /* @__PURE__ */ jsx("div", { "data-id": row.key, children: /* @__PURE__ */ jsx(Call, { c: calls[0] }) });
      const tools = calls.filter((c) => c.kind === "tool");
      const label = phrase(tools.map((c) => {
        var _a, _b;
        return ((_a = c.use) == null ? void 0 : _a.tool) ?? ((_b = c.result) == null ? void 0 : _b.tool) ?? "Tool";
      }), calls.length - tools.length);
      const bad = tools.some((c) => {
        var _a;
        return ((_a = c.result) == null ? void 0 : _a.ok) === false;
      });
      const pending = tools.some((c) => !c.result);
      const ok = tools.length > 0 && tools.every((c) => {
        var _a;
        return ((_a = c.result) == null ? void 0 : _a.ok) === true;
      });
      return /* @__PURE__ */ jsx(Details, { k: "g:" + row.key, className: "tools", children: () => /* @__PURE__ */ jsxs(Fragment, { children: [
        /* @__PURE__ */ jsxs("summary", { children: [
          /* @__PURE__ */ jsx("span", { className: "ico", children: "⋯" }),
          /* @__PURE__ */ jsx("span", { className: "t", children: label }),
          bad ? /* @__PURE__ */ jsx("span", { className: "state bad", "aria-label": "failed", children: "✗" }) : pending ? /* @__PURE__ */ jsx("span", { className: "state", children: "…" }) : ok ? /* @__PURE__ */ jsx("span", { className: "state ok", "aria-label": "done", children: "✓" }) : null
        ] }),
        /* @__PURE__ */ jsx("div", { className: "tool-list", children: calls.map((c) => /* @__PURE__ */ jsx(Call, { c }, c.key)) })
      ] }) });
    }
  }
}
function stripTags(html) {
  return (html ?? "").replace(/<[^>]*>/g, "").replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, '"').replace(/&#39;/g, "'").replace(/&amp;/g, "&");
}
const ChatRow = memo(ChatRowImpl, (a, b) => a.row.stamp === b.row.stamp && a.row.kind === b.row.kind);
function choiceKeys(b, key) {
  const keys = (b.choices ?? []).map((c) => c.key);
  const to = keys.indexOf(key);
  const from = b.selected ? keys.indexOf(b.selected) : -1;
  if (to < 0) return [];
  if (from < 0) return [key];
  const d = to - from;
  return [...Array(Math.abs(d)).fill(d > 0 ? "down" : "up"), "enter"];
}
function ChoiceBar({ blocked, onKeys }) {
  const [busy, setBusy] = useState(false);
  const choices = blocked.choices ?? [];
  if (!choices.length) return null;
  const pick = async (keys) => {
    if (!keys.length || busy) return;
    setBusy(true);
    try {
      await onKeys(keys);
    } finally {
      setBusy(false);
    }
  };
  return /* @__PURE__ */ jsxs("div", { className: "blocked chat-col", role: "group", "aria-label": "Agent is waiting for your answer", children: [
    /* @__PURE__ */ jsx("div", { className: "blocked-label", children: "Waiting for you" }),
    blocked.question && /* @__PURE__ */ jsx("pre", { className: "blocked-q", children: blocked.question }),
    /* @__PURE__ */ jsxs("div", { className: "quick", children: [
      choices.map((c) => /* @__PURE__ */ jsxs(
        "button",
        {
          type: "button",
          disabled: busy,
          className: c.key === blocked.selected ? "chosen" : void 0,
          onClick: () => void pick(choiceKeys(blocked, c.key)),
          children: [
            c.key,
            ". ",
            c.label
          ]
        },
        c.key
      )),
      /* @__PURE__ */ jsx("button", { type: "button", className: "ghost", disabled: busy, onClick: () => void pick(["esc"]), children: "Esc" })
    ] })
  ] });
}
function VirtualListInner({ items, getKey, render, estimate = 90, overscan = 900, className = "", onPinnedChange, header }, ref) {
  const sc = useRef(null);
  const heights = useRef(/* @__PURE__ */ new Map());
  const nodes = useRef(/* @__PURE__ */ new Map());
  const [version, setVersion] = useState(0);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewport, setViewport] = useState(() => typeof window === "undefined" ? 800 : window.innerHeight);
  const pinned = useRef(true);
  const ro = useRef(null);
  const offs = useMemo(
    () => offsets(items.length, heights.current, (i) => getKey(items[i]), estimate),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [items, version, estimate]
  );
  const total = offs[items.length];
  const effTop = pinned.current ? Math.max(0, total - viewport) : scrollTop;
  const w = computeWindow(offs, effTop, viewport, overscan);
  const measure = useCallback(() => {
    let changed = false;
    let above = 0;
    const el = sc.current;
    const top = (el == null ? void 0 : el.scrollTop) ?? 0;
    const o = offs;
    items.forEach((it, i) => {
      const n = nodes.current.get(getKey(it));
      if (!n) return;
      const h = n.offsetHeight;
      if (h <= 0) return;
      const k = getKey(it);
      const prev = heights.current.get(k) ?? estimate;
      if (Math.abs(prev - h) > 0.5) {
        heights.current.set(k, h);
        changed = true;
        if (o[i + 1] <= top) above += h - prev;
      } else if (!heights.current.has(k)) heights.current.set(k, h);
    });
    if (changed) {
      if (above && el && !pinned.current) el.scrollTop = top + above;
      setVersion((v) => v + 1);
    }
  }, [items, getKey, estimate, offs]);
  useLayoutEffect(() => {
    measure();
    if (pinned.current && sc.current) sc.current.scrollTop = sc.current.scrollHeight;
  });
  useLayoutEffect(() => {
    const el = sc.current;
    if (!el) return;
    const size = () => setViewport(el.clientHeight || window.innerHeight);
    size();
    if (typeof ResizeObserver === "undefined") return;
    const obs = new ResizeObserver(() => {
      size();
    });
    obs.observe(el);
    ro.current = new ResizeObserver(() => measure());
    nodes.current.forEach((n) => {
      var _a;
      return (_a = ro.current) == null ? void 0 : _a.observe(n);
    });
    return () => {
      var _a;
      obs.disconnect();
      (_a = ro.current) == null ? void 0 : _a.disconnect();
    };
  }, []);
  const onScroll = () => {
    const el = sc.current;
    if (!el) return;
    setScrollTop(el.scrollTop);
    const p = atBottom(el.scrollTop, el.clientHeight || viewport, el.scrollHeight);
    if (p !== pinned.current) {
      pinned.current = p;
      onPinnedChange == null ? void 0 : onPinnedChange(p);
    }
  };
  useImperativeHandle(ref, () => ({
    scrollToBottom() {
      pinned.current = true;
      onPinnedChange == null ? void 0 : onPinnedChange(true);
      if (sc.current) sc.current.scrollTop = sc.current.scrollHeight;
      setVersion((v) => v + 1);
    },
    scrollToIndex(i) {
      pinned.current = false;
      onPinnedChange == null ? void 0 : onPinnedChange(false);
      const y = Math.max(0, (offs[i] ?? 0) - 60);
      if (sc.current) sc.current.scrollTop = y;
      setScrollTop(y);
    }
  }));
  return /* @__PURE__ */ jsx("div", { className: "chat-scroll " + className, ref: sc, onScroll, "data-testid": "chat-scroll", children: /* @__PURE__ */ jsxs("div", { className: "chat-col", children: [
    header,
    /* @__PURE__ */ jsx("div", { className: "chat-pad", style: { height: w.padTop }, "aria-hidden": "true" }),
    items.slice(w.start, w.end).map((it, n) => {
      const k = getKey(it);
      return /* @__PURE__ */ jsx(
        "div",
        {
          className: "vrow",
          "data-key": k,
          ref: (el) => {
            var _a, _b;
            const prev = nodes.current.get(k);
            if (prev && prev !== el) (_a = ro.current) == null ? void 0 : _a.unobserve(prev);
            if (el) {
              nodes.current.set(k, el);
              (_b = ro.current) == null ? void 0 : _b.observe(el);
            } else nodes.current.delete(k);
          },
          children: render(it, w.start + n)
        },
        k
      );
    }),
    /* @__PURE__ */ jsx("div", { className: "chat-pad", style: { height: w.padBottom }, "aria-hidden": "true" })
  ] }) });
}
const VirtualList = forwardRef(VirtualListInner);
const keyOf = (r) => r.key;
const ChatView = forwardRef(function ChatView2({ items, loading, error, blocked, status, offline, onKeys, fileHref, onNavigate, searchOpen, onSearchClose, footer, className = "" }, ref) {
  const list = useRef(null);
  const rows = useMemo(() => buildRows(items), [items]);
  const [pinned, setPinned] = useState(true);
  const [needle, setNeedle] = useState("");
  const [hit, setHit] = useState(0);
  const open = useRef(/* @__PURE__ */ new Set());
  const openCtx = useMemo(() => ({ has: (k) => open.current.has(k), set: (k, v) => {
    if (v) open.current.add(k);
    else open.current.delete(k);
  } }), []);
  useImperativeHandle(ref, () => ({ scrollToBottom: () => {
    var _a;
    return (_a = list.current) == null ? void 0 : _a.scrollToBottom();
  } }));
  const matches2 = useMemo(() => {
    const n = needle.trim().toLowerCase();
    if (!n) return [];
    return rows.flatMap((r, i) => r.items.some((it) => (it.text ?? stripTags(it.html) + " " + (it.summary ?? "")).toLowerCase().includes(n)) ? [i] : []);
  }, [rows, needle]);
  const jump = (d) => {
    var _a;
    if (!matches2.length) return;
    const n = (hit + d + matches2.length) % matches2.length;
    setHit(n);
    (_a = list.current) == null ? void 0 : _a.scrollToIndex(matches2[n]);
  };
  const onClick = (e) => {
    var _a, _b, _c;
    const t = e.target;
    const copy = t.closest(".codeblk .copy");
    if (copy) {
      const code = ((_b = (_a = copy.closest(".codeblk")) == null ? void 0 : _a.querySelector("code")) == null ? void 0 : _b.textContent) ?? "";
      void ((_c = navigator.clipboard) == null ? void 0 : _c.writeText(code).then(() => {
        copy.textContent = "Copied";
        setTimeout(() => {
          copy.textContent = "Copy";
        }, 1500);
      }));
      return;
    }
    const a = t.closest("a[href]");
    if (!a || !onNavigate || a.target === "_blank" || e.metaKey || e.ctrlKey || e.shiftKey || e.button !== 0) return;
    if (onNavigate(a.href, a)) e.preventDefault();
  };
  const header = loading ? /* @__PURE__ */ jsxs("div", { className: "skel-chat", "aria-hidden": "true", children: [
    /* @__PURE__ */ jsx("div", { className: "skel bubble-r" }),
    /* @__PURE__ */ jsx("div", { className: "skel line w90" }),
    /* @__PURE__ */ jsx("div", { className: "skel line w80" }),
    /* @__PURE__ */ jsx("div", { className: "skel tool" })
  ] }) : rows.length === 0 ? /* @__PURE__ */ jsx("p", { className: "empty chat-empty", children: error ?? "Nothing here yet." }) : null;
  return /* @__PURE__ */ jsxs("div", { className: "chat " + className, onClick, children: [
    searchOpen && /* @__PURE__ */ jsxs("div", { className: "chat-search", role: "search", children: [
      /* @__PURE__ */ jsx(
        "input",
        {
          type: "search",
          autoFocus: true,
          "aria-label": "Search in chat",
          placeholder: "Search in chat",
          value: needle,
          onChange: (e) => {
            setNeedle(e.target.value);
            setHit(0);
          },
          onKeyDown: (e) => {
            if (e.key === "Enter") jump(e.shiftKey ? -1 : 1);
            if (e.key === "Escape") onSearchClose == null ? void 0 : onSearchClose();
          }
        }
      ),
      /* @__PURE__ */ jsx("span", { className: "muted", "aria-live": "polite", children: needle ? `${matches2.length ? hit + 1 : 0}/${matches2.length}` : "" }),
      /* @__PURE__ */ jsx("button", { type: "button", className: "icon", "aria-label": "Previous match", onClick: () => jump(-1), children: "↑" }),
      /* @__PURE__ */ jsx("button", { type: "button", className: "icon", "aria-label": "Next match", onClick: () => jump(1), children: "↓" })
    ] }),
    /* @__PURE__ */ jsx(FileHrefCtx.Provider, { value: fileHref ?? null, children: /* @__PURE__ */ jsx(OpenCtx.Provider, { value: openCtx, children: /* @__PURE__ */ jsx(VirtualList, { ref: list, items: rows, getKey: keyOf, onPinnedChange: setPinned, header, render: (r) => /* @__PURE__ */ jsx(ChatRow, { row: r }) }) }) }),
    footer,
    blocked && !offline && onKeys && /* @__PURE__ */ jsx(ChoiceBar, { blocked, onKeys: async (keys) => {
      var _a;
      await onKeys(keys);
      (_a = list.current) == null ? void 0 : _a.scrollToBottom();
    } }),
    !pinned && /* @__PURE__ */ jsx("button", { type: "button", className: "jump show", onClick: () => {
      var _a;
      return (_a = list.current) == null ? void 0 : _a.scrollToBottom();
    }, children: "Jump to latest" }),
    status === "working" && !offline && /* @__PURE__ */ jsxs("div", { className: "live live-working", role: "status", children: [
      /* @__PURE__ */ jsx("span", { className: "dot", "aria-hidden": "true" }),
      "working…"
    ] })
  ] });
});
const s = { width: 20, height: 20, viewBox: "0 0 24 24", fill: "none", stroke: "currentColor", strokeWidth: 1.8, strokeLinecap: "round", strokeLinejoin: "round", "aria-hidden": true };
const IconSidebar = () => /* @__PURE__ */ jsxs("svg", { ...s, children: [
  /* @__PURE__ */ jsx("rect", { x: "3", y: "4", width: "18", height: "16", rx: "3" }),
  /* @__PURE__ */ jsx("path", { d: "M9 4v16" })
] });
const IconBack = () => /* @__PURE__ */ jsx("svg", { ...s, children: /* @__PURE__ */ jsx("path", { d: "m15 18-6-6 6-6" }) });
const IconMore = () => /* @__PURE__ */ jsxs("svg", { ...s, children: [
  /* @__PURE__ */ jsx("circle", { cx: "5", cy: "12", r: "1" }),
  /* @__PURE__ */ jsx("circle", { cx: "12", cy: "12", r: "1" }),
  /* @__PURE__ */ jsx("circle", { cx: "19", cy: "12", r: "1" })
] });
const IconTerminal = () => /* @__PURE__ */ jsxs("svg", { ...s, children: [
  /* @__PURE__ */ jsx("path", { d: "m4 17 6-5-6-5" }),
  /* @__PURE__ */ jsx("path", { d: "M12 19h8" })
] });
const IconChat = () => /* @__PURE__ */ jsx("svg", { ...s, children: /* @__PURE__ */ jsx("path", { d: "M21 12a8 8 0 0 1-11.6 7.1L4 20l1-4.6A8 8 0 1 1 21 12Z" }) });
const IconPlus = () => /* @__PURE__ */ jsx("svg", { ...s, children: /* @__PURE__ */ jsx("path", { d: "M12 5v14M5 12h14" }) });
const IconUp = () => /* @__PURE__ */ jsx("svg", { ...s, children: /* @__PURE__ */ jsx("path", { d: "M12 19V5M5 12l7-7 7 7" }) });
const IconFile = () => /* @__PURE__ */ jsxs("svg", { ...s, children: [
  /* @__PURE__ */ jsx("path", { d: "M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8Z" }),
  /* @__PURE__ */ jsx("path", { d: "M14 3v5h5" })
] });
function slashMatches(text, commands) {
  if (!/^\/[^\s]*$/.test(text)) return [];
  const t = text.toLowerCase();
  return commands.filter((c) => c.name.toLowerCase().startsWith(t) && c.name.toLowerCase() !== t).slice(0, 60);
}
const none = [];
function Composer({ draftKey, storage = defaultStorage, commands = none, terminal = false, attachments = none, uploading = false, onRemoveAttachment, onFiles, onSend, onKey }) {
  const [text, setText] = useState(() => storage.get("draft:" + draftKey) ?? "");
  const [sel, setSel] = useState(0);
  const [dismissed, setDismissed] = useState(false);
  const ta = useRef(null);
  const file = useRef(null);
  const id = useId();
  const popId = "slash-" + id;
  useEffect(() => {
    storage.set("draft:" + draftKey, text || null);
  }, [text, draftKey, storage]);
  useEffect(() => {
    const t = ta.current;
    if (t) {
      t.style.height = "auto";
      t.style.height = Math.min(t.scrollHeight, 220) + "px";
    }
  }, [text]);
  const options = useMemo(() => dismissed ? [] : slashMatches(text, commands), [text, commands, dismissed]);
  const open = options.length > 0;
  const pick = (c) => {
    var _a;
    setText(c.name + " ");
    setSel(0);
    (_a = ta.current) == null ? void 0 : _a.focus();
  };
  const coarse = typeof matchMedia === "function" && matchMedia("(pointer: coarse)").matches;
  const send = async () => {
    const t = text.trimEnd();
    if (!t && attachments.length === 0) return;
    setText("");
    setDismissed(false);
    await onSend(t);
  };
  const keydown = (e) => {
    if (open) {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setSel((s2) => (s2 + 1) % options.length);
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        setSel((s2) => (s2 - 1 + options.length) % options.length);
        return;
      }
      if (e.key === "Tab" || e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        pick(options[Math.min(sel, options.length - 1)]);
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setDismissed(true);
        return;
      }
    }
    if (e.key === "Enter" && !e.shiftKey && !coarse && !e.nativeEvent.isComposing) {
      e.preventDefault();
      void send();
    }
  };
  const paste = (e) => {
    const imgs = [...e.clipboardData.files].filter((f) => f.type.startsWith("image/"));
    if (imgs.length && onFiles) {
      e.preventDefault();
      onFiles(imgs);
    }
  };
  return /* @__PURE__ */ jsxs("div", { className: "composer-wrap", children: [
    /* @__PURE__ */ jsx("div", { className: "slash-pop", role: "listbox", id: popId, "aria-label": "Commands", hidden: !open, children: options.map((c, i) => /* @__PURE__ */ jsxs("div", { id: `${popId}-${i}`, role: "option", "aria-selected": i === sel, onMouseDown: (e) => {
      e.preventDefault();
      pick(c);
    }, children: [
      /* @__PURE__ */ jsx("span", { className: "slash-name", children: c.name }),
      /* @__PURE__ */ jsx("span", { className: "slash-desc", children: c.description })
    ] }, c.name)) }),
    /* @__PURE__ */ jsxs("div", { className: "composer", children: [
      attachments.length > 0 && /* @__PURE__ */ jsx("div", { className: "attachments", children: attachments.map((a) => /* @__PURE__ */ jsxs("span", { className: "attachment", title: a.path, children: [
        a.preview && /* @__PURE__ */ jsx("img", { className: "thumb", src: a.preview, alt: "" }),
        a.name,
        /* @__PURE__ */ jsx("button", { type: "button", "aria-label": "Remove " + a.name, onClick: () => onRemoveAttachment == null ? void 0 : onRemoveAttachment(a), children: "×" })
      ] }, a.path)) }),
      /* @__PURE__ */ jsx(
        "textarea",
        {
          ref: ta,
          rows: 1,
          value: text,
          "aria-label": "Message",
          "aria-autocomplete": "list",
          "aria-controls": popId,
          "aria-expanded": open,
          "aria-activedescendant": open ? `${popId}-${Math.min(sel, options.length - 1)}` : void 0,
          placeholder: uploading ? "Uploading image…" : attachments.length ? "Add a message, or send the image" : terminal ? "Type into the terminal" : "Type a message",
          onChange: (e) => {
            setText(e.target.value);
            setSel(0);
            setDismissed(false);
          },
          onKeyDown: keydown,
          onPaste: paste
        }
      ),
      /* @__PURE__ */ jsxs("div", { className: "composer-row", children: [
        onFiles && /* @__PURE__ */ jsxs(Fragment, { children: [
          /* @__PURE__ */ jsx(
            "input",
            {
              ref: file,
              className: "hidden-file",
              type: "file",
              accept: "image/*",
              multiple: true,
              tabIndex: -1,
              "aria-label": "Attach an image",
              onChange: (e) => {
                var _a;
                if ((_a = e.target.files) == null ? void 0 : _a.length) onFiles == null ? void 0 : onFiles([...e.target.files]);
                e.target.value = "";
              }
            }
          ),
          /* @__PURE__ */ jsx("button", { type: "button", className: "icon attach", title: "Attach an image (paste and drop work too)", "aria-label": "Attach an image", onClick: () => {
            var _a;
            return (_a = file.current) == null ? void 0 : _a.click();
          }, children: /* @__PURE__ */ jsx(IconPlus, {}) })
        ] }),
        terminal && /* @__PURE__ */ jsxs(Fragment, { children: [
          /* @__PURE__ */ jsx("button", { type: "button", className: "key", title: "Esc", onClick: () => onKey == null ? void 0 : onKey("esc"), children: "esc" }),
          /* @__PURE__ */ jsx("button", { type: "button", className: "key", title: "Ctrl-C", onClick: () => onKey == null ? void 0 : onKey("ctrl+c"), children: "^C" })
        ] }),
        /* @__PURE__ */ jsx("span", { className: "grow" }),
        /* @__PURE__ */ jsx("button", { type: "button", className: "send", "aria-label": "Send", onClick: () => void send(), children: /* @__PURE__ */ jsx(IconUp, {}) })
      ] })
    ] })
  ] });
}
function PaneHeader({ title, agent = "", session, status = "", statusText = "", cwd, leading, view = "chat", onToggleView, actions }) {
  return /* @__PURE__ */ jsxs("header", { className: "pane-bar", children: [
    leading,
    /* @__PURE__ */ jsxs("div", { className: "who", title: cwd || void 0, children: [
      /* @__PURE__ */ jsx("span", { className: "badge-a", "data-agent": agentKey(agent), "aria-hidden": "true", children: agentIcon(agent) }),
      /* @__PURE__ */ jsx("h1", { className: "name", children: title }),
      session && /* @__PURE__ */ jsx("span", { className: "pill", children: session }),
      /* @__PURE__ */ jsx("span", { className: "state state-" + (status || "none"), role: "status", children: statusText })
    ] }),
    onToggleView && /* @__PURE__ */ jsx("button", { type: "button", className: "icon tgl" + (view === "terminal" ? " on" : ""), "aria-pressed": view === "terminal", "aria-label": "Terminal", title: view === "terminal" ? "Back to chat" : "Terminal", onClick: onToggleView, children: view === "terminal" ? /* @__PURE__ */ jsx(IconChat, {}) : /* @__PURE__ */ jsx(IconTerminal, {}) }),
    actions
  ] });
}
const plainClick = (e) => e.button === 0 && !e.metaKey && !e.ctrlKey && !e.shiftKey && !e.altKey;
function Sidebar({ items, loaded = true, current, hrefFor, onOpen, filter: filterProp, onFilterChange, store = defaultStorage, renderGroupExtra, renderSubtitle, header, footer, empty }) {
  const [own, setOwn] = useState("");
  const filter = filterProp ?? own;
  const setFilter = (f) => {
    if (filterProp === void 0) setOwn(f);
    onFilterChange == null ? void 0 : onFilterChange(f);
  };
  const [showAll, setShowAll] = useState(() => store.get("showall") === "1");
  const [folded, setFolded] = useState(() => new Set((store.get("folded") ?? "").split("\n").filter(Boolean)));
  const search = useRef(null);
  useEffect(() => {
    const k = (e) => {
      var _a;
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        (_a = search.current) == null ? void 0 : _a.focus();
      }
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, []);
  const rows = useMemo(() => items.filter((i) => i.machine.online).flatMap(({ machine, tree }) => ((tree == null ? void 0 : tree.panes) ?? []).map((pane) => ({ machineId: machine.id, machineName: machine.name, pane, session: workspaceLabel(tree, pane.workspace_id) }))), [items]);
  const groups = useMemo(() => groupRows(rows, filter, showAll), [rows, filter, showAll]);
  const offline = items.filter((i) => !i.machine.online);
  const fold = (k) => {
    const n = new Set(folded);
    if (!n.delete(k)) n.add(k);
    setFolded(n);
    store.set("folded", [...n].join("\n"));
  };
  const click = (m, p) => (e) => {
    if (!onOpen || !plainClick(e)) return;
    e.preventDefault();
    onOpen(m, p, e);
  };
  return /* @__PURE__ */ jsxs("aside", { className: "side", "aria-label": "Agents", children: [
    header && /* @__PURE__ */ jsx("div", { className: "side-head", children: header }),
    /* @__PURE__ */ jsxs("div", { className: "side-search", children: [
      /* @__PURE__ */ jsx("input", { ref: search, type: "search", placeholder: "Search agents", "aria-label": "Search agents", value: filter, onChange: (e) => setFilter(e.target.value) }),
      /* @__PURE__ */ jsx("kbd", { children: "⌘K" })
    ] }),
    /* @__PURE__ */ jsxs("nav", { className: "side-list", "aria-label": "Panes", children: [
      loaded && items.length === 0 && (empty ?? /* @__PURE__ */ jsx("p", { className: "empty pad", children: "No machines are linked yet." })),
      groups.map((g) => {
        const open = !folded.has(g.key);
        return /* @__PURE__ */ jsxs("section", { className: "proj", children: [
          /* @__PURE__ */ jsxs("button", { type: "button", className: "proj-head", title: g.path, "aria-expanded": open, onClick: () => fold(g.key), children: [
            /* @__PURE__ */ jsx("span", { className: "chev", children: open ? "▾" : "▸" }),
            /* @__PURE__ */ jsx("span", { className: "proj-name", children: g.name }),
            /* @__PURE__ */ jsx("span", { className: "proj-n", children: g.rows.length }),
            renderGroupExtra == null ? void 0 : renderGroupExtra(g)
          ] }),
          open && g.rows.map((r) => {
            const st = paneStatus(r.pane);
            const agent = paneAgent(r.pane);
            const cur = (current == null ? void 0 : current.machine) === r.machineId && (current == null ? void 0 : current.pane) === r.pane.pane_id;
            return /* @__PURE__ */ jsxs(
              "a",
              {
                href: hrefFor == null ? void 0 : hrefFor(r.machineId, r.pane.pane_id),
                className: "ent" + (cur ? " cur" : ""),
                title: paneCwd(r.pane),
                onClick: click(r.machineId, r.pane.pane_id),
                "aria-current": cur ? "page" : void 0,
                children: [
                  /* @__PURE__ */ jsx("span", { className: "dot dot-" + (st || "none"), "aria-hidden": "true" }),
                  /* @__PURE__ */ jsx("span", { className: "badge-a", "data-agent": agentKey(agent), "aria-hidden": "true", children: agentIcon(agent) }),
                  /* @__PURE__ */ jsx("span", { className: "ent-title", children: displayName(r.pane) }),
                  /* @__PURE__ */ jsx("span", { className: "ent-time", children: st === "blocked" ? "!" : "" }),
                  /* @__PURE__ */ jsxs("span", { className: "ent-sub", children: [
                    st === "blocked" && /* @__PURE__ */ jsx("b", { className: "needs", children: "needs you · " }),
                    renderSubtitle ? renderSubtitle(r) : /* @__PURE__ */ jsxs(Fragment, { children: [
                      agent && /* @__PURE__ */ jsxs(Fragment, { children: [
                        /* @__PURE__ */ jsx("b", { className: "kind", children: agent.toLowerCase() }),
                        " · "
                      ] }),
                      r.machineName,
                      " · ",
                      r.session
                    ] })
                  ] })
                ]
              },
              r.machineId + "/" + r.pane.pane_id
            );
          })
        ] }, g.key);
      }),
      filter && groups.length === 0 && /* @__PURE__ */ jsx("p", { className: "empty pad", children: "No match." }),
      offline.length > 0 && !filter && /* @__PURE__ */ jsxs("section", { className: "proj", children: [
        /* @__PURE__ */ jsx("div", { className: "proj-head static", children: /* @__PURE__ */ jsx("span", { className: "proj-name", children: "Offline machines" }) }),
        offline.map(({ machine }) => /* @__PURE__ */ jsxs("a", { href: hrefFor == null ? void 0 : hrefFor(machine.id), className: "ent off", onClick: click(machine.id), children: [
          /* @__PURE__ */ jsx("span", { className: "dot dot-none" }),
          /* @__PURE__ */ jsx("span", { className: "ent-title", children: machine.name || machine.id }),
          /* @__PURE__ */ jsx("span", {})
        ] }, machine.id))
      ] })
    ] }),
    /* @__PURE__ */ jsxs("div", { className: "side-foot", children: [
      /* @__PURE__ */ jsxs("label", { className: "toggle", children: [
        /* @__PURE__ */ jsx("input", { type: "checkbox", checked: showAll, onChange: (e) => {
          setShowAll(e.target.checked);
          store.set("showall", e.target.checked ? "1" : null);
        } }),
        " Show all panes"
      ] }),
      footer
    ] })
  ] });
}
export {
  ChatRow,
  ChatView,
  ChoiceBar,
  Composer,
  FileHrefCtx,
  IconBack,
  IconChat,
  IconFile,
  IconMore,
  IconPlus,
  IconSidebar,
  IconTerminal,
  IconUp,
  OpenCtx,
  PaneHeader,
  Sidebar,
  Terminal,
  TransportError,
  VirtualList,
  agentIcon,
  agentKey,
  atBottom,
  backoff,
  buildRows,
  choiceKeys,
  computeWindow,
  connectSSE,
  decodeBase64,
  defaultStorage,
  displayName,
  folderName,
  groupRows,
  httpTransport,
  isAgent,
  matches,
  memoryStorage,
  mergeItems,
  offsets,
  pairCalls,
  paneAgent,
  paneCwd,
  paneId,
  paneStatus,
  phrase,
  slashMatches,
  statusRank,
  stripTags,
  toolIcon,
  useChat,
  verb,
  webStorage,
  workspaceLabel
};
//# sourceMappingURL=index.js.map
