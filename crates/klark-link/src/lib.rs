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

const TREE_POLL: Duration = Duration::from_secs(2);
const READ_TICK: Duration = Duration::from_millis(25);
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

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

/// One connection's lifetime.
pub fn run_once(config: &LinkConfig) -> Result<(), String> {
    let mut request = config
        .hub_url
        .as_str()
        .into_client_request()
        .map_err(|e| e.to_string())?;
    let bearer = HeaderValue::from_str(&format!("Bearer {}", config.token))
        .map_err(|_| "token contains invalid header characters".to_string())?;
    request.headers_mut().insert("Authorization", bearer);
    let (mut ws, _) =
        tungstenite::connect(request).map_err(|e| format!("connect {}: {e}", config.hub_url))?;
    set_read_timeout(&ws, Some(READ_TICK));
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
            Ok(Message::Text(text)) => match serde_json::from_str::<HubMessage>(&text) {
                Ok(msg) => {
                    for reply in session.handle(msg) {
                        send(&mut ws, &reply)?;
                    }
                }
                Err(e) => eprintln!("klark link: ignoring unknown hub message: {e}"),
            },
            Ok(Message::Close(_)) => return Ok(()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(tungstenite::Error::ConnectionClosed) => return Ok(()),
            Err(e) => return Err(format!("read: {e}")),
        }

        // Outbound: terminal frames, closures, tree changes.
        for msg in session.drain() {
            send(&mut ws, &msg)?;
        }
        if let Some(tree) = session.poll_tree() {
            send(&mut ws, &tree)?;
        }
    }
}

fn send(ws: &mut Socket, msg: &AgentMessage) -> Result<(), String> {
    let text = serde_json::to_string(msg).map_err(|e| e.to_string())?;
    ws.send(Message::text(text))
        .map_err(|e| format!("send: {e}"))
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
    last_tree: Option<Value>,
    next_tree_poll: Instant,
}

impl Session {
    pub fn new(upstream: Upstream) -> Self {
        let (frames_tx, frames_rx) = mpsc::channel();
        Self {
            upstream,
            observers: HashMap::new(),
            frames_tx,
            frames_rx,
            last_tree: None,
            next_tree_poll: Instant::now(),
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
            HubMessage::Refresh => {
                self.last_tree = None;
                self.next_tree_poll = Instant::now();
                vec![]
            }
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
            if out.len() >= 256 {
                return out;
            }
        }
    }

    /// The tree, if it is time to look and it changed since it was last sent.
    pub fn poll_tree(&mut self) -> Option<AgentMessage> {
        if Instant::now() < self.next_tree_poll {
            return None;
        }
        self.next_tree_poll = Instant::now() + TREE_POLL;
        let snapshot = match self.upstream.snapshot() {
            Ok(s) => s,
            Err(e) => {
                eprintln!("klark link: snapshot: {e}");
                return None;
            }
        };
        if self.last_tree.as_ref() == Some(&snapshot) {
            return None;
        }
        self.last_tree = Some(snapshot.clone());
        Some(AgentMessage::Tree { snapshot })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
