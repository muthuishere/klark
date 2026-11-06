//! `klark expose` serves EVERY running session through one hub link, with the
//! hub unaware of sessions: the default session's ids stay plain, the `work`
//! session's are namespaced `work/<id>` (crates/klark-link/src/sessions.rs).
//!
//! Two headless servers (default + work) in one config dir, a real
//! `klark expose` child, and a tiny in-test WebSocket hub.

#![cfg(unix)]

pub mod support;

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use tungstenite::{Message, WebSocket};

const BIN: &str = env!("CARGO_BIN_EXE_klark");

struct Env {
    base: PathBuf,
    config: PathBuf,
    runtime: PathBuf,
    children: Vec<Child>,
}

impl Drop for Env {
    fn drop(&mut self) {
        for c in &mut self.children {
            let _ = c.kill();
            let _ = c.wait();
        }
        support::cleanup_test_base(&self.base);
    }
}

impl Env {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let base = PathBuf::from(format!(
            "/tmp/klark-expose-test-{}-{nanos}",
            std::process::id()
        ));
        let config = base.join("config");
        let runtime = base.join("runtime");
        for app in ["klark", "klark-dev"] {
            fs::create_dir_all(config.join(app)).unwrap();
            fs::write(config.join(app).join("config.toml"), "onboarding = false\n").unwrap();
        }
        fs::create_dir_all(&runtime).unwrap();
        support::register_runtime_dir(&runtime);
        Env {
            base,
            config,
            runtime,
            children: Vec::new(),
        }
    }

    fn cmd(&self, session: Option<&str>) -> Command {
        let mut c = Command::new(BIN);
        c.env("XDG_CONFIG_HOME", &self.config)
            .env("XDG_RUNTIME_DIR", &self.runtime)
            .env("SHELL", "/bin/sh")
            .env("KLARK_MACHINE_NAME", "testbox")
            .env_remove("KLARK_SOCKET_PATH")
            .env_remove("KLARK_PANE_ID")
            .env_remove("KLARK_HUB_URL")
            .env_remove("KLARK_HUB_JWT")
            .env_remove("KLARK_HUB_TOKEN");
        match session {
            Some(s) => c.env("KLARK_SESSION", s),
            None => c.env_remove("KLARK_SESSION"),
        };
        c
    }

    fn spawn(&mut self, mut c: Command) {
        let child = c
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn klark");
        support::register_spawned_klark_pid(Some(child.id()));
        self.children.push(child);
    }

    fn socket(&self, session: &str) -> PathBuf {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            for app in ["klark", "klark-dev"] {
                let dir = self.config.join(app);
                let dir = if session == "default" {
                    dir
                } else {
                    dir.join("sessions").join(session)
                };
                let sock = dir.join("klark.sock");
                if UnixStream::connect(&sock).is_ok() {
                    return sock;
                }
            }
            assert!(
                Instant::now() < deadline,
                "socket for {session} never appeared"
            );
            thread::sleep(Duration::from_millis(50));
        }
    }
}

fn call(socket: &Path, method: &str, params: Value) -> Value {
    let mut s = UnixStream::connect(socket).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let mut line = json!({"id": "t", "method": method, "params": params}).to_string();
    line.push('\n');
    s.write_all(line.as_bytes()).unwrap();
    let mut out = String::new();
    BufReader::new(s).read_line(&mut out).unwrap();
    serde_json::from_str(&out).unwrap()
}

fn new_pane(socket: &Path, cwd: &Path) -> String {
    let created = call(
        socket,
        "workspace.create",
        json!({"cwd": cwd, "focus": true}),
    );
    created["result"]["root_pane"]["pane_id"]
        .as_str()
        .unwrap_or_else(|| panic!("workspace.create: {created}"))
        .to_string()
}

/// What the machine sent: a JSON message, or terminal output for a pane.
enum In {
    Json(Value),
    Frame(String, Vec<u8>),
}

fn read_in(ws: &mut WebSocket<TcpStream>) -> Option<In> {
    let gunzip = |b: &[u8]| {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(b)
            .read_to_end(&mut out)
            .unwrap();
        out
    };
    match ws.read() {
        Ok(Message::Binary(b)) if b.first() == Some(&0x1f) => {
            Some(In::Json(serde_json::from_slice(&gunzip(&b)).unwrap()))
        }
        Ok(Message::Binary(b)) if matches!(b.first(), Some(1 | 2)) => {
            let n = b[1] as usize;
            let id = String::from_utf8_lossy(&b[2..2 + n]).to_string();
            let body = if b[0] == 1 {
                gunzip(&b[2 + n..])
            } else {
                b[2 + n..].to_vec()
            };
            Some(In::Frame(id, body))
        }
        Ok(Message::Text(t)) => Some(In::Json(serde_json::from_str(&t).unwrap())),
        Ok(_) => None,
        Err(tungstenite::Error::Io(e))
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            None
        }
        Err(e) => panic!("hub read: {e}"),
    }
}

fn send(ws: &mut WebSocket<TcpStream>, v: Value) {
    ws.send(Message::text(v.to_string())).unwrap();
}

/// Read until `f` accepts a message, or time out.
fn until<T>(ws: &mut WebSocket<TcpStream>, what: &str, mut f: impl FnMut(In) -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        if let Some(msg) = read_in(ws) {
            if let Some(v) = f(msg) {
                return v;
            }
        }
    }
}

#[test]
fn expose_serves_every_running_session() {
    let mut env = Env::new();
    env.spawn({
        let mut c = env.cmd(None);
        c.arg("server");
        c
    });
    env.spawn({
        let mut c = env.cmd(Some("work"));
        c.arg("server");
        c
    });
    let default = env.socket("default");
    let work = env.socket("work");
    let d_pane = new_pane(&default, &env.base);
    let w_pane = new_pane(&work, &env.base);

    let hub = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}/agent", hub.local_addr().unwrap());
    env.spawn({
        let mut c = env.cmd(None);
        c.args(["expose", "--hub", &url, "--token", "t0ken"]);
        c
    });
    let (stream, _) = hub.accept().unwrap();
    let mut ws = tungstenite::accept(stream).unwrap();
    ws.get_ref()
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    until(&mut ws, "hello", |m| match m {
        In::Json(v) if v["type"] == "hello" => Some(()),
        _ => None,
    });

    // One tree, both sessions; `work` ids namespaced, its workspace labelled.
    send(&mut ws, json!({"type": "get_tree"}));
    let tree = until(&mut ws, "tree", |m| match m {
        In::Json(v) if v["type"] == "tree" => Some(v),
        _ => None,
    });
    let snap = &tree["snapshot"];
    let panes: Vec<&str> = snap["panes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["pane_id"].as_str())
        .collect();
    let w_wire = format!("work/{w_pane}");
    assert!(panes.contains(&d_pane.as_str()), "{snap}");
    assert!(panes.contains(&w_wire.as_str()), "{snap}");
    let w = snap["panes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["pane_id"] == json!(w_wire))
        .unwrap();
    assert!(w["workspace_id"].as_str().unwrap().starts_with("work/"));
    assert!(snap["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .any(|ws| ws["workspace_id"] == w["workspace_id"]
            && ws["label"].as_str().unwrap().starts_with("work · ")));
    let rev = tree["rev"].as_str().unwrap().to_string();
    send(&mut ws, json!({"type": "get_tree", "known_rev": rev}));
    until(&mut ws, "tree_unchanged", |m| match m {
        In::Json(v) if v["type"] == "tree_unchanged" => Some(()),
        In::Json(v) if v["type"] == "tree" => Some(()), // a prompt redraw may change the tree
        _ => None,
    });

    // Subscribe to the work pane: frames come back under its namespaced id.
    send(&mut ws, json!({"type": "subscribe", "pane_id": w_wire}));
    until(&mut ws, "a frame for the work pane", |m| match m {
        In::Frame(id, body) => (id == w_wire && !body.is_empty()).then_some(()),
        In::Json(v) if v["type"] == "closed" => panic!("work pane closed: {v}"),
        _ => None,
    });

    // Input to the work pane lands in the work session, not the default one.
    send(
        &mut ws,
        json!({"type": "input", "pane_id": w_wire, "text": "echo EXPOSE_$((40+2))_OK"}),
    );
    send(
        &mut ws,
        json!({"type": "keys", "pane_id": w_wire, "keys": ["Enter"]}),
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let read = call(
            &work,
            "pane.read",
            json!({"pane_id": w_pane, "source": "recent", "lines": 40}),
        );
        if read.to_string().contains("EXPOSE_42_OK") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "input never reached the work pane: {read}"
        );
        let _ = read_in(&mut ws);
    }

    // A call addressed to the work pane is routed (prefix stripped).
    send(
        &mut ws,
        json!({"type": "call", "id": "r1", "method": "pane.read", "params": {"pane_id": w_wire, "source": "recent", "lines": 40}}),
    );
    let res = until(&mut ws, "call result", |m| match m {
        In::Json(v) if v["type"] == "result" && v["id"] == "r1" => Some(v),
        _ => None,
    });
    assert!(res["error"].is_null(), "{res}");
    assert!(res.to_string().contains("EXPOSE_42_OK"), "{res}");
}
