//! klark-link: the machine side of the klark hub.
//!
//! A klark machine dials OUT to a hub over WebSocket (no inbound port, no
//! tunnel), authenticates with a machine token, and then serves the hub lazily:
//! the pane tree on connect and on change, terminal bytes only for panes the hub
//! subscribes to, and reads only when asked.

pub mod assets;
pub mod files;
pub mod filter;
pub mod http;
pub mod protocol;
pub mod sessions;
pub mod tls;
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

use filter::{AgentFilter, AgentInfo};
use protocol::{call_allowed, AgentMessage, HubMessage, PROTOCOL_VERSION};
use sessions::{decode_id, SessionSource, DEFAULT_SESSION};
use upstream::{MsgReply, Observer, Upstream};

/// Upper bound on one loop turn, and so on how long pane output is merged
/// before sending. 50 ms halves message count vs 25 ms and lets gzip pay off
/// (docs/experiments/E-03). Input from the hub is handled as soon as it arrives.
const READ_TICK: Duration = Duration::from_millis(50);
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
    /// Where `fetch_asset` stores downloads: `<klark data dir>/uploads`.
    pub uploads_dir: PathBuf,
    /// Every running session on this machine. `socket` serves the default
    /// session; with a source, the other sessions are exposed too, their ids
    /// namespaced `<session>/<id>` (see `sessions`). `None` exposes only
    /// `socket`, as before.
    pub sessions: Option<SessionSource>,
    /// `--agents`: expose only the panes running these agents (see `filter`).
    /// `None` exposes every pane, as before.
    pub agents: Option<AgentFilter>,
}

/// Connect to the hub and serve it forever, reconnecting with backoff.
pub fn run(config: LinkConfig) -> ! {
    let mut backoff = Duration::from_secs(1);
    loop {
        let started = Instant::now();
        match run_once(&config) {
            Ok(()) => eprintln!("klark expose: hub closed the connection"),
            Err(e) => eprintln!("klark expose: {e}"),
        }
        if started.elapsed() > Duration::from_secs(60) {
            backoff = Duration::from_secs(1);
        }
        let wait = jittered(backoff, &config.machine_id);
        eprintln!("klark expose: reconnecting in {:.1}s", wait.as_secs_f64());
        std::thread::sleep(wait);
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

/// Backoff with ±50% jitter, so 100 machines that lost the same hub don't all
/// reconnect in the same instant (STATUS S-14, experiment E-10). The jitter is
/// seeded from the machine id and the clock: no RNG dependency is needed.
pub fn jittered(base: Duration, machine_id: &str) -> Duration {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    machine_id.hash(&mut h);
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
        .hash(&mut h);
    jitter_from(base, h.finish())
}

fn jitter_from(base: Duration, seed: u64) -> Duration {
    // Uniform in [0.5, 1.5) × base.
    let factor = 0.5 + (seed % 1_000_000) as f64 / 1_000_000.0;
    base.mul_f64(factor)
}

/// The browser link to a pane on the hub, built from the hub URL the link uses.
///
/// `ws://hub:8080/agent` becomes `http://hub:8080/go?machine=<name>&pane=<id>`
/// (`wss` becomes `https`). The hub's `/go` route finds the machine by the name
/// it reported in `hello`. This is what "share this" hands back (ADR 0008).
pub fn pane_url(hub_url: &str, machine_name: &str, pane_id: &str) -> Result<String, String> {
    let mut url = url::Url::parse(hub_url).map_err(|e| format!("hub url {hub_url}: {e}"))?;
    let scheme = match url.scheme() {
        "ws" | "http" => "http",
        "wss" | "https" => "https",
        other => {
            return Err(format!(
                "hub url scheme {other} is not ws, wss, http or https"
            ))
        }
    };
    url.set_scheme(scheme)
        .map_err(|_| "cannot set url scheme".to_string())?;
    url.set_path("/go");
    url.set_query(None);
    url.query_pairs_mut()
        .append_pair("machine", machine_name)
        .append_pair("pane", pane_id);
    Ok(url.to_string())
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
    let (ws, _) = ws_connect(request).map_err(|e| format!("connect {hub_url}: {e}"))?;
    set_read_timeout(&ws, Some(READ_TICK));
    Ok(ws)
}

/// The WebSocket handshake; `wss://` uses `tls::client_config` (webpki
/// roots + KLARK_HUB_CA_FILE).
fn ws_connect(
    request: tungstenite::handshake::client::Request,
) -> Result<(Socket, tungstenite::handshake::client::Response), String> {
    let uri = request.uri().clone();
    let tls = match uri.scheme_str() {
        Some("ws") => false,
        Some("wss") => true,
        other => return Err(format!("unsupported scheme {other:?}")),
    };
    let host = uri.host().ok_or("no host")?.to_string();
    let port = uri.port_u16().unwrap_or(if tls { 443 } else { 80 });
    let host_only = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let tcp = TcpStream::connect((host_only.as_str(), port)).map_err(|e| e.to_string())?;
    let _ = tcp.set_nodelay(true);
    let connector = if tls {
        tungstenite::Connector::Rustls(tls::client_config()?)
    } else {
        tungstenite::Connector::Plain
    };
    tungstenite::client_tls_with_config(request, tcp, None, Some(connector)).map_err(|e| match e {
        tungstenite::HandshakeError::Failure(e) => e.to_string(),
        tungstenite::HandshakeError::Interrupted(_) => "handshake interrupted".to_string(),
    })
}

pub fn run_once(config: &LinkConfig) -> Result<(), String> {
    let mut ws = connect_hub(&config.hub_url, &config.token, &config.machine_id)?;
    eprintln!("klark expose: connected to {}", config.hub_url);

    let upstream = Upstream::new(config.socket.clone(), config.bin.clone());
    let mut session = Session::new(upstream);
    session.sessions = config.sessions.clone();
    session.agents = config.agents.clone();
    session.assets = Some(AssetTarget {
        hub_base: http::hub_http_base(&config.hub_url)?,
        token: config.token.clone(),
        uploads_dir: config.uploads_dir.clone(),
    });

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
                Some(Err(e)) => eprintln!("klark expose: ignoring unknown hub message: {e}"),
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

/// Where `fetch_asset` downloads from and stores to.
#[derive(Debug, Clone)]
pub struct AssetTarget {
    pub hub_base: String,
    /// The machine token: the hub serves an asset only to the machine it is
    /// addressed to.
    pub token: String,
    pub uploads_dir: PathBuf,
}

/// Per-connection state: which panes are being streamed, and the last tree sent.
pub struct Session {
    upstream: Upstream,
    observers: HashMap<String, Observer>,
    frames_tx: Sender<AgentMessage>,
    frames_rx: Receiver<AgentMessage>,
    pub assets: Option<AssetTarget>,
    /// Lists the other running sessions; `None` serves `upstream` alone.
    pub sessions: Option<SessionSource>,
    /// Upstreams of non-default sessions, resolved lazily and dropped on error.
    others: HashMap<String, Upstream>,
    default_down_logged: bool,
    /// `--agents`; `None` exposes everything.
    pub agents: Option<AgentFilter>,
    /// Every agent with whether it is exposed, and when that was computed.
    exposure: Option<(Instant, Vec<(AgentInfo, bool)>)>,
}

/// How long an exposure decision is reused for input and reads. A miss always
/// recomputes, so an agent that just started is never refused for long.
const EXPOSURE_TTL: Duration = Duration::from_secs(2);

impl Session {
    pub fn new(upstream: Upstream) -> Self {
        let (frames_tx, frames_rx) = mpsc::channel();
        Self {
            upstream,
            observers: HashMap::new(),
            frames_tx,
            frames_rx,
            assets: None,
            sessions: None,
            others: HashMap::new(),
            default_down_logged: false,
            agents: None,
            exposure: None,
        }
    }

    /// The upstream serving a session, resolving and caching it on first use.
    fn upstream_for(&mut self, session: &str) -> Result<Upstream, String> {
        if session == DEFAULT_SESSION {
            return Ok(self.upstream.clone());
        }
        if let Some(up) = self.others.get(session) {
            return Ok(up.clone());
        }
        let socket = self
            .sessions
            .as_ref()
            .and_then(|src| {
                src.running()
                    .into_iter()
                    .find(|(name, _)| name == session)
                    .map(|(_, socket)| socket)
            })
            .ok_or_else(|| format!("no running klark session {session}"))?;
        let up = Upstream::new(socket, self.upstream.bin().to_path_buf()).for_session(session);
        self.others.insert(session.to_string(), up.clone());
        Ok(up)
    }

    /// Forget a session's cached upstream after an error (it may have stopped).
    fn forget(&mut self, session: &str) {
        self.others.remove(session);
    }

    /// One local API call for a wire-addressed request; errors drop the cache.
    fn call_in(&mut self, session: &str, method: &str, params: Value) -> Result<Value, String> {
        let up = self.upstream_for(session)?;
        let out = up.call(method, params);
        if out.is_err() {
            self.forget(session);
        }
        out
    }

    pub fn subscribed(&self) -> impl Iterator<Item = &String> {
        self.observers.keys()
    }

    pub fn handle(&mut self, msg: HubMessage) -> Vec<AgentMessage> {
        if let Some(refused) = self.refuse(&msg) {
            return refused;
        }
        match msg {
            HubMessage::Subscribe { pane_id } => {
                if self.observers.contains_key(&pane_id) {
                    return vec![];
                }
                let (session, local) = decode_id(&pane_id);
                let (session, local) = (session.to_string(), local.to_string());
                let observed = self
                    .upstream_for(&session)
                    .and_then(|up| up.observe(&local, self.frames_tx.clone()));
                match observed {
                    Ok(obs) => {
                        self.observers.insert(pane_id, obs);
                        vec![]
                    }
                    Err(reason) => {
                        self.forget(&session);
                        vec![AgentMessage::Closed { pane_id, reason }]
                    }
                }
            }
            HubMessage::Unsubscribe { pane_id } => {
                self.observers.remove(&pane_id);
                vec![]
            }
            HubMessage::Input { pane_id, text } => {
                let (session, local) = decode_id(&pane_id);
                let params = json!({"pane_id": local, "text": text});
                self.fire(session, "pane.send_text", params)
            }
            HubMessage::Keys { pane_id, keys } => {
                let (session, local) = decode_id(&pane_id);
                let params = json!({"pane_id": local, "keys": keys});
                self.fire(session, "pane.send_keys", params)
            }
            HubMessage::Call { id, method, params } => {
                let (result, error) = if call_allowed(&method) {
                    match sessions::route_params(&params)
                        .and_then(|(session, params)| self.filtered_call(&session, &method, params))
                    {
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
            // v2 messaging (ADR 0009): run our own `klark msg` so local and
            // remote delivery share one code path. Answered asynchronously
            // through the frames channel, so a slow pane never stalls the link.
            HubMessage::ListAgents { id } => {
                let args = vec!["msg".into(), "ls".into(), "--local".into(), "--json".into()];
                match self.agents.clone() {
                    None => self
                        .upstream
                        .run_msg(id, args, MsgReply::Json, self.frames_tx.clone()),
                    Some(filter) => {
                        // Answer through a relay that drops unexposed agents.
                        let (tx, rx) = mpsc::channel();
                        self.upstream.run_msg(id, args, MsgReply::Json, tx);
                        let out = self.frames_tx.clone();
                        let _ = std::thread::Builder::new()
                            .name("klark-link-agents".into())
                            .spawn(move || {
                                if let Ok(mut msg) = rx.recv() {
                                    if let AgentMessage::Result {
                                        result: Some(r), ..
                                    } = &mut msg
                                    {
                                        *r = filter_agent_list(&filter, r);
                                    }
                                    let _ = out.send(msg);
                                }
                            });
                    }
                }
                vec![]
            }
            HubMessage::Deliver {
                id,
                session,
                agent,
                from,
                text,
            } => {
                self.upstream.run_msg(
                    id,
                    deliver_args(&session, &agent, &from, &text),
                    MsgReply::Delivered,
                    self.frames_tx.clone(),
                );
                vec![]
            }
            // v3 (ADR 0011): both run off the loop thread with a timeout.
            HubMessage::GetTranscript {
                id,
                pane_id,
                after,
                before,
                limit,
            } => {
                let (session, local) = decode_id(&pane_id);
                let mut args = transcript_args(local, after.as_deref(), before.as_deref(), limit);
                if session != DEFAULT_SESSION {
                    // The global flag picks the session's socket (src/session.rs).
                    args.splice(0..0, ["--session".to_string(), session.to_string()]);
                }
                self.upstream
                    .run_msg(id, args, MsgReply::Json, self.frames_tx.clone());
                vec![]
            }
            // v4: a file a chat linked to. Checked here (files::check), whatever
            // the hub decided: pane folder or home only, no credentials, size cap.
            HubMessage::GetFile {
                id,
                pane_id,
                path,
                max_bytes,
            } => {
                let (session, local) = decode_id(&pane_id);
                let (session, local) = (session.to_string(), local.to_string());
                let up = match self.upstream_for(&session) {
                    Ok(up) => up,
                    Err(e) => {
                        return vec![AgentMessage::Result {
                            id,
                            result: None,
                            error: Some(e),
                        }]
                    }
                };
                let out = self.frames_tx.clone();
                let spawned = std::thread::Builder::new()
                    .name(format!("klark-link-file-{id}"))
                    .spawn({
                        let id = id.clone();
                        move || {
                            let outcome = up
                                .call("pane.get", json!({"pane_id": local}))
                                .and_then(|info| {
                                    let pane = info.get("pane").cloned().unwrap_or(info);
                                    let cwd = pane
                                        .get("cwd")
                                        .and_then(Value::as_str)
                                        .filter(|c| !c.is_empty())
                                        .map(std::path::PathBuf::from)
                                        .ok_or("the pane has no working directory")?;
                                    let home = files::home_dir();
                                    files::read(&path, &cwd, home.as_deref(), files::max_bytes(max_bytes))
                                });
                            if let Err(e) = &outcome {
                                eprintln!("klark expose: get_file {path}: {e}");
                            }
                            let (result, error) = match outcome {
                                Ok(v) => (Some(v), None),
                                Err(e) => (None, Some(e)),
                            };
                            let _ = out.send(AgentMessage::Result { id, result, error });
                        }
                    });
                match spawned {
                    Ok(_) => vec![],
                    Err(e) => vec![AgentMessage::Result {
                        id,
                        result: None,
                        error: Some(e.to_string()),
                    }],
                }
            }
            HubMessage::FetchAsset {
                id,
                sha256,
                name,
                mime,
                size,
                pane_id,
                caption,
            } => {
                let Some(target) = self.assets.clone() else {
                    return vec![AgentMessage::Result {
                        id,
                        result: None,
                        error: Some("assets are not configured on this link".into()),
                    }];
                };
                let (session, local) = decode_id(&pane_id);
                let (session, pane_id) = (session.to_string(), local.to_string());
                let up = match self.upstream_for(&session) {
                    Ok(up) => up,
                    Err(e) => {
                        return vec![AgentMessage::Result {
                            id,
                            result: None,
                            error: Some(e),
                        }]
                    }
                };
                let out = self.frames_tx.clone();
                let spawned = std::thread::Builder::new()
                    .name(format!("klark-link-asset-{id}"))
                    .spawn({
                        let id = id.clone();
                        move || {
                            let outcome = assets::fetch(
                                &target.hub_base,
                                &target.token,
                                &target.uploads_dir,
                                &sha256,
                                size,
                                mime.as_deref(),
                            )
                            .and_then(|path| {
                                let text =
                                    assets::prompt_text(name.as_deref(), &path, caption.as_deref());
                                prompt_pane(&up, &pane_id, &text)?;
                                Ok(path)
                            });
                            let (result, error) = match outcome {
                                Ok(p) => (Some(json!({"path": p.display().to_string()})), None),
                                Err(e) => (None, Some(e)),
                            };
                            let _ = out.send(AgentMessage::Result { id, result, error });
                        }
                    });
                match spawned {
                    Ok(_) => vec![],
                    Err(e) => vec![AgentMessage::Result {
                        id,
                        result: None,
                        error: Some(e.to_string()),
                    }],
                }
            }
        }
    }

    /// The refusal for a message that addresses a pane or agent `--agents`
    /// does not expose; `None` lets it through.
    fn refuse(&mut self, msg: &HubMessage) -> Option<Vec<AgentMessage>> {
        self.agents.as_ref()?;
        let answer = |id: &str, what: String, me: &Self| {
            vec![AgentMessage::Result {
                id: id.to_string(),
                result: None,
                error: Some(me.not_exposed(&what)),
            }]
        };
        match msg {
            HubMessage::Subscribe { pane_id }
            | HubMessage::Input { pane_id, .. }
            | HubMessage::Keys { pane_id, .. } => {
                let (s, l) = decode_id(pane_id);
                // Anything that types into a pane is checked against a fresh
                // agent list, so a just-renamed agent never takes input.
                let writes = !matches!(msg, HubMessage::Subscribe { .. });
                (!self.exposed(s, l, writes)).then(|| {
                    let reason = self.not_exposed(&format!("pane {pane_id}"));
                    eprintln!("klark expose: refused: {reason}");
                    vec![AgentMessage::Closed {
                        pane_id: pane_id.clone(),
                        reason,
                    }]
                })
            }
            HubMessage::GetTranscript { id, pane_id, .. }
            | HubMessage::GetFile { id, pane_id, .. }
            | HubMessage::FetchAsset { id, pane_id, .. } => {
                let (s, l) = decode_id(pane_id);
                let writes = matches!(msg, HubMessage::FetchAsset { .. });
                (!self.exposed(s, l, writes)).then(|| answer(id, format!("pane {pane_id}"), self))
            }
            HubMessage::Deliver {
                id, session, agent, ..
            } => (!self.exposed(session, agent, true))
                .then(|| answer(id, format!("agent {session}/{agent}"), self)),
            HubMessage::Call { id, params, .. } => {
                let (session, params) = sessions::route_params(params).ok()?;
                let target = ["pane_id", "target"]
                    .iter()
                    .find_map(|k| params.get(*k).and_then(Value::as_str))?
                    .to_string();
                (!self.exposed(&session, &target, false)).then(|| {
                    let wire = sessions::encode_id(&session, &target);
                    answer(id, format!("pane {wire}"), self)
                })
            }
            _ => None,
        }
    }

    fn not_exposed(&self, what: &str) -> String {
        let only = self
            .agents
            .as_ref()
            .map(AgentFilter::describe)
            .unwrap_or_default();
        format!(
            "{what} is not exposed: this machine exposes only agents {only} (klark expose --agents)"
        )
    }

    /// Every agent of every running session (`agent.list`).
    fn agent_infos(&mut self) -> Vec<AgentInfo> {
        let mut names = vec![DEFAULT_SESSION.to_string()];
        if let Some(src) = &self.sessions {
            let mut others: Vec<String> = src
                .running()
                .into_iter()
                .map(|(n, _)| n)
                .filter(|n| n != DEFAULT_SESSION && sessions::valid_session(n))
                .collect();
            others.sort();
            names.extend(others);
        }
        let mut out = Vec::new();
        for name in names {
            let Ok(up) = self.upstream_for(&name) else {
                continue;
            };
            match up.call("agent.list", json!({})) {
                Ok(list) => out.extend(
                    list.get("agents")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|a| AgentInfo::from_json(&name, a))
                        .map(|mut a| {
                            a.session = name.clone();
                            a
                        }),
                ),
                Err(_) => self.forget(&name),
            }
        }
        out
    }

    /// Recompute which agents are exposed (no-op without `--agents`).
    fn refresh_exposure(&mut self) {
        let Some(filter) = self.agents.clone() else {
            return;
        };
        let infos = self.agent_infos();
        let on = filter.exposed(&infos);
        self.exposure = Some((Instant::now(), infos.into_iter().zip(on).collect()));
    }

    /// Whether `id` (a local pane id, or an agent name) in `session` is an
    /// exposed agent. A stale answer, or a miss, is recomputed once; with
    /// `fresh_only` (anything that writes to the pane) it is always recomputed.
    fn exposed(&mut self, session: &str, id: &str, fresh_only: bool) -> bool {
        if self.agents.is_none() {
            return true;
        }
        let check = |me: &Self| {
            me.exposure.as_ref().is_some_and(|(_, all)| {
                all.iter().any(|(a, on)| {
                    *on && a.session == session
                        && (a.pane_id == id
                            || a.name
                                .as_deref()
                                .is_some_and(|n| n.eq_ignore_ascii_case(id)))
                })
            })
        };
        let fresh = !fresh_only
            && self
                .exposure
                .as_ref()
                .is_some_and(|(at, _)| at.elapsed() < EXPOSURE_TTL);
        if fresh && check(self) {
            return true;
        }
        self.refresh_exposure();
        check(self)
    }

    /// A session's snapshot cut down to its exposed panes (unchanged without
    /// `--agents`), by the exposure last computed.
    fn filter_session_snapshot(&self, session: &str, snapshot: &Value) -> Value {
        let Some((_, all)) = self.exposure.as_ref().filter(|_| self.agents.is_some()) else {
            return snapshot.clone();
        };
        let keep: std::collections::HashSet<&str> = all
            .iter()
            .filter(|(a, on)| *on && a.session == session)
            .map(|(a, _)| a.pane_id.as_str())
            .collect();
        filter::filter_snapshot(snapshot, &|p| keep.contains(p))
    }

    /// A hub `call`; with `--agents`, the session-wide reads return only what
    /// is exposed (pane-addressed calls were already checked by `refuse`).
    fn filtered_call(
        &mut self,
        session: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        let out = self.call_in(session, method, params)?;
        if self.agents.is_none() {
            return Ok(out);
        }
        match method {
            "session.snapshot" => {
                self.refresh_exposure();
                Ok(self.filter_session_snapshot(session, &out))
            }
            "workspace.list" | "tab.list" | "pane.list" => {
                self.refresh_exposure();
                let snap = self.call_in(session, "session.snapshot", Value::Null)?;
                let kept = self.filter_session_snapshot(session, &snap);
                Ok(filter::filter_list_result(&out, &kept))
            }
            _ => Ok(out),
        }
    }

    fn fire(&mut self, session: &str, method: &str, params: Value) -> Vec<AgentMessage> {
        if let Err(e) = self.call_in(session, method, params) {
            eprintln!("klark expose: {e}");
        }
        vec![]
    }

    /// Frames and closures produced by observer threads since the last drain.
    pub fn drain(&mut self) -> Vec<AgentMessage> {
        let mut out = Vec::new();
        loop {
            match self.frames_rx.try_recv() {
                Ok(msg) => {
                    match &msg {
                        AgentMessage::Closed { pane_id, .. } => {
                            self.observers.remove(pane_id);
                        }
                        // Never report the size of a pane nobody watches.
                        AgentMessage::PaneSize { pane_id, .. }
                            if !self.observers.contains_key(pane_id) =>
                        {
                            continue
                        }
                        _ => {}
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
    pub fn tree(&mut self, known_rev: Option<String>) -> AgentMessage {
        let snapshot = self.snapshot_all();
        let rev = tree_rev(&snapshot);
        if known_rev.as_deref() == Some(rev.as_str()) {
            AgentMessage::TreeUnchanged { rev }
        } else {
            AgentMessage::Tree { rev, snapshot }
        }
    }
}

impl Session {
    /// The default session's snapshot merged with every other running
    /// session's (namespaced). Sessions that started or stopped since the last
    /// pull simply appear or disappear. Without other sessions the default
    /// snapshot goes out unchanged, exactly as before.
    fn snapshot_all(&mut self) -> Value {
        self.refresh_exposure();
        let default = self
            .upstream
            .snapshot()
            .map(|s| self.filter_session_snapshot(DEFAULT_SESSION, &s));
        let mut running: Vec<(String, PathBuf)> = self
            .sessions
            .as_ref()
            .map(SessionSource::running)
            .unwrap_or_default()
            .into_iter()
            .filter(|(n, _)| n != DEFAULT_SESSION && sessions::valid_session(n))
            .collect();
        running.sort_by(|a, b| a.0.cmp(&b.0));
        if running.is_empty() {
            return default.unwrap_or_else(|e| {
                eprintln!("klark expose: snapshot: {e}");
                json!({"error": e})
            });
        }
        let mut parts = Vec::with_capacity(running.len() + 1);
        match default {
            Ok(s) => {
                self.default_down_logged = false;
                parts.push((DEFAULT_SESSION.to_string(), s))
            }
            // Other sessions run without the default one: say so once, not every refresh.
            Err(e) if !self.default_down_logged => {
                self.default_down_logged = true;
                eprintln!("klark expose: snapshot {DEFAULT_SESSION}: {e} (not logged again until it is back)");
            }
            Err(_) => {}
        }
        self.others
            .retain(|name, _| running.iter().any(|(n, _)| n == name));
        let bin = self.upstream.bin().to_path_buf();
        for (name, socket) in running {
            let up = self
                .others
                .entry(name.clone())
                .or_insert_with(|| Upstream::new(socket, bin.clone()).for_session(&name))
                .clone();
            match up.snapshot() {
                Ok(s) => {
                    let s = self.filter_session_snapshot(&name, &s);
                    parts.push((name, s))
                }
                Err(e) => {
                    eprintln!("klark expose: snapshot {name}: {e}");
                    self.others.remove(&name);
                }
            }
        }
        sessions::merge_snapshots(&parts)
    }
}

/// A `msg ls --local --json` answer cut down to the exposed agents.
pub fn filter_agent_list(filter: &AgentFilter, list: &Value) -> Value {
    let Some(items) = list.as_array() else {
        return list.clone();
    };
    let infos: Vec<AgentInfo> = items
        .iter()
        .filter_map(|v| AgentInfo::from_json(DEFAULT_SESSION, v))
        .collect();
    let exposed = filter.exposed_panes(&infos);
    Value::Array(
        items
            .iter()
            .filter(|v| {
                AgentInfo::from_json(DEFAULT_SESSION, v)
                    .is_some_and(|a| exposed.contains(&(a.session, a.pane_id)))
            })
            .cloned()
            .collect(),
    )
}

/// `klark msg deliver` arguments for a hub `deliver` (ADR 0009). `--` keeps a
/// message that starts with `-` from being read as a flag.
pub fn deliver_args(session: &str, agent: &str, from: &str, text: &str) -> Vec<String> {
    vec![
        "msg".into(),
        "deliver".into(),
        "--session".into(),
        session.into(),
        "--from".into(),
        from.into(),
        agent.into(),
        "--".into(),
        text.into(),
    ]
}

/// Hidden `klark transcript` arguments for a hub `get_transcript`.
pub fn transcript_args(
    pane_id: &str,
    after: Option<&str>,
    before: Option<&str>,
    limit: Option<u32>,
) -> Vec<String> {
    let mut v = vec!["transcript".into(), "--pane".into(), pane_id.into()];
    if let Some(a) = after {
        v.extend(["--after".into(), a.into()]);
    }
    if let Some(b) = before {
        v.extend(["--before".into(), b.into()]);
    }
    if let Some(l) = limit {
        v.extend(["--limit".into(), l.to_string()]);
    }
    v.push("--json".into());
    v
}

/// Prompt the pane's agent (`agent.prompt`, as `klark msg deliver` does). A
/// pane with no detected agent gets the text typed instead (`pane.send_text`).
fn prompt_pane(up: &Upstream, pane_id: &str, text: &str) -> Result<(), String> {
    match up.call("agent.prompt", json!({"target": pane_id, "text": text})) {
        Ok(_) => Ok(()),
        Err(e) => up
            .call("pane.send_text", json!({"pane_id": pane_id, "text": text}))
            .map(|_| ())
            .map_err(|e2| format!("{e}; {e2}")),
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
    fn pane_url_maps_the_link_url_to_the_hub_page() {
        assert_eq!(
            pane_url("ws://hub.lan:8080/agent", "my laptop", "p 1").unwrap(),
            "http://hub.lan:8080/go?machine=my+laptop&pane=p+1"
        );
        assert_eq!(
            pane_url("wss://hub.example.com/agent", "m", "p1").unwrap(),
            "https://hub.example.com/go?machine=m&pane=p1"
        );
        assert!(pane_url("ftp://x/agent", "m", "p").is_err());
        assert!(pane_url("not a url", "m", "p").is_err());
    }

    #[test]
    fn jitter_stays_within_half_to_one_and_a_half() {
        let base = Duration::from_secs(8);
        for seed in [0u64, 1, 499_999, 999_999, u64::MAX, 123_456_789] {
            let d = jitter_from(base, seed);
            assert!(
                d >= Duration::from_secs(4) && d < Duration::from_secs(12),
                "{d:?}"
            );
        }
        assert_ne!(jitter_from(base, 1), jitter_from(base, 600_000));
    }

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
