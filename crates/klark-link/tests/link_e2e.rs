#![cfg(unix)]
//! Drive `run_once` against a fake hub (HTTP, ADR 0013) and a fake klark API
//! socket: hello -> tree -> allowed call answered, forbidden call refused,
//! input forwarded as pane.send_text.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use klark_link::fakehub::{FakeHub, In};
use serde_json::{json, Value};

#[test]
fn link_serves_a_hub_lazily() {
    let dir = std::env::temp_dir().join(format!("klark-link-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("api.sock");
    let _ = std::fs::remove_file(&sock);

    // Fake klark API: answer every request, record the methods it saw.
    let api = UnixListener::bind(&sock).unwrap();
    let (seen_tx, seen_rx) = mpsc::channel::<(String, Value)>();
    thread::spawn(move || {
        for conn in api.incoming() {
            let Ok(conn) = conn else { break };
            let mut reader = BufReader::new(conn.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let req: Value = serde_json::from_str(&line).unwrap();
            let method = req["method"].as_str().unwrap().to_string();
            let result = match method.as_str() {
                "session.snapshot" => {
                    json!({"snapshot": {"panes": [{"pane_id": "p1", "agent_status": "blocked"}]}})
                }
                "pane.read" => json!({"read": {"text": "hello"}}),
                _ => json!({}),
            };
            let _ = seen_tx.send((method, req["params"].clone()));
            let mut conn = conn;
            let _ = writeln!(conn, "{}", json!({"id": req["id"], "result": result}));
        }
    });

    // Fake hub (crates/klark-link/src/fakehub.rs).
    let hub = FakeHub::start();
    let config = klark_link::LinkConfig {
        hub_url: hub.url(),
        token: "t0ken".into(),
        machine_id: "m1".into(),
        machine_name: "m1".into(),
        klark_version: "test".into(),
        socket: sock.clone(),
        bin: "/nonexistent/klark".into(),
        uploads_dir: std::env::temp_dir().join("klark-link-e2e-uploads"),
        sessions: None,
        agents: None,
    };
    thread::spawn(move || {
        let _ = klark_link::run_once(&config);
    });

    let hello = hub.until("hello", |m| match m {
        In::Hello(v) => Some(v),
        _ => None,
    });
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["machine_id"], "m1");
    assert_eq!(hello["protocol"], 4);
    assert_eq!(hub.auth(), vec!["Bearer t0ken".to_string()]);
    // Nothing else arrives unasked: the hub pulls the tree.
    assert_eq!(hub.recv(Duration::from_millis(600)), None);
    hub.send(json!({"type": "get_tree"}));
    let tree = next(&hub);
    assert_eq!(tree["type"], "tree");
    assert_eq!(tree["snapshot"]["panes"][0]["pane_id"], "p1");
    let rev = tree["rev"].as_str().unwrap().to_string();
    hub.send(json!({"type": "get_tree", "known_rev": rev}));
    let again = next(&hub);
    assert_eq!(again, json!({"type": "tree_unchanged", "rev": rev}));

    hub.send(
        json!({"type": "call", "id": "1", "method": "pane.read", "params": {"pane_id": "p1", "source": "visible"}}),
    );
    hub.send(
        json!({"type": "call", "id": "2", "method": "pane.close", "params": {"pane_id": "p1"}}),
    );
    hub.send(json!({"type": "input", "pane_id": "p1", "text": "yes"}));
    hub.send(json!({"type": "ping"}));

    let results: Vec<Value> = (0..3).map(|_| next(&hub)).collect();
    assert_eq!(
        results[0],
        json!({"type": "result", "id": "1", "result": {"read": {"text": "hello"}}})
    );
    assert_eq!(
        results[1]["error"],
        "pane.close is not allowed from the hub"
    );
    assert_eq!(results[2], json!({"type": "pong"}));

    let seen: Vec<(String, Value)> = seen_rx.try_iter().collect();
    assert!(seen
        .iter()
        .any(|(m, p)| m == "pane.send_text" && p["text"] == "yes"));
    assert!(!seen.iter().any(|(m, _)| m == "pane.close"));
    // Pushes are sequenced 1, 2, 3, ... with no gaps.
    let seqs = hub.push_seqs();
    assert!(!seqs.is_empty());
    assert_eq!(seqs, (1..=seqs.len() as u64).collect::<Vec<_>>());
    // The hub's heartbeat lines are not answered: the idle link stays quiet.
    let before = hub.push_seqs().len();
    thread::sleep(Duration::from_millis(1200));
    assert_eq!(hub.push_seqs().len(), before);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn link_reconnects_with_a_new_session_after_the_hub_forgets_it() {
    let hub = FakeHub::start();
    let config = klark_link::LinkConfig {
        hub_url: format!("ws://{}/agent", hub.url().trim_start_matches("http://")),
        token: "t".into(),
        machine_id: "m2".into(),
        machine_name: "m2".into(),
        klark_version: "test".into(),
        socket: "/nonexistent/api.sock".into(),
        bin: "/nonexistent/klark".into(),
        uploads_dir: std::env::temp_dir().join("klark-link-e2e-uploads"),
        sessions: None,
        agents: None,
    };
    // `run` is the forever loop with jittered backoff (1 s first).
    thread::spawn(move || klark_link::run(config));
    hub.until("first hello", |m| matches!(m, In::Hello(_)).then_some(()));
    assert_eq!(hub.sessions(), 1);
    hub.send(json!({"type": "ping"}));
    assert_eq!(next(&hub), json!({"type": "pong"}));

    hub.drop_session();
    hub.until("second hello", |m| matches!(m, In::Hello(_)).then_some(()));
    assert_eq!(hub.sessions(), 2);
    // The new session works, with its own sequence.
    hub.send(json!({"type": "ping"}));
    assert_eq!(next(&hub), json!({"type": "pong"}));
}

#[test]
fn output_is_batched_into_few_segments() {
    use base64::Engine as _;
    let f = |s: &str| klark_link::protocol::AgentMessage::Frame {
        pane_id: "p1".into(),
        bytes: base64::engine::general_purpose::STANDARD.encode(s),
    };
    // 200 small writes in one tick become one frame in one segment.
    let msgs: Vec<_> = (0..200).map(|i| f(&format!("line {i}\r\n"))).collect();
    let merged = klark_link::coalesce(msgs);
    assert_eq!(merged.len(), 1);
    let (wire, _) = klark_link::transport::encode_batch(&merged).unwrap();
    assert!(wire.len() < 1200, "{} bytes", wire.len());
}

/// The next JSON message the machine pushed.
fn next(hub: &FakeHub) -> Value {
    hub.until("a message", |m| match m {
        In::Json(v) => Some(v),
        _ => None,
    })
}
