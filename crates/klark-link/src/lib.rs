//! klark-link: the machine side of the klark hub.
//!
//! A klark machine dials OUT to a hub over WebSocket (no inbound port, no
//! tunnel), authenticates with a machine token, and then serves the hub lazily:
//! the pane tree on connect and on change, terminal bytes only for panes the hub
//! subscribes to, and reads only when asked.

pub mod protocol;
pub mod upstream;

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tungstenite::client::IntoClientRequest;
use tungstenite::http::HeaderValue;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use protocol::{call_allowed, AgentMessage, HubMessage, PROTOCOL_VERSION};
use upstream::{Observer, Upstream};

const READ_TICK: Duration = Duration::from_millis(25);
/// Handshake header carrying the machine's own id.
pub const MACHINE_HEADER: &str = "X-Klark-Machine";
const MAX_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct LinkConfig {
    /// e.g. `wss://hub.example.com/agent`
    pub hub_url: String,
    pub token: String,
    pub machine_id: String,
    pub machine_name: String,
    pub klark_version: String,
    pub socket: PathBuf,
    pub bin: PathBuf,
}

/// Connect to the hub and serve it forever, reconnecting with backoff.
pub fn run(config: LinkConfig) -> ! {
    let mut backoff = Duration::from_secs(1);
    loop {
        let started = Instant::now();
        match run_once(&config) {
            Ok(()) => eprintln!("klark link: hub closed the connection"),
            Err(e) => eprintln!("klark link: {e}"),
        }
        if started.elapsed() > Duration::from_secs(60) {
            backoff = Duration::from_secs(1);
        }
        eprintln!("klark link: reconnecting in {}s", backoff.as_secs());
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

pub type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

/// One connection's lifetime.
/// Open the hub WebSocket with the auth and machine headers.
pub fn connect_hub(hub_url: &str, token: &str, machine_id: &str) -> Result<Socket, String> {
    let mut request = hub_url.into_client_request().map_err(|e| e.to_string())?;
    let bearer = HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| "token contains invalid header characters".to_string())?;
    request.headers_mut().insert("Authorization", bearer);
    if let Ok(id) = HeaderValue::from_str(machine_id) {
        request.headers_mut().insert(MACHINE_HEADER, id);
    }
    let (ws, _) = tungstenite::connect(request).map_err(|e| format!("connect {hub_url}: {e}"))?;
    set_read_timeout(&ws, Some(READ_TICK));
    Ok(ws)
}

pub fn run_once(config: &LinkConfig) -> Result<(), String> {
    let mut ws = connect_hub(&config.hub_url, &config.token, &config.machine_id)?;
    eprintln!("klark link: connected to {}", config.hub_url);

    let upstream = Upstream::new(config.socket.clone(), config.bin.clone());
    let mut session = Session::new(upstream);

    send(
        &mut ws,
        &AgentMessage::Hello {
            protocol: PROTOCOL_VERSION,
            machine_id: config.machine_id.clone(),
            machine_name: config.machine_name.clone(),
            klark_version: config.klark_version.clone(),
        },
    )?;

    loop {
        // Inbound from the hub.
        match ws.read() {
            Ok(Message::Close(_)) => return Ok(()),
            Ok(msg) => match decode_hub(msg) {
                Some(Ok(msg)) => {
                    for reply in session.handle(msg) {
                        send(&mut ws, &reply)?;
                    }
                }
                Some(Err(e)) => eprintln!("klark link: ignoring unknown hub message: {e}"),
                None => {}
            },
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(tungstenite::Error::ConnectionClosed) => return Ok(()),
            Err(e) => return Err(format!("read: {e}")),
        }

        // Outbound: only output of panes the hub subscribed to.
        for msg in coalesce(session.drain()) {
            send(&mut ws, &msg)?;
        }
    }
}

pub fn send(ws: &mut Socket, msg: &AgentMessage) -> Result<(), String> {
    ws.send(Message::binary(encode(msg)?))
        .map_err(|e| format!("send: {e}"))
}

/// Wire encoding: gzip-compressed JSON in a binary frame (ADR 0001, D5).
pub fn encode(msg: &AgentMessage) -> Result<Vec<u8>, String> {
    if let AgentMessage::Frame { pane_id, bytes } = msg {
        use base64::Engine as _;
        if let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(bytes.as_bytes()) {
            if pane_id.len() <= u8::MAX as usize {
                return Ok(encode_frame(pane_id, &raw));
            }
        }
    }
    let json = serde_json::to_vec(msg).map_err(|e| e.to_string())?;
    Ok(gzip(&json))
}

/// Binary frame layouts (ADR 0001, D5):
///   0x1f …                       gzip JSON message (the gzip magic byte)
///   0x01 len id gzip(raw)        terminal output, compressed
///   0x02 len id raw              terminal output, too small to be worth gzip
/// Raw output is never base64'd on the wire: base64 inflates it by a third and
/// defeats gzip.
pub const FRAME_GZIP: u8 = 0x01;
pub const FRAME_RAW: u8 = 0x02;

pub fn encode_frame(pane_id: &str, raw: &[u8]) -> Vec<u8> {
    let compressed = if raw.len() >= 128 {
        Some(gzip(raw))
    } else {
        None
    };
    let (kind, body) = match &compressed {
        Some(gz) if gz.len() < raw.len() => (FRAME_GZIP, gz.as_slice()),
        _ => (FRAME_RAW, raw),
    };
    let mut out = Vec::with_capacity(2 + pane_id.len() + body.len());
    out.push(kind);
    out.push(pane_id.len() as u8);
    out.extend_from_slice(pane_id.as_bytes());
    out.extend_from_slice(body);
    out
}

fn gzip(data: &[u8]) -> Vec<u8> {
    use std::io::Write as _;
    let mut gz = flate2::write::GzEncoder::new(
        Vec::with_capacity(data.len() / 3 + 32),
        flate2::Compression::fast(),
    );
    let _ = gz.write_all(data);
    gz.finish().unwrap_or_default()
}

/// Decode a hub message: gzip JSON in a binary frame, or plain JSON text.
pub fn decode_hub(msg: Message) -> Option<Result<HubMessage, String>> {
    use std::io::Read as _;
    match msg {
        Message::Text(text) => Some(serde_json::from_str(&text).map_err(|e| e.to_string())),
        Message::Binary(bytes) => {
            let mut json = Vec::new();
            if let Err(e) = flate2::read::GzDecoder::new(&bytes[..]).read_to_end(&mut json) {
                return Some(Err(e.to_string()));
            }
            Some(serde_json::from_slice(&json).map_err(|e| e.to_string()))
        }
        _ => None,
    }
}

/// Merge consecutive output for the same pane into one frame, so a chatty
/// pane costs one message per tick instead of one per write (ADR 0001, D5).
pub fn coalesce(messages: Vec<AgentMessage>) -> Vec<AgentMessage> {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut out: Vec<AgentMessage> = Vec::with_capacity(messages.len());
    let mut pending: Vec<(String, Vec<u8>)> = Vec::new();
    let flush = |pending: &mut Vec<(String, Vec<u8>)>, out: &mut Vec<AgentMessage>| {
        for (pane_id, bytes) in pending.drain(..) {
            out.push(AgentMessage::Frame {
                pane_id,
                bytes: b64.encode(bytes),
            });
        }
    };
    for msg in messages {
        match msg {
            AgentMessage::Frame { pane_id, bytes } => {
                let Ok(raw) = b64.decode(bytes.as_bytes()) else {
                    continue;
                };
                match pending.iter_mut().find(|(p, _)| *p == pane_id) {
                    Some((_, buf)) => buf.extend_from_slice(&raw),
                    None => pending.push((pane_id, raw)),
                }
            }
            other => {
                // Order matters only within a pane: flush before a pane's close.
                flush(&mut pending, &mut out);
                out.push(other);
            }
        }
    }
    flush(&mut pending, &mut out);
    out
}

fn set_read_timeout(ws: &Socket, timeout: Option<Duration>) {
    let tcp = match ws.get_ref() {
        MaybeTlsStream::Plain(s) => s,
        MaybeTlsStream::Rustls(s) => s.get_ref(),
        _ => return,
    };
    let _ = tcp.set_read_timeout(timeout);
}

/// Per-connection state: which panes are being streamed, and the last tree sent.
pub struct Session {
    upstream: Upstream,
    observers: HashMap<String, Observer>,
    frames_tx: Sender<AgentMessage>,
    frames_rx: Receiver<AgentMessage>,
}

impl Session {
    pub fn new(upstream: Upstream) -> Self {
        let (frames_tx, frames_rx) = mpsc::channel();
        Self {
            upstream,
            observers: HashMap::new(),
            frames_tx,
            frames_rx,
        }
    }

    pub fn subscribed(&self) -> impl Iterator<Item = &String> {
        self.observers.keys()
    }

    pub fn handle(&mut self, msg: HubMessage) -> Vec<AgentMessage> {
        match msg {
            HubMessage::Subscribe { pane_id } => {
                if self.observers.contains_key(&pane_id) {
                    return vec![];
                }
                match self.upstream.observe(&pane_id, self.frames_tx.clone()) {
                    Ok(obs) => {
                        self.observers.insert(pane_id, obs);
                        vec![]
                    }
                    Err(reason) => vec![AgentMessage::Closed { pane_id, reason }],
                }
            }
            HubMessage::Unsubscribe { pane_id } => {
                self.observers.remove(&pane_id);
                vec![]
            }
            HubMessage::Input { pane_id, text } => {
                self.fire("pane.send_text", json!({"pane_id": pane_id, "text": text}))
            }
            HubMessage::Keys { pane_id, keys } => {
                self.fire("pane.send_keys", json!({"pane_id": pane_id, "keys": keys}))
            }
            HubMessage::Call { id, method, params } => {
                let (result, error) = if call_allowed(&method) {
                    match self.upstream.call(&method, params) {
                        Ok(v) => (Some(v), None),
                        Err(e) => (None, Some(e)),
                    }
                } else {
                    (None, Some(format!("{method} is not allowed from the hub")))
                };
                vec![AgentMessage::Result { id, result, error }]
            }
            HubMessage::GetTree { known_rev } => vec![self.tree(known_rev)],
            HubMessage::Ping => vec![AgentMessage::Pong],
        }
    }

    fn fire(&self, method: &str, params: Value) -> Vec<AgentMessage> {
        if let Err(e) = self.upstream.call(method, params) {
            eprintln!("klark link: {e}");
        }
        vec![]
    }

    /// Frames and closures produced by observer threads since the last drain.
    pub fn drain(&mut self) -> Vec<AgentMessage> {
        let mut out = Vec::new();
        loop {
            match self.frames_rx.try_recv() {
                Ok(msg) => {
                    if let AgentMessage::Closed { pane_id, .. } = &msg {
                        self.observers.remove(pane_id);
                    }
                    out.push(msg);
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => return out,
            }
            if out.len() >= 4096 {
                return out;
            }
        }
    }

    /// The tree, or `TreeUnchanged` when the hub already holds this revision.
    pub fn tree(&self, known_rev: Option<String>) -> AgentMessage {
        let snapshot = match self.upstream.snapshot() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("klark link: snapshot: {e}");
                serde_json::json!({"error": e})
            }
        };
        let rev = tree_rev(&snapshot);
        if known_rev.as_deref() == Some(rev.as_str()) {
            AgentMessage::TreeUnchanged { rev }
        } else {
            AgentMessage::Tree { rev, snapshot }
        }
    }
}

/// Content revision of a tree: equal trees give equal revisions.
pub fn tree_rev(snapshot: &Value) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    snapshot.to_string().hash(&mut h);
    format!("{:016x}", h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_coalesce_per_pane_and_keep_close_order() {
        let f = |p: &str, b: &str| AgentMessage::Frame {
            pane_id: p.into(),
            bytes: b.into(),
        };
        // "aGk=" = "hi", "IQ==" = "!", "eA==" = "x"
        let out = coalesce(vec![
            f("p1", "aGk="),
            f("p2", "eA=="),
            f("p1", "IQ=="),
            AgentMessage::Closed {
                pane_id: "p2".into(),
                reason: "eof".into(),
            },
        ]);
        assert_eq!(
            out,
            vec![
                f("p1", "aGkh"),
                f("p2", "eA=="),
                AgentMessage::Closed {
                    pane_id: "p2".into(),
                    reason: "eof".into()
                },
            ]
        );
    }

    #[test]
    fn frames_go_binary_without_base64() {
        let raw = "compiling crate\r\n".repeat(50);
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &raw);
        let wire = encode(&AgentMessage::Frame {
            pane_id: "p1".into(),
            bytes: b64,
        })
        .unwrap();
        assert_eq!(wire[0], FRAME_GZIP);
        assert_eq!(&wire[2..4], b"p1");
        assert!(
            wire.len() < raw.len() / 4,
            "{} bytes for {}",
            wire.len(),
            raw.len()
        );
        let small = encode(&AgentMessage::Frame {
            pane_id: "p1".into(),
            bytes: "aGk=".into(),
        })
        .unwrap();
        assert_eq!(small, vec![FRAME_RAW, 2, b'p', b'1', b'h', b'i']);
    }

    #[test]
    fn encode_round_trips_through_gzip() {
        use std::io::Read as _;
        let bytes = encode(&AgentMessage::Pong).unwrap();
        let mut json = String::new();
        flate2::read::GzDecoder::new(&bytes[..])
            .read_to_string(&mut json)
            .unwrap();
        assert_eq!(json, r#"{"type":"pong"}"#);
    }

    #[test]
    fn disallowed_call_is_answered_with_error() {
        let mut s = Session::new(Upstream::new("/nonexistent".into(), "klark".into()));
        let out = s.handle(HubMessage::Call {
            id: "7".into(),
            method: "pane.close".into(),
            params: Value::Null,
        });
        assert_eq!(
            out,
            vec![AgentMessage::Result {
                id: "7".into(),
                result: None,
                error: Some("pane.close is not allowed from the hub".into())
            }]
        );
        assert_eq!(s.handle(HubMessage::Ping), vec![AgentMessage::Pong]);
    }

    #[test]
    fn failed_subscribe_reports_closed() {
        let mut s = Session::new(Upstream::new(
            "/nonexistent".into(),
            "/nonexistent/klark".into(),
        ));
        let out = s.handle(HubMessage::Subscribe {
            pane_id: "p1".into(),
        });
        assert!(matches!(&out[..], [AgentMessage::Closed { pane_id, .. }] if pane_id == "p1"));
        assert_eq!(s.subscribed().count(), 0);
    }
}
