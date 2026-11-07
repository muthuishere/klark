//! A tiny in-process hub speaking the HTTP link (ADR 0013), for tests here and
//! in the klark crate: sessions, the NDJSON event stream, pushed segments.
//! Not for production use.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// What the machine sent.
#[derive(Debug, Clone, PartialEq)]
pub enum In {
    /// The `hello` that opened a session.
    Hello(Value),
    Json(Value),
    /// Terminal output, decoded.
    Frame(String, Vec<u8>),
}

#[derive(Default)]
struct State {
    /// Current session id; older ones are refused (404).
    session: String,
    next_seq: u64,
    queue: Vec<(u64, Value)>,
    last_push: HashMap<String, u64>,
    push_seqs: Vec<u64>,
    auth: Vec<String>,
}

struct Inner {
    state: Mutex<State>,
    cv: Condvar,
    tx: Mutex<Sender<In>>,
    sessions: AtomicU64,
    heartbeat: Duration,
}

pub struct FakeHub {
    inner: Arc<Inner>,
    rx: Receiver<In>,
    addr: std::net::SocketAddr,
}

impl FakeHub {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        let inner = Arc::new(Inner {
            state: Mutex::new(State::default()),
            cv: Condvar::new(),
            tx: Mutex::new(tx),
            sessions: AtomicU64::new(0),
            heartbeat: Duration::from_millis(500),
        });
        let i = inner.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { break };
                let i = i.clone();
                std::thread::spawn(move || serve(conn, &i));
            }
        });
        Self { inner, rx, addr }
    }

    /// `http://127.0.0.1:<port>`
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Queue a hub message for the machine.
    pub fn send(&self, msg: Value) {
        let mut st = self.inner.state.lock().unwrap();
        st.next_seq += 1;
        let seq = st.next_seq;
        st.queue.push((seq, msg));
        self.inner.cv.notify_all();
    }

    pub fn recv(&self, timeout: Duration) -> Option<In> {
        self.rx.recv_timeout(timeout).ok()
    }

    /// Receive until `f` accepts a message, or panic after 20 s.
    pub fn until<T>(&self, what: &str, mut f: impl FnMut(In) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out waiting for {what}");
            if let Some(m) = self.recv(left.min(Duration::from_millis(200))) {
                if let Some(v) = f(m) {
                    return v;
                }
            }
        }
    }

    /// Sessions opened so far.
    pub fn sessions(&self) -> u64 {
        self.inner.sessions.load(Ordering::SeqCst)
    }

    /// Authorization headers seen on session requests.
    pub fn auth(&self) -> Vec<String> {
        self.inner.state.lock().unwrap().auth.clone()
    }

    /// Every push `seq` accepted, in arrival order (dupes dropped).
    pub fn push_seqs(&self) -> Vec<u64> {
        self.inner.state.lock().unwrap().push_seqs.clone()
    }

    /// Forget the current session, as a hub restart or revoke does: its
    /// stream ends and its pushes get 404, so the machine opens a new one.
    pub fn drop_session(&self) {
        let mut st = self.inner.state.lock().unwrap();
        st.session = String::new();
        st.queue.clear();
        self.inner.cv.notify_all();
    }
}

fn param<'a>(path: &'a str, key: &str) -> Option<&'a str> {
    path.split_once('?')?
        .1
        .split('&')
        .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
}

fn reply(w: &mut TcpStream, status: &str, body: &str) -> std::io::Result<()> {
    write!(
        w,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
}

fn serve(conn: TcpStream, hub: &Inner) {
    let mut w = conn.try_clone().unwrap();
    let mut r = BufReader::new(conn);
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let mut parts = line.split_whitespace();
        let (method, path) = (
            parts.next().unwrap_or("").to_string(),
            parts.next().unwrap_or("").to_string(),
        );
        let (mut len, mut auth, mut gz) = (0usize, String::new(), false);
        loop {
            let mut h = String::new();
            if r.read_line(&mut h).unwrap_or(0) == 0 {
                return;
            }
            if h == "\r\n" {
                break;
            }
            let (k, v) = h.split_once(':').unwrap_or((&h, ""));
            match k.to_ascii_lowercase().as_str() {
                "content-length" => len = v.trim().parse().unwrap_or(0),
                "authorization" => auth = v.trim().to_string(),
                "content-encoding" => gz = v.contains("gzip"),
                _ => {}
            }
        }
        let mut body = vec![0u8; len];
        if r.read_exact(&mut body).is_err() {
            return;
        }
        if gz {
            let mut out = Vec::new();
            let _ = flate2::read::GzDecoder::new(&body[..]).read_to_end(&mut out);
            body = out;
        }
        let route = path.split('?').next().unwrap_or("");
        let current = |hub: &Inner| {
            let st = hub.state.lock().unwrap();
            let s = param(&path, "session").unwrap_or("");
            !st.session.is_empty() && st.session == s
        };
        let ok = match (method.as_str(), route) {
            ("POST", "/agent/session") => {
                let n = hub.sessions.fetch_add(1, Ordering::SeqCst) + 1;
                let id = format!("s{n}");
                {
                    let mut st = hub.state.lock().unwrap();
                    st.session = id.clone();
                    st.auth.push(auth);
                    hub.cv.notify_all();
                }
                let hello = serde_json::from_slice(&body).unwrap_or(Value::Null);
                let _ = hub.tx.lock().unwrap().send(In::Hello(hello));
                reply(&mut w, "200 OK", &json!({"session_id": id}).to_string())
            }
            ("GET", "/agent/events") if current(hub) => {
                let after: u64 = param(&path, "after")
                    .and_then(|a| a.parse().ok())
                    .unwrap_or(0);
                events(&mut w, hub, param(&path, "session").unwrap_or(""), after);
                return;
            }
            ("POST", "/agent/push") if current(hub) => {
                let seq: u64 = param(&path, "seq")
                    .and_then(|a| a.parse().ok())
                    .unwrap_or(0);
                let session = param(&path, "session").unwrap_or("").to_string();
                let fresh = {
                    let mut st = hub.state.lock().unwrap();
                    let last = st.last_push.entry(session).or_default();
                    let fresh = seq > *last;
                    if fresh {
                        *last = seq;
                        st.push_seqs.push(seq);
                    }
                    fresh
                };
                if fresh {
                    let tx = hub.tx.lock().unwrap().clone();
                    for l in String::from_utf8_lossy(&body).lines() {
                        let Ok(v) = serde_json::from_str::<Value>(l) else {
                            continue;
                        };
                        let m = if v["type"] == "frame" {
                            use base64::Engine as _;
                            In::Frame(
                                v["pane_id"].as_str().unwrap_or("").to_string(),
                                match v["text"].as_str() {
                                    Some(t) => t.as_bytes().to_vec(),
                                    None => base64::engine::general_purpose::STANDARD
                                        .decode(v["bytes"].as_str().unwrap_or(""))
                                        .unwrap_or_default(),
                                },
                            )
                        } else {
                            In::Json(v)
                        };
                        let _ = tx.send(m);
                    }
                }
                reply(&mut w, "200 OK", "{}")
            }
            (_, "/agent/events" | "/agent/push") => {
                reply(&mut w, "404 Not Found", r#"{"error":"unknown session"}"#)
            }
            _ => reply(&mut w, "404 Not Found", r#"{"error":"no route"}"#),
        };
        if ok.is_err() {
            return;
        }
    }
}

/// Stream queued messages after `after` as chunked NDJSON until the session ends.
fn events(w: &mut TcpStream, hub: &Inner, session: &str, mut after: u64) {
    let chunk = |w: &mut TcpStream, line: &str| -> std::io::Result<()> {
        write!(w, "{:x}\r\n{line}\n\r\n", line.len() + 1)?;
        w.flush()
    };
    if write!(
        w,
        "HTTP/1.1 200 OK\r\nContent-Type: application/x-ndjson\r\nTransfer-Encoding: chunked\r\nCache-Control: no-cache\r\n\r\n"
    )
    .is_err()
    {
        return;
    }
    let mut st = hub.state.lock().unwrap();
    loop {
        if st.session != session {
            drop(st);
            let _ = write!(w, "0\r\n\r\n");
            return;
        }
        let out: Vec<String> = st
            .queue
            .iter()
            .filter(|(s, _)| *s > after)
            .map(|(s, v)| {
                let mut v = v.clone();
                v["seq"] = json!(s);
                v.to_string()
            })
            .collect();
        if let Some((s, _)) = st.queue.last() {
            after = after.max(*s);
        }
        drop(st);
        for l in &out {
            if chunk(w, l).is_err() {
                return;
            }
        }
        st = hub.state.lock().unwrap();
        if st.queue.last().is_some_and(|(s, _)| *s > after) {
            continue;
        }
        let (g, t) = hub.cv.wait_timeout(st, hub.heartbeat).unwrap();
        st = g;
        if t.timed_out() {
            drop(st);
            if chunk(w, r#"{"type":"ping"}"#).is_err() {
                return;
            }
            st = hub.state.lock().unwrap();
        }
    }
}
