#![cfg(unix)]
//! Drive `run_once` against a fake hub (WebSocket server) and a fake klark API
//! socket: hello -> tree -> allowed call answered, forbidden call refused,
//! input forwarded as pane.send_text.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::net::UnixListener;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};
use tungstenite::Message;

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

    // Fake hub.
    let hub = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}/agent", hub.local_addr().unwrap());
    let config = klark_link::LinkConfig {
        hub_url: url,
        token: "t0ken".into(),
        machine_id: "m1".into(),
        machine_name: "m1".into(),
        klark_version: "test".into(),
        socket: sock.clone(),
        bin: "/nonexistent/klark".into(),
        uploads_dir: std::env::temp_dir().join("klark-link-e2e-uploads"),
        sessions: None,
    };
    thread::spawn(move || {
        let _ = klark_link::run_once(&config);
    });

    let (stream, _) = hub.accept().unwrap();
    let auth = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let auth_seen = auth.clone();
    let mut ws = tungstenite::accept_hdr(
        stream,
        move |req: &tungstenite::handshake::server::Request, resp| {
            *auth_seen.lock().unwrap() = req
                .headers()
                .get("authorization")
                .unwrap()
                .to_str()
                .unwrap()
                .to_string();
            Ok(resp)
        },
    )
    .unwrap();
    assert_eq!(*auth.lock().unwrap(), "Bearer t0ken");
    ws.get_ref()
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();

    let hello = next(&mut ws);
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["machine_id"], "m1");
    // Nothing else arrives unasked: the hub pulls the tree.
    ws.send(Message::text(json!({"type": "get_tree"}).to_string()))
        .unwrap();
    let tree = next(&mut ws);
    assert_eq!(tree["type"], "tree");
    assert_eq!(tree["snapshot"]["panes"][0]["pane_id"], "p1");
    let rev = tree["rev"].as_str().unwrap().to_string();
    ws.send(Message::text(
        json!({"type": "get_tree", "known_rev": rev}).to_string(),
    ))
    .unwrap();
    let again = next(&mut ws);
    assert_eq!(again, json!({"type": "tree_unchanged", "rev": rev}));

    let send = |ws: &mut tungstenite::WebSocket<std::net::TcpStream>, v: Value| {
        ws.send(Message::text(v.to_string())).unwrap();
    };
    send(
        &mut ws,
        json!({"type": "call", "id": "1", "method": "pane.read", "params": {"pane_id": "p1", "source": "visible"}}),
    );
    send(
        &mut ws,
        json!({"type": "call", "id": "2", "method": "pane.close", "params": {"pane_id": "p1"}}),
    );
    send(
        &mut ws,
        json!({"type": "input", "pane_id": "p1", "text": "yes"}),
    );
    send(&mut ws, json!({"type": "ping"}));

    let mut results = Vec::new();
    while results.len() < 3 {
        if let Some(v) = decode(ws.read().unwrap()) {
            {
                results.push(v);
            }
        }
    }
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
    let _ = std::fs::remove_dir_all(dir);
}

/// Machine messages arrive gzip-compressed in binary frames.
fn decode(msg: Message) -> Option<Value> {
    use std::io::Read as _;
    match msg {
        Message::Binary(b) => {
            let mut json = Vec::new();
            flate2::read::GzDecoder::new(&b[..])
                .read_to_end(&mut json)
                .unwrap();
            Some(serde_json::from_slice(&json).unwrap())
        }
        Message::Text(t) => Some(serde_json::from_str(&t).unwrap()),
        _ => None,
    }
}

fn next(ws: &mut tungstenite::WebSocket<std::net::TcpStream>) -> Value {
    loop {
        if let Some(v) = decode(ws.read().unwrap()) {
            return v;
        }
    }
}
