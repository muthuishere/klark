//! The machine <-> hub transport, plain HTTP only (ADR 0013):
//!
//! - `POST /agent/session` (machine token, `hello` as the JSON body) opens a
//!   link session and answers `{"session_id"}`.
//! - Down: `GET /agent/events?session=<id>&after=<seq>` is held open and
//!   streams NDJSON, one hub message per line, each with a `seq`; a
//!   `{"type":"ping"}` line (no seq) every 15 s keeps proxies and the reader
//!   awake. After a drop the reader resumes with `after=<last seq>`.
//! - Up: `POST /agent/push?session=<id>&seq=<n>`, a short request per batch
//!   (NDJSON, one machine message per line) on one kept-alive
//!   connection; gzip above 1 KB, small segments plain. The hub ignores a
//!   `seq` it already has, so a retried push is
//!   harmless.
//!
//! No WebSocket, no streamed request body: it passes any proxy, ingress or
//! corporate firewall that passes ordinary HTTP.

use std::io::BufRead;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::http::{self, KeepAlive};
use crate::protocol::{AgentMessage, HubMessage};
use crate::MACHINE_HEADER;

/// The hub sends a ping line every 15 s; three missed ones mean the stream is dead.
pub const EVENTS_IDLE_TIMEOUT: Duration = Duration::from_secs(45);
const PUSH_TIMEOUT: Duration = Duration::from_secs(20);
/// Consecutive failed resumes of the event stream before the link gives up
/// and the caller opens a new session.
const MAX_RESUMES: u32 = 3;

/// One link session with a hub.
pub struct HubLink {
    base: String,
    session: String,
    headers: String,
    push: KeepAlive,
    seq: u64,
    stop: Arc<AtomicBool>,
    /// Hub messages in order; an `Err` means the stream is gone for good.
    pub events: Receiver<Result<HubMessage, String>>,
}

impl Drop for HubLink {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn auth_header(token: &str) -> Result<String, String> {
    if token.contains(['\r', '\n']) {
        return Err("token contains invalid header characters".into());
    }
    Ok(format!("Authorization: Bearer {token}\r\n"))
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

impl HubLink {
    /// Open a session (`hello` is its body) and start reading the event stream.
    /// `hub_url` is any hub URL (`http(s)://host:port[/...]`; old `ws(s)://`
    /// URLs map to `http(s)`).
    pub fn open(
        hub_url: &str,
        token: &str,
        machine_id: &str,
        hello: &AgentMessage,
    ) -> Result<Self, String> {
        let base = http::hub_http_base(hub_url)?;
        // Every request carries the token; only the session request names the
        // machine (fleet tokens), later ones are tied to the session.
        let headers = auth_header(token)?;
        let mut first = headers.clone();
        if !machine_id.is_empty() && !machine_id.contains(['\r', '\n']) {
            first.push_str(&format!("{MACHINE_HEADER}: {machine_id}\r\n"));
        }
        let mut push = KeepAlive::new(&base, PUSH_TIMEOUT);
        let body = serde_json::to_vec(hello).map_err(|e| e.to_string())?;
        let (status, resp) = push.send(
            "POST",
            "/agent/session",
            &format!("{first}Content-Type: application/json\r\n"),
            &body,
        )?;
        let resp = http::HttpResponse {
            status,
            body: String::from_utf8_lossy(&resp).into_owned(),
        };
        if !resp.ok() {
            return Err(format!("{base}/agent/session: {}", resp.error_text()));
        }
        let session = serde_json::from_str::<Value>(&resp.body)
            .ok()
            .and_then(|v| {
                v.get("session_id")
                    .and_then(Value::as_str)
                    .map(String::from)
            })
            .filter(|s| !s.is_empty())
            .ok_or("the hub sent no session_id")?;
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, events) = mpsc::channel();
        let url = format!("{base}/agent/events?session={}", enc(&session));
        let (h, st) = (headers.clone(), stop.clone());
        std::thread::Builder::new()
            .name("klark-link-events".into())
            .spawn(move || read_events(&url, &h, &st, &tx))
            .map_err(|e| e.to_string())?;
        Ok(Self {
            base,
            session,
            headers,
            push,
            seq: 0,
            stop,
            events,
        })
    }

    pub fn session_id(&self) -> &str {
        &self.session
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// Send one batch (a "segment"). Returns its size on the wire.
    pub fn push(&mut self, msgs: &[AgentMessage]) -> Result<usize, String> {
        if msgs.is_empty() {
            return Ok(0);
        }
        let (body, gzipped) = encode_batch(msgs)?;
        self.seq += 1;
        let path = format!(
            "/agent/push?session={}&seq={}",
            enc(&self.session),
            self.seq
        );
        let headers = format!(
            "{}Content-Type: application/x-ndjson\r\n{}",
            self.headers,
            if gzipped {
                "Content-Encoding: gzip\r\n"
            } else {
                ""
            }
        );
        let (status, resp) = self.push.send("POST", &path, &headers, &body)?;
        if !(200..300).contains(&status) {
            let r = http::HttpResponse {
                status,
                body: String::from_utf8_lossy(&resp).into_owned(),
            };
            return Err(format!("push: {}", r.error_text()));
        }
        Ok(body.len())
    }
}

/// Segments smaller than this go out plain: gzip would cost CPU (and its
/// ~20-byte header) for nothing on a keystroke echo.
pub const GZIP_MIN: usize = 1024;

/// A batch on the wire: one JSON message per line, gzip'd when that pays
/// (returns the body and whether it is gzip).
pub fn encode_batch(msgs: &[AgentMessage]) -> Result<(Vec<u8>, bool), String> {
    let mut lines = Vec::with_capacity(256 * msgs.len());
    for m in msgs {
        match frame_text(m) {
            // Terminal output that is UTF-8 goes as JSON text, not base64:
            // base64 grows it by a third and hides its repetition from gzip.
            Some(line) => serde_json::to_writer(&mut lines, &line),
            None => serde_json::to_writer(&mut lines, m),
        }
        .map_err(|e| e.to_string())?;
        lines.push(b'\n');
    }
    if lines.len() < GZIP_MIN {
        return Ok((lines, false));
    }
    let gz = crate::gzip(&lines);
    Ok(if gz.len() < lines.len() {
        (gz, true)
    } else {
        (lines, false)
    })
}

/// `{"type":"frame","pane_id","text"}` for a frame whose bytes are UTF-8.
fn frame_text(m: &AgentMessage) -> Option<Value> {
    use base64::Engine as _;
    let AgentMessage::Frame { pane_id, bytes } = m else {
        return None;
    };
    let raw = base64::engine::general_purpose::STANDARD
        .decode(bytes.as_bytes())
        .ok()?;
    let text = String::from_utf8(raw).ok()?;
    Some(serde_json::json!({"type": "frame", "pane_id": pane_id, "text": text}))
}

/// One NDJSON line from the event stream: `Ok(Some((seq, msg)))` for a hub
/// message, `Ok(None)` for a heartbeat or blank line.
pub fn parse_event(line: &str) -> Result<Option<(u64, HubMessage)>, String> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }
    let v: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
    let Some(seq) = v.get("seq").and_then(Value::as_u64) else {
        // Only the heartbeat comes without a sequence number.
        return Ok(None);
    };
    let msg = serde_json::from_value(v).map_err(|e| e.to_string())?;
    Ok(Some((seq, msg)))
}

fn read_events(
    url: &str,
    headers: &str,
    stop: &AtomicBool,
    tx: &Sender<Result<HubMessage, String>>,
) {
    let mut last = 0u64;
    let mut failures = 0u32;
    while !stop.load(Ordering::Relaxed) {
        let stream =
            http::open_stream(&format!("{url}&after={last}"), headers, EVENTS_IDLE_TIMEOUT);
        let error = match stream {
            // The hub forgot the session (replaced, revoked, hub restarted).
            Err((status @ (401 | 403 | 404 | 410), e)) => {
                let _ = tx.send(Err(format!("events: HTTP {status}: {e}")));
                return;
            }
            Err((_, e)) => e,
            Ok(mut r) => {
                failures = 0;
                let mut line = String::new();
                loop {
                    line.clear();
                    match r.read_line(&mut line) {
                        Ok(0) => break "the hub ended the event stream".to_string(),
                        Ok(_) => {}
                        Err(e) => break format!("events: {e}"),
                    }
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    match parse_event(&line) {
                        Ok(Some((seq, msg))) if seq > last => {
                            last = seq;
                            if tx.send(Ok(msg)).is_err() {
                                return;
                            }
                        }
                        Ok(_) => {}
                        Err(e) => eprintln!("klark expose: ignoring unknown hub message: {e}"),
                    }
                }
            }
        };
        failures += 1;
        if failures > MAX_RESUMES {
            let _ = tx.send(Err(error));
            return;
        }
        std::thread::sleep(Duration::from_millis(250 * failures as u64));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_and_blank_lines_are_not_messages() {
        assert_eq!(parse_event("{\"type\":\"ping\"}\n").unwrap(), None);
        assert_eq!(parse_event("\r\n").unwrap(), None);
        assert_eq!(
            parse_event("{\"seq\":7,\"type\":\"get_tree\"}").unwrap(),
            Some((7, HubMessage::GetTree { known_rev: None }))
        );
        assert!(parse_event("{\"seq\":1,\"type\":\"nope\"}").is_err());
    }

    #[test]
    fn batches_are_ndjson_gzip_only_when_large() {
        use std::io::Read as _;
        let (small, gz) = encode_batch(&[AgentMessage::Pong, AgentMessage::Pong]).unwrap();
        assert!(!gz);
        assert_eq!(small, b"{\"type\":\"pong\"}\n{\"type\":\"pong\"}\n");
        let many = vec![AgentMessage::Pong; 100];
        let (wire, gz) = encode_batch(&many).unwrap();
        assert!(gz && wire.len() < 200, "{}", wire.len());
        let mut s = String::new();
        flate2::read::GzDecoder::new(&wire[..])
            .read_to_string(&mut s)
            .unwrap();
        assert_eq!(s, "{\"type\":\"pong\"}\n".repeat(100));
    }
}
