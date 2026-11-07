//! Simulate many klark machines against a hub.
//!
//!   klark-loadtest --hub ws://localhost:8080/agent --token <fleet token> \
//!       [--machines 100] [--panes 12] [--rate 20000] [--seconds 60] [--tick-ms 50] \
//!       [--csv stats.csv] [--interval-ms 2000] [--reconnect] [--prefix sim]
//!
//! Each simulated machine speaks the real link protocol v3 (crates/klark-link)
//! and behaves like a real klark machine as far as the hub can tell:
//! - a realistic tree: 3 workspaces x 2 tabs, `--panes` panes, two thirds of
//!   them running an agent (claude / codex / devin) whose status changes now
//!   and then, so the hub sees a full tree only when something changed;
//! - `get_transcript` with a growing conversation (markdown, code blocks, tool
//!   calls), answered incrementally from the `after` cursor;
//! - `list_agents`, `deliver`, `pane.read` (a ~2 KB coloured screen),
//!   `fetch_asset`, `ping`;
//! - for every subscribed pane only: `pane_size` on subscribe, then `--rate`
//!   bytes/s of mixed build-log / diff / markdown output in binary frames,
//!   merged per `--tick-ms`, with a 1 s burst at 4x every 10 s;
//! - `input` on a subscribed pane is echoed back at once as output, so a
//!   browser can time a keystroke round trip through the hub.
//!
//! Nothing is sent unasked except `hello`. Every byte in each direction is
//! counted per message kind; `--csv` writes the cumulative counters every
//! `--interval-ms`. With `--reconnect`, a dropped machine reconnects with the
//! real link's backoff (1 s doubling to 30 s, +-50 % jitter), as in E-10.
//!
//! Configure the hub with one machine entry `{ "Name": "load", "Fleet": true, ... }`
//! so a single token admits them all as load:sim-000, load:sim-001, ...

use std::collections::HashMap;
use std::io::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine as _;
use klark_link::protocol::{AgentMessage, HubMessage, PROTOCOL_VERSION};
use serde_json::{json, Value};
use tungstenite::Message;

macro_rules! counters {
    ($($name:ident),* $(,)?) => {
        #[derive(Default)]
        struct Stats { $($name: AtomicU64,)* }
        impl Stats {
            fn header() -> String { ["t_s", $(stringify!($name)),*].join(",") }
            fn row(&self, t: f64) -> String {
                let mut s = format!("{t:.1}");
                $( s.push(','); s.push_str(&self.$name.load(Ordering::Relaxed).to_string()); )*
                s
            }
        }
    };
}

counters!(
    connected,  // gauge: machines linked right now
    connects,   // total successful connects (reconnects included)
    failed,     // total connect or link failures
    subscribed, // gauge: panes streaming right now
    msgs_in,
    bytes_in, // every hub->machine byte (payload, after websocket framing)
    msgs_out,
    bytes_out, // every machine->hub byte
    get_tree,
    tree_full,
    tree_full_bytes,
    tree_unchanged,
    transcript_calls,
    transcript_items,
    transcript_bytes,
    list_agents,
    calls,
    screen_bytes,
    pings,
    inputs,
    keys,
    subscribes,
    unsubscribes,
    frames,
    frame_raw_bytes,
    frame_wire_bytes,
    other_bytes, // hello, pane_size, deliver/asset results
);

struct Args {
    hub: String,
    token: String,
    machines: usize,
    panes: usize,
    rate: usize,
    seconds: u64,
    tick_ms: u64,
    csv: Option<String>,
    interval_ms: u64,
    reconnect: bool,
    prefix: String,
}

fn parse_args() -> Args {
    let mut a = Args {
        hub: String::new(),
        token: String::new(),
        machines: 100,
        panes: 12,
        rate: 20000,
        seconds: 60,
        tick_ms: 50,
        csv: None,
        interval_ms: 2000,
        reconnect: false,
        prefix: "sim".into(),
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let v = argv.get(i + 1).cloned().unwrap_or_default();
        let mut step = 2;
        match argv[i].as_str() {
            "--hub" => a.hub = v,
            "--token" => a.token = v,
            "--machines" => a.machines = v.parse().expect("--machines"),
            "--panes" => a.panes = v.parse().expect("--panes"),
            "--rate" => a.rate = v.parse().expect("--rate"),
            "--seconds" => a.seconds = v.parse().expect("--seconds"),
            "--tick-ms" => a.tick_ms = v.parse().expect("--tick-ms"),
            "--csv" => a.csv = Some(v),
            "--interval-ms" => a.interval_ms = v.parse().expect("--interval-ms"),
            "--prefix" => a.prefix = v,
            "--reconnect" => {
                a.reconnect = true;
                step = 1;
            }
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
        i += step;
    }
    if a.hub.is_empty() || a.token.is_empty() {
        eprintln!("usage: klark-loadtest --hub <ws url> --token <fleet token> [--machines N] [--panes N] [--rate B/s] [--seconds S] [--csv FILE] [--reconnect]");
        std::process::exit(2);
    }
    a
}

fn main() {
    let args = Arc::new(parse_args());
    let stats = Arc::new(Stats::default());
    let deadline = Instant::now() + Duration::from_secs(args.seconds);
    let started = Instant::now();
    let mut handles = Vec::new();
    for n in 0..args.machines {
        let (args, stats) = (args.clone(), stats.clone());
        handles.push(
            thread::Builder::new()
                .stack_size(256 * 1024)
                .spawn(move || machine_loop(n, &args, &stats, started, deadline))
                .expect("spawn"),
        );
    }
    let mut csv = args.csv.as_ref().map(|p| {
        let mut f = std::fs::File::create(p).expect("--csv");
        writeln!(f, "{}", Stats::header()).ok();
        f
    });
    let mut last_print = Instant::now();
    while Instant::now() < deadline {
        thread::sleep(
            Duration::from_millis(args.interval_ms)
                .min(deadline.saturating_duration_since(Instant::now())),
        );
        let t = started.elapsed().as_secs_f64();
        if let Some(f) = csv.as_mut() {
            writeln!(f, "{}", stats.row(t)).ok();
            f.flush().ok();
        }
        if last_print.elapsed() >= Duration::from_secs(10) {
            last_print = Instant::now();
            report(&stats, started.elapsed());
        }
    }
    for h in handles {
        let _ = h.join();
    }
    println!("final:");
    report(&stats, started.elapsed());
}

fn report(s: &Stats, elapsed: Duration) {
    let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
    let secs = elapsed.as_secs_f64().max(0.001);
    println!(
        "{:>5.0}s  connected {:>4} (connects {}, failed {})  subscribed {:>3}  get_tree {} (full {})  transcripts {}  list_agents {}  in {} msgs/{} KB  out {} msgs/{} KB ({:.1} KB/s)",
        secs,
        g(&s.connected),
        g(&s.connects),
        g(&s.failed),
        g(&s.subscribed),
        g(&s.get_tree),
        g(&s.tree_full),
        g(&s.transcript_calls),
        g(&s.list_agents),
        g(&s.msgs_in),
        g(&s.bytes_in) / 1024,
        g(&s.msgs_out),
        g(&s.bytes_out) / 1024,
        g(&s.bytes_out) as f64 / 1024.0 / secs,
    );
}

/// The real link's reconnect policy (crates/klark-link/src/lib.rs run()).
fn machine_loop(n: usize, args: &Args, stats: &Stats, started: Instant, deadline: Instant) {
    // Stagger the first connects so the hub sees a ramp, not a thundering herd.
    thread::sleep(Duration::from_millis((n as u64 * 20) % 2000));
    let machine_id = format!("{}-{n:03}", args.prefix);
    let mut backoff = Duration::from_secs(1);
    let mut state = MachineState::new(n, args.panes, started);
    loop {
        let t0 = Instant::now();
        if let Err(e) = simulate(&machine_id, &mut state, args, stats, deadline) {
            stats.failed.fetch_add(1, Ordering::Relaxed);
            if !args.reconnect {
                eprintln!("{machine_id}: {e}");
            }
        }
        if !args.reconnect || Instant::now() >= deadline {
            return;
        }
        if t0.elapsed() > Duration::from_secs(60) {
            backoff = Duration::from_secs(1);
        }
        thread::sleep(klark_link::jittered(backoff, &machine_id));
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

const LABELS: [&str; 3] = ["api", "web", "infra"];
const AGENTS: [&str; 3] = ["claude", "codex", "devin"];
const STATUSES: [&str; 3] = ["working", "idle", "blocked"];

struct MachineState {
    n: usize,
    panes: usize,
    started: Instant,
    line: u64,
}

impl MachineState {
    fn new(n: usize, panes: usize, started: Instant) -> Self {
        Self {
            n,
            panes,
            started,
            line: 0,
        }
    }

    /// Statuses move every ~20 s, offset per machine, like agents finishing turns.
    fn epoch(&self) -> u64 {
        (self.started.elapsed().as_millis() as u64 + self.n as u64 * 197) / 20_000
    }

    fn agent_of(&self, p: usize) -> Option<&'static str> {
        (p % 3 != 2).then(|| AGENTS[(p + self.n) % AGENTS.len()])
    }

    fn status_of(&self, p: usize) -> &'static str {
        // Only one pane per machine changes per epoch.
        let e = self.epoch() as usize;
        let changed = if p == e % self.panes.max(1) { 1 } else { 0 };
        STATUSES[(p + changed) % STATUSES.len()]
    }

    fn tree(&self) -> Value {
        let mut workspaces = Vec::new();
        let mut tabs = Vec::new();
        for w in 0..3 {
            workspaces.push(json!({"workspace_id": format!("w{w}"), "label": LABELS[w], "number": w + 1, "focused": w == 0}));
            for t in 0..2 {
                tabs.push(json!({"tab_id": format!("t{w}{t}"), "workspace_id": format!("w{w}"), "label": format!("tab {}", t + 1), "number": t + 1}));
            }
        }
        let panes: Vec<Value> = (0..self.panes)
            .map(|p| {
                let (w, t) = (p % 3, (p / 3) % 2);
                let agent = self.agent_of(p);
                json!({
                    "pane_id": format!("p{p}"), "workspace_id": format!("w{w}"), "tab_id": format!("t{w}{t}"),
                    "title": match agent { Some(a) => format!("{a} — project-{p}"), None => format!("zsh — project-{p}") },
                    "agent": agent.unwrap_or(""),
                    "agent_status": if agent.is_some() { self.status_of(p) } else { "" },
                    "cwd": format!("/Users/dev/work/sim-{:03}/project-{p}", self.n),
                    "cols": 132, "rows": 40, "focused": p == 0,
                })
            })
            .collect();
        json!({"workspaces": workspaces, "tabs": tabs, "panes": panes})
    }

    fn agents(&self) -> Value {
        Value::Array(
            (0..self.panes)
                .filter_map(|p| {
                    self.agent_of(p).map(|a| {
                        json!({"session": "default", "name": format!("{a}-{p}"), "agent": a,
                               "pane_id": format!("p{p}"), "status": self.status_of(p)})
                    })
                })
                .collect(),
        )
    }

    /// Item k of pane p's conversation; 30 items exist at start, then one every 6 s.
    fn transcript_len(&self, p: usize) -> u64 {
        30 + (self.started.elapsed().as_secs() + p as u64) / 6
    }

    fn item(&self, p: usize, k: u64) -> Value {
        let id = format!("p{p}-{k}");
        match k % 4 {
            0 => {
                json!({"id": id, "role": "user", "kind": "text", "text": format!("Please fix the failing test in module {k} and explain the change.")})
            }
            1 => json!({"id": id, "role": "assistant", "kind": "tool_use", "tool": "Bash",
                        "input": {"command": format!("cargo test -p crate-{} -- --nocapture", k % 7)}}),
            2 => json!({"id": id, "role": "assistant", "kind": "tool_result", "ok": k % 3 != 0,
                        "text": format!("running 12 tests\ntest a::b ... ok\ntest a::c ... FAILED\n\nfailures: assertion `left == right` failed at src/m{k}.rs:{}\n", k * 7 % 400)}),
            _ => json!({"id": id, "role": "assistant", "kind": "text", "text": format!(
                "The failure in **module {k}** comes from an off-by-one in the window check.\n\n\
                 - `next()` advanced past the end\n- the test expected {k} items\n\n\
                 ```rust\nfn window(xs: &[u32], n: usize) -> impl Iterator<Item = &[u32]> {{\n    xs.windows(n.min(xs.len()).max(1))\n}}\n```\n\n\
                 I changed `src/m{k}.rs` and the suite passes now. Next I will look at `crate-{}`.", k % 7)}),
        }
    }

    fn transcript(&self, pane_id: &str, after: Option<&str>) -> Value {
        let p: usize = pane_id.trim_start_matches('p').parse().unwrap_or(0);
        let total = self.transcript_len(p);
        let from = after
            .and_then(|a| a.parse::<u64>().ok())
            .map(|a| a + 1)
            .unwrap_or(total.saturating_sub(200));
        let items: Vec<Value> = (from..total).map(|k| self.item(p, k)).collect();
        json!({"agent": self.agent_of(p), "cursor": (total - 1).to_string(), "items": items,
               "source": if self.agent_of(p).is_some() { "jsonl" } else { "screen" }, "has_more": false})
    }

    fn screen(&self, pane_id: &str) -> String {
        let mut s = String::from("\x1b[H\x1b[2J");
        for r in 0..30 {
            s.push_str(&format!("\x1b[3{}m{pane_id}\x1b[0m {:>3} │ \x1b[1mCompiling\x1b[0m crate-{} v0.{}.0 (/work/crate-{})\r\n", r % 7 + 1, r, r % 9, r, r % 9));
        }
        s.push_str("\x1b[32m$\x1b[0m ");
        s
    }

    /// `budget` bytes of output that compress like real agent output: build
    /// logs, diffs and markdown with changing numbers and hashes.
    fn output(&mut self, machine_id: &str, pane_id: &str, budget: usize) -> String {
        let mut out = String::with_capacity(budget + 200);
        while out.len() < budget {
            self.line += 1;
            let l = self.line;
            let h = l.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 20;
            match l % 5 {
                0 => out.push_str(&format!("\x1b[32m   Compiling\x1b[0m {machine_id}-dep{} v{}.{}.{} ({h:x})\r\n", l % 97, l % 4, l % 13, h % 31)),
                1 => out.push_str(&format!("\x1b[31m-    let total = items.iter().map(|x| x.{}).sum::<u{}>();\x1b[0m\r\n", ["len", "size", "weight"][(l % 3) as usize], 32 << (l % 2))),
                2 => out.push_str(&format!("\x1b[32m+    let total: u64 = items.iter().map(|x| x.cost_{h:x} as u64).sum();\x1b[0m\r\n")),
                3 => out.push_str(&format!("{pane_id} │ test result: ok. {} passed; {} failed; finished in {}.{:02}s\r\n", h % 300, h % 3, h % 9, l % 100)),
                _ => out.push_str(&format!("⏺ Reading src/module_{}.rs ({} lines) — commit {h:012x}\r\n", l % 211, h % 900)),
            }
        }
        out
    }
}

fn simulate(
    machine_id: &str,
    st: &mut MachineState,
    args: &Args,
    stats: &Stats,
    deadline: Instant,
) -> Result<(), String> {
    let mut ws = klark_link::connect_hub(&args.hub, &args.token, machine_id)?;
    stats.connected.fetch_add(1, Ordering::Relaxed);
    stats.connects.fetch_add(1, Ordering::Relaxed);
    let mut subscribed: Vec<String> = Vec::new();
    let result = link(
        &mut ws,
        machine_id,
        st,
        args,
        stats,
        deadline,
        &mut subscribed,
    );
    stats.connected.fetch_sub(1, Ordering::Relaxed);
    stats
        .subscribed
        .fetch_sub(subscribed.len() as u64, Ordering::Relaxed);
    if result.is_ok() {
        let _ = ws.close(None);
    }
    result
}

#[derive(Clone, Copy)]
enum Kind {
    Tree,
    Unchanged,
    Transcript,
    Screen,
    Frame,
    Other,
}

fn link(
    ws: &mut klark_link::Socket,
    machine_id: &str,
    st: &mut MachineState,
    args: &Args,
    stats: &Stats,
    deadline: Instant,
    subscribed: &mut Vec<String>,
) -> Result<(), String> {
    let send = |ws: &mut klark_link::Socket,
                msg: &AgentMessage,
                kind: Kind|
     -> Result<(), String> {
        let bytes = klark_link::encode(msg)?;
        let len = bytes.len() as u64;
        stats.msgs_out.fetch_add(1, Ordering::Relaxed);
        stats.bytes_out.fetch_add(len, Ordering::Relaxed);
        match kind {
            Kind::Tree => stats.tree_full_bytes.fetch_add(len, Ordering::Relaxed),
            Kind::Transcript => stats.transcript_bytes.fetch_add(len, Ordering::Relaxed),
            Kind::Screen => stats.screen_bytes.fetch_add(len, Ordering::Relaxed),
            Kind::Frame => stats.frame_wire_bytes.fetch_add(len, Ordering::Relaxed),
            Kind::Unchanged | Kind::Other => stats.other_bytes.fetch_add(len, Ordering::Relaxed),
        };
        ws.send(Message::binary(bytes))
            .map_err(|e| format!("send: {e}"))
    };
    send(
        ws,
        &AgentMessage::Hello {
            protocol: PROTOCOL_VERSION,
            machine_id: machine_id.to_string(),
            machine_name: machine_id.to_string(),
            klark_version: "loadtest".into(),
        },
        Kind::Other,
    )?;

    let b64 = base64::engine::general_purpose::STANDARD;
    let mut last_emit = Instant::now();
    let mut echo: HashMap<String, String> = HashMap::new();
    let tick = Duration::from_millis(args.tick_ms);
    while Instant::now() < deadline {
        match ws.read() {
            Ok(Message::Close(_)) => return Err("hub closed".into()),
            Ok(msg) => {
                let len = match &msg {
                    Message::Text(t) => t.len(),
                    Message::Binary(b) => b.len(),
                    _ => 0,
                } as u64;
                if let Some(Ok(msg)) = klark_link::decode_hub(msg) {
                    stats.msgs_in.fetch_add(1, Ordering::Relaxed);
                    stats.bytes_in.fetch_add(len, Ordering::Relaxed);
                    let reply: Option<(AgentMessage, Kind)> = match msg {
                        HubMessage::GetTree { known_rev } => {
                            stats.get_tree.fetch_add(1, Ordering::Relaxed);
                            let snapshot = st.tree();
                            let rev = klark_link::tree_rev(&snapshot);
                            Some(if known_rev.as_deref() == Some(rev.as_str()) {
                                stats.tree_unchanged.fetch_add(1, Ordering::Relaxed);
                                (AgentMessage::TreeUnchanged { rev }, Kind::Unchanged)
                            } else {
                                stats.tree_full.fetch_add(1, Ordering::Relaxed);
                                (AgentMessage::Tree { rev, snapshot }, Kind::Tree)
                            })
                        }
                        HubMessage::Call { id, method, params } => {
                            stats.calls.fetch_add(1, Ordering::Relaxed);
                            let pane = params["pane_id"]
                                .as_str()
                                .or(params["target"].as_str())
                                .unwrap_or("p0")
                                .to_string();
                            let text = if method == "pane.read" && params["source"] == "visible" {
                                st.screen(&pane)
                            } else {
                                format!("{machine_id} {pane}: recent output\n")
                            };
                            Some((
                                AgentMessage::Result {
                                    id,
                                    result: Some(json!({"read": {"text": text}})),
                                    error: None,
                                },
                                Kind::Screen,
                            ))
                        }
                        HubMessage::Subscribe { pane_id } => {
                            stats.subscribes.fetch_add(1, Ordering::Relaxed);
                            if !subscribed.contains(&pane_id) {
                                subscribed.push(pane_id.clone());
                                stats.subscribed.fetch_add(1, Ordering::Relaxed);
                            }
                            Some((
                                AgentMessage::PaneSize {
                                    pane_id,
                                    cols: 132,
                                    rows: 40,
                                },
                                Kind::Other,
                            ))
                        }
                        HubMessage::Unsubscribe { pane_id } => {
                            stats.unsubscribes.fetch_add(1, Ordering::Relaxed);
                            if let Some(i) = subscribed.iter().position(|p| *p == pane_id) {
                                subscribed.remove(i);
                                stats.subscribed.fetch_sub(1, Ordering::Relaxed);
                            }
                            None
                        }
                        HubMessage::Ping => {
                            stats.pings.fetch_add(1, Ordering::Relaxed);
                            Some((AgentMessage::Pong, Kind::Other))
                        }
                        HubMessage::Input { pane_id, text } => {
                            stats.inputs.fetch_add(1, Ordering::Relaxed);
                            if subscribed.contains(&pane_id) {
                                echo.entry(pane_id).or_default().push_str(&text);
                            }
                            None
                        }
                        HubMessage::Keys { pane_id, keys } => {
                            stats.keys.fetch_add(1, Ordering::Relaxed);
                            if subscribed.contains(&pane_id) && keys.iter().any(|k| k == "enter") {
                                echo.entry(pane_id).or_default().push_str("\r\n");
                            }
                            None
                        }
                        HubMessage::ListAgents { id } => {
                            stats.list_agents.fetch_add(1, Ordering::Relaxed);
                            Some((
                                AgentMessage::Result {
                                    id,
                                    result: Some(st.agents()),
                                    error: None,
                                },
                                Kind::Other,
                            ))
                        }
                        HubMessage::Deliver { id, .. } => Some((
                            AgentMessage::Result {
                                id,
                                result: Some(json!({"delivered": true})),
                                error: None,
                            },
                            Kind::Other,
                        )),
                        HubMessage::FetchAsset { id, .. } => Some((
                            AgentMessage::Result {
                                id,
                                result: Some(json!({"path": "/tmp/loadtest.png"})),
                                error: None,
                            },
                            Kind::Other,
                        )),
                        HubMessage::GetFile { id, .. } => Some((
                            AgentMessage::Result {
                                id,
                                result: None,
                                error: Some("loadtest machines serve no files".into()),
                            },
                            Kind::Other,
                        )),
                        HubMessage::GetTranscript {
                            id, pane_id, after, ..
                        } => {
                            stats.transcript_calls.fetch_add(1, Ordering::Relaxed);
                            let result = st.transcript(&pane_id, after.as_deref());
                            stats.transcript_items.fetch_add(
                                result["items"].as_array().map_or(0, |a| a.len()) as u64,
                                Ordering::Relaxed,
                            );
                            Some((
                                AgentMessage::Result {
                                    id,
                                    result: Some(result),
                                    error: None,
                                },
                                Kind::Transcript,
                            ))
                        }
                    };
                    if let Some((reply, kind)) = reply {
                        send(ws, &reply, kind)?;
                    }
                }
            }
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => return Err(format!("read: {e}")),
        }
        // A keystroke echo goes out at once, like a real shell's.
        for (pane_id, text) in echo.drain() {
            stats.frames.fetch_add(1, Ordering::Relaxed);
            stats
                .frame_raw_bytes
                .fetch_add(text.len() as u64, Ordering::Relaxed);
            send(
                ws,
                &AgentMessage::Frame {
                    pane_id,
                    bytes: b64.encode(text),
                },
                Kind::Frame,
            )?;
        }
        // Stream output only for subscribed panes, merged per tick.
        if !subscribed.is_empty() && last_emit.elapsed() >= tick && args.rate > 0 {
            let elapsed = last_emit.elapsed();
            last_emit = Instant::now();
            // A 1 s burst at 4x every 10 s.
            let burst = if st.started.elapsed().as_secs() % 10 == 0 {
                4
            } else {
                1
            };
            let budget = args.rate * burst * elapsed.as_millis() as usize / 1000;
            for pane_id in subscribed.clone() {
                let out = st.output(machine_id, &pane_id, budget);
                stats.frames.fetch_add(1, Ordering::Relaxed);
                stats
                    .frame_raw_bytes
                    .fetch_add(out.len() as u64, Ordering::Relaxed);
                send(
                    ws,
                    &AgentMessage::Frame {
                        pane_id,
                        bytes: b64.encode(out),
                    },
                    Kind::Frame,
                )?;
            }
        } else if subscribed.is_empty() {
            last_emit = Instant::now();
        }
    }
    Ok(())
}
