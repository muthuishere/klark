//! Simulate many klark machines against a hub.
//!
//!   klark-loadtest --hub ws://localhost:8080/agent --token <fleet token> \
//!       [--machines 100] [--panes 8] [--rate 2000] [--seconds 60] [--tick-ms 25]
//!
//! Each simulated machine speaks the real link protocol (crates/klark-link):
//! it answers `get_tree` (unchanged most of the time, like a real machine),
//! answers `pane.read`, and streams `--rate` bytes/s of fake terminal output
//! for every pane the hub subscribes to — and nothing else. Configure the hub
//! with one machine entry `{ "Name": "load", "Fleet": true, ... }` so a single
//! token admits them all as load:sim-001, load:sim-002, …

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine as _;
use klark_link::protocol::{AgentMessage, HubMessage, PROTOCOL_VERSION};
use serde_json::{json, Value};

#[derive(Default)]
struct Stats {
    connected: AtomicU64,
    failed: AtomicU64,
    msgs_in: AtomicU64,
    msgs_out: AtomicU64,
    bytes_out: AtomicU64,
    tree_pulls: AtomicU64,
    subscribed: AtomicU64,
}

struct Args {
    hub: String,
    token: String,
    machines: usize,
    panes: usize,
    rate: usize,
    seconds: u64,
    tick_ms: u64,
}

fn parse_args() -> Args {
    let mut a = Args {
        hub: String::new(),
        token: String::new(),
        machines: 100,
        panes: 8,
        rate: 2000,
        seconds: 60,
        tick_ms: 25,
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let v = argv.get(i + 1).cloned().unwrap_or_default();
        match argv[i].as_str() {
            "--hub" => a.hub = v,
            "--token" => a.token = v,
            "--machines" => a.machines = v.parse().expect("--machines"),
            "--panes" => a.panes = v.parse().expect("--panes"),
            "--rate" => a.rate = v.parse().expect("--rate"),
            "--seconds" => a.seconds = v.parse().expect("--seconds"),
            "--tick-ms" => a.tick_ms = v.parse().expect("--tick-ms"),
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
        i += 2;
    }
    if a.hub.is_empty() || a.token.is_empty() {
        eprintln!("usage: klark-loadtest --hub <ws url> --token <fleet token> [--machines N] [--panes N] [--rate B/s] [--seconds S]");
        std::process::exit(2);
    }
    a
}

fn main() {
    let args = Arc::new(parse_args());
    let stats = Arc::new(Stats::default());
    let deadline = Instant::now() + Duration::from_secs(args.seconds);
    let mut handles = Vec::new();
    for n in 0..args.machines {
        let (args, stats) = (args.clone(), stats.clone());
        handles.push(thread::spawn(move || {
            // Stagger connects so the hub sees a ramp, not a thundering herd.
            thread::sleep(Duration::from_millis((n as u64 * 20) % 2000));
            if let Err(e) = simulate(n, &args, &stats, deadline) {
                stats.failed.fetch_add(1, Ordering::Relaxed);
                eprintln!("sim-{n:03}: {e}");
            }
        }));
    }
    let started = Instant::now();
    while Instant::now() < deadline {
        thread::sleep(
            Duration::from_secs(5).min(deadline.saturating_duration_since(Instant::now())),
        );
        report(&stats, started.elapsed());
    }
    for h in handles {
        let _ = h.join();
    }
    println!("final:");
    report(&stats, started.elapsed());
}

fn report(s: &Stats, elapsed: Duration) {
    let secs = elapsed.as_secs_f64().max(0.001);
    let out = s.bytes_out.load(Ordering::Relaxed);
    println!(
        "{:>5.0}s  connected {:>4}  failed {:>3}  subscribed panes {:>4}  tree pulls {:>6}  in {:>7} msgs  out {:>7} msgs  {:>8.1} KB/s out",
        secs,
        s.connected.load(Ordering::Relaxed),
        s.failed.load(Ordering::Relaxed),
        s.subscribed.load(Ordering::Relaxed),
        s.tree_pulls.load(Ordering::Relaxed),
        s.msgs_in.load(Ordering::Relaxed),
        s.msgs_out.load(Ordering::Relaxed),
        out as f64 / 1024.0 / secs,
    );
}

fn fake_tree(machine: usize, panes: usize, epoch: u64) -> Value {
    let statuses = ["working", "idle", "blocked", ""];
    let panes: Vec<Value> = (0..panes)
        .map(|p| {
            json!({
                "pane_id": format!("p{p}"), "workspace_id": "w1", "tab_id": "t1",
                "title": format!("sim-{machine:03} pane {p}"),
                "agent": if p % 2 == 0 { "claude" } else { "" },
                "agent_status": statuses[(p + epoch as usize) % statuses.len()],
                "cwd": format!("/work/project-{p}"),
            })
        })
        .collect();
    json!({"workspaces": [{"workspace_id": "w1", "label": "main", "number": 1}], "tabs": [], "panes": panes})
}

fn simulate(n: usize, args: &Args, stats: &Stats, deadline: Instant) -> Result<(), String> {
    let machine_id = format!("sim-{n:03}");
    let mut ws = klark_link::connect_hub(&args.hub, &args.token, &machine_id)?;
    stats.connected.fetch_add(1, Ordering::Relaxed);
    let send = |ws: &mut klark_link::Socket, msg: &AgentMessage| -> Result<(), String> {
        let bytes = klark_link::encode(msg)?;
        stats.msgs_out.fetch_add(1, Ordering::Relaxed);
        stats
            .bytes_out
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        klark_link::send(ws, msg)
    };
    send(
        &mut ws,
        &AgentMessage::Hello {
            protocol: PROTOCOL_VERSION,
            machine_id: machine_id.clone(),
            machine_name: machine_id.clone(),
            klark_version: "loadtest".into(),
        },
    )?;

    let started = Instant::now();
    let mut subscribed: Vec<String> = Vec::new();
    let mut last_emit = Instant::now();
    let mut line = 0u64;
    let b64 = base64::engine::general_purpose::STANDARD;
    while Instant::now() < deadline {
        match ws.read() {
            Ok(msg) => {
                if let Some(Ok(msg)) = klark_link::decode_hub(msg) {
                    stats.msgs_in.fetch_add(1, Ordering::Relaxed);
                    // The tree changes every ~30 s, like agents changing state.
                    let epoch = started.elapsed().as_secs() / 30;
                    let reply = match msg {
                        HubMessage::GetTree { known_rev } => {
                            stats.tree_pulls.fetch_add(1, Ordering::Relaxed);
                            let snapshot = fake_tree(n, args.panes, epoch);
                            let rev = klark_link::tree_rev(&snapshot);
                            Some(if known_rev.as_deref() == Some(rev.as_str()) {
                                AgentMessage::TreeUnchanged { rev }
                            } else {
                                AgentMessage::Tree { rev, snapshot }
                            })
                        }
                        HubMessage::Call { id, .. } => Some(AgentMessage::Result {
                            id,
                            result: Some(
                                json!({"read": {"text": format!("{machine_id} screen\r\n$ ")}}),
                            ),
                            error: None,
                        }),
                        HubMessage::Subscribe { pane_id } => {
                            if !subscribed.contains(&pane_id) {
                                subscribed.push(pane_id);
                                stats.subscribed.fetch_add(1, Ordering::Relaxed);
                            }
                            None
                        }
                        HubMessage::Unsubscribe { pane_id } => {
                            if let Some(i) = subscribed.iter().position(|p| *p == pane_id) {
                                subscribed.remove(i);
                                stats.subscribed.fetch_sub(1, Ordering::Relaxed);
                            }
                            None
                        }
                        HubMessage::Ping => Some(AgentMessage::Pong),
                        HubMessage::Input { .. } | HubMessage::Keys { .. } => None,
                    };
                    if let Some(reply) = reply {
                        send(&mut ws, &reply)?;
                    }
                }
            }
            Err(tungstenite::Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => {
                stats
                    .subscribed
                    .fetch_sub(subscribed.len() as u64, Ordering::Relaxed);
                return Err(format!("read: {e}"));
            }
        }
        // Stream output only for subscribed panes, batched every --tick-ms.
        if last_emit.elapsed() >= Duration::from_millis(args.tick_ms) {
            let budget = args.rate * last_emit.elapsed().as_millis() as usize / 1000;
            last_emit = Instant::now();
            for pane_id in subscribed.clone() {
                let mut out = String::new();
                while out.len() < budget {
                    line += 1;
                    out.push_str(&format!("\x1b[32m{machine_id}\x1b[0m {pane_id} line {line}: compiling crate {}…\r\n", line % 97));
                }
                send(
                    &mut ws,
                    &AgentMessage::Frame {
                        pane_id,
                        bytes: b64.encode(out),
                    },
                )?;
            }
        }
    }
    stats
        .subscribed
        .fetch_sub(subscribed.len() as u64, Ordering::Relaxed);
    let _ = ws.close(None);
    Ok(())
}
