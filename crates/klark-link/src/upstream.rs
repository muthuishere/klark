//! Talking to the local klark server: one-shot socket API calls and per-pane
//! terminal observers. Ported from klark-expose's Go `internal/upstream`.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

use crate::protocol::AgentMessage;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Socket API client. Each call opens a connection, writes one NDJSON request,
/// reads one NDJSON response — the same shape klark-expose relied on.
#[derive(Debug, Clone)]
pub struct Upstream {
    socket: PathBuf,
    bin: PathBuf,
    timeout: Duration,
    /// The klark session this socket belongs to when it is not the default
    /// one. Pane ids this upstream reports to the hub carry it as a prefix
    /// (`<session>/<pane_id>`, see `crate::sessions`).
    session: Option<String>,
}

/// One NDJSON request line.
#[cfg(windows)]
fn request_line(method: &str, params: Value) -> Result<Vec<u8>, String> {
    let params = if params.is_null() { json!({}) } else { params };
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed).to_string();
    let mut line = serde_json::to_vec(&json!({"id": id, "method": method, "params": params}))
        .map_err(|e| e.to_string())?;
    line.push(b'\n');
    Ok(line)
}

/// Connect to the server's pipe, write the request, read one response line.
#[cfg(windows)]
fn windows_exchange(socket: &Path, method: &str, request: &[u8]) -> Result<String, String> {
    use interprocess::local_socket::{prelude::*, GenericNamespaced, Stream};

    let name = socket.to_string_lossy().to_string();
    let name = name
        .to_ns_name::<GenericNamespaced>()
        .map_err(|e| format!("connect {}: {e}", socket.display()))?;
    let mut conn =
        Stream::connect(name).map_err(|e| format!("connect {}: {e}", socket.display()))?;
    conn.write_all(request)
        .map_err(|e| format!("write {method}: {e}"))?;
    let mut reader = BufReader::new(conn);
    let mut buf = String::new();
    reader
        .read_line(&mut buf)
        .map_err(|e| format!("read {method}: {e}"))?;
    Ok(buf)
}

impl Upstream {
    pub fn new(socket: PathBuf, bin: PathBuf) -> Self {
        Self {
            socket,
            bin,
            timeout: Duration::from_secs(10),
            session: None,
        }
    }

    /// The same upstream, labelled as serving a named (non-default) session.
    pub fn for_session(mut self, session: &str) -> Self {
        self.session = (session != crate::sessions::DEFAULT_SESSION).then(|| session.to_string());
        self
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    pub fn bin(&self) -> &Path {
        &self.bin
    }

    /// The pane id the hub knows for a local pane id of this upstream.
    pub fn wire_id(&self, local: &str) -> String {
        crate::sessions::encode_id(
            self.session
                .as_deref()
                .unwrap_or(crate::sessions::DEFAULT_SESSION),
            local,
        )
    }

    #[cfg(unix)]
    pub fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        use std::os::unix::net::UnixStream;

        let mut conn = UnixStream::connect(&self.socket)
            .map_err(|e| format!("connect {}: {e}", self.socket.display()))?;
        conn.set_read_timeout(Some(self.timeout)).ok();
        conn.set_write_timeout(Some(self.timeout)).ok();
        let params = if params.is_null() { json!({}) } else { params };
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed).to_string();
        let mut line = serde_json::to_vec(&json!({"id": id, "method": method, "params": params}))
            .map_err(|e| e.to_string())?;
        line.push(b'\n');
        conn.write_all(&line)
            .map_err(|e| format!("write {method}: {e}"))?;

        let mut reader = BufReader::new(conn);
        let mut buf = String::new();
        reader
            .read_line(&mut buf)
            .map_err(|e| format!("read {method}: {e}"))?;
        parse_response(method, &buf)
    }

    /// Windows: the server listens on a named pipe whose name is the socket
    /// path (namespaced, as `src/ipc.rs` binds it). A pipe has no read
    /// timeout, so the exchange runs on a helper thread and the caller stops
    /// waiting after `timeout`; the thread ends when the server answers or
    /// closes the pipe.
    #[cfg(windows)]
    pub fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let request = request_line(method, params)?;
        let socket = self.socket.clone();
        let m = method.to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        thread::Builder::new()
            .name("klark-link-call".into())
            .spawn(move || {
                let _ = tx.send(windows_exchange(&socket, &m, &request));
            })
            .map_err(|e| format!("{method}: {e}"))?;
        match rx.recv_timeout(self.timeout) {
            Ok(Ok(line)) => parse_response(method, &line),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(format!("read {method}: timed out")),
        }
    }

    #[cfg(not(any(unix, windows)))]
    pub fn call(&self, method: &str, _params: Value) -> Result<Value, String> {
        Err(format!(
            "{method}: klark expose is not supported on this platform yet"
        ))
    }

    pub fn snapshot(&self) -> Result<Value, String> {
        let result = self.call("session.snapshot", Value::Null)?;
        Ok(result.get("snapshot").cloned().unwrap_or(result))
    }

    /// The pane's real PTY size (cols, rows).
    ///
    /// `terminal session observe` does NOT resize the pane: the server renders
    /// the pane's grid into a virtual area of the observer's handshake size
    /// (default 120x40, src/cli.rs), clipping or padding it. So the exposer
    /// must pass the pane's own size. The API has no size field, but
    /// `pane.process_info` names the pane's tty, and TIOCGWINSZ on that tty is
    /// exactly what the program in the pane sees.
    pub fn pane_size(&self, pane_id: &str) -> Option<(u16, u16)> {
        let info = self
            .call("pane.process_info", json!({"pane_id": pane_id}))
            .ok()?;
        let info = info.get("process_info").unwrap_or(&info);
        // `tty` is not filled in by the server today (src/app/api/panes.rs
        // handle_pane_process_info sets None), so derive it from the shell pid.
        let tty = match info.get("tty").and_then(Value::as_str) {
            Some(t) => PathBuf::from(t),
            None => tty_of_pid(info.get("shell_pid").and_then(Value::as_u64)? as u32)?,
        };
        tty_size(&tty)
    }

    /// Stream `klark terminal session observe <pane>` at the pane's own size.
    /// Every output chunk is forwarded as an `AgentMessage::Frame`; the size
    /// as `AgentMessage::PaneSize` on start and on every change. A pane resize
    /// restarts the observer at the new size (its first frame is full).
    pub fn observe(&self, pane_id: &str, out: Sender<AgentMessage>) -> Result<Observer, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let slot: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
        let first = self.spawn_observer(pane_id, self.pane_size(pane_id))?;
        let up = self.clone();
        let pane = pane_id.to_string();
        let (stop2, slot2) = (stop.clone(), slot.clone());
        thread::Builder::new()
            .name(format!("klark-link-observe-{pane}"))
            .spawn(move || up.supervise(pane, first, out, stop2, slot2))
            .map_err(|e| e.to_string())?;
        Ok(Observer { stop, child: slot })
    }

    fn spawn_observer(
        &self,
        pane_id: &str,
        size: Option<(u16, u16)>,
    ) -> Result<(Child, Option<(u16, u16)>), String> {
        let mut cmd = Command::new(&self.bin);
        cmd.args(["terminal", "session", "observe", pane_id]);
        if let Some((cols, rows)) = size {
            cmd.args(["--cols", &cols.to_string(), "--rows", &rows.to_string()]);
        }
        let child = cmd
            .env("KLARK_SOCKET_PATH", &self.socket)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("observe {pane_id}: {e}"))?;
        Ok((child, size))
    }

    fn supervise(
        &self,
        pane: String,
        first: (Child, Option<(u16, u16)>),
        out: Sender<AgentMessage>,
        stop: Arc<AtomicBool>,
        slot: Arc<Mutex<Option<Child>>>,
    ) {
        let wire = self.wire_id(&pane);
        let mut last_sent: Option<(u16, u16)> = None;
        let send_size = |size: (u16, u16), last: &mut Option<(u16, u16)>| {
            if *last != Some(size) && !stop.load(Ordering::SeqCst) {
                *last = Some(size);
                let _ = out.send(AgentMessage::PaneSize {
                    pane_id: wire.clone(),
                    cols: size.0,
                    rows: size.1,
                });
            }
        };
        let mut next = Some(first);
        loop {
            let (mut child, size) = match next.take() {
                Some(c) => c,
                None => match self.spawn_observer(&pane, self.pane_size(&pane)) {
                    Ok(c) => c,
                    Err(reason) => {
                        if !stop.load(Ordering::SeqCst) {
                            let _ = out.send(AgentMessage::Closed {
                                pane_id: wire.clone(),
                                reason,
                            });
                        }
                        return;
                    }
                },
            };
            let Some(stdout) = child.stdout.take() else {
                return;
            };
            *slot.lock().unwrap() = Some(child);
            if stop.load(Ordering::SeqCst) {
                kill_slot(&slot);
                return;
            }
            if let Some(size) = size {
                send_size(size, &mut last_sent);
            }
            // Poll the pane's size; on a change, kill this observer so the
            // read loop below ends and we restart at the new size.
            let resized = Arc::new(AtomicBool::new(false));
            let done = Arc::new(AtomicBool::new(false));
            if size.is_some() {
                let (up, pane, slot, stop) =
                    (self.clone(), pane.clone(), slot.clone(), stop.clone());
                let (resized, done) = (resized.clone(), done.clone());
                let _ = thread::Builder::new()
                    .name(format!("klark-link-size-{pane}"))
                    .spawn(move || {
                        while !done.load(Ordering::SeqCst) && !stop.load(Ordering::SeqCst) {
                            thread::sleep(SIZE_POLL);
                            if done.load(Ordering::SeqCst) {
                                return;
                            }
                            if let Some(now) = up.pane_size(&pane) {
                                if Some(now) != size {
                                    resized.store(true, Ordering::SeqCst);
                                    kill_slot(&slot);
                                    return;
                                }
                            }
                        }
                    });
            }
            let mut reason = "eof".to_string();
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                match parse_observe_line(&line) {
                    ObserveLine::Frame { bytes, size, .. } => {
                        if let Some(size) = size {
                            send_size(size, &mut last_sent);
                        }
                        if out
                            .send(AgentMessage::Frame {
                                pane_id: wire.clone(),
                                bytes,
                            })
                            .is_err()
                        {
                            done.store(true, Ordering::SeqCst);
                            kill_slot(&slot);
                            return;
                        }
                    }
                    ObserveLine::Closed(r) => {
                        reason = r;
                        break;
                    }
                    ObserveLine::Ignore => {}
                }
            }
            done.store(true, Ordering::SeqCst);
            kill_slot(&slot);
            if stop.load(Ordering::SeqCst) {
                return;
            }
            if resized.load(Ordering::SeqCst) {
                continue;
            }
            let _ = out.send(AgentMessage::Closed {
                pane_id: wire,
                reason,
            });
            return;
        }
    }

    /// Run `klark <args>` (a `klark msg` subcommand) on a thread with a
    /// timeout, and send its outcome to the hub as `result {id, ...}`.
    pub fn run_msg(
        &self,
        id: String,
        args: Vec<String>,
        reply: MsgReply,
        out: Sender<AgentMessage>,
    ) {
        let bin = self.bin.clone();
        let spawned = thread::Builder::new()
            .name(format!("klark-link-msg-{id}"))
            .spawn({
                let id = id.clone();
                let out = out.clone();
                move || {
                    let outcome = run_with_timeout(&bin, &args, MSG_TIMEOUT);
                    let _ = out.send(msg_result(id, reply, outcome));
                }
            });
        if let Err(e) = spawned {
            let _ = out.send(AgentMessage::Result {
                id,
                result: None,
                error: Some(format!("klark msg: {e}")),
            });
        }
    }
}

/// How the hub wants a `klark msg` run answered.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MsgReply {
    /// stdout is JSON; it is the result.
    Json,
    /// Success is `{"delivered": true}`.
    Delivered,
}

const MSG_TIMEOUT: Duration = Duration::from_secs(20);

/// (exit ok, stdout, stderr)
type RunOutcome = Result<(bool, String, String), String>;

fn run_with_timeout(bin: &Path, args: &[String], timeout: Duration) -> RunOutcome {
    use std::io::Read as _;
    let mut child = Command::new(bin)
        .args(args)
        .env_remove("KLARK_PANE_ID")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("klark msg: {e}"))?;
    // Drain pipes on threads so a large `ls` cannot fill a pipe and deadlock.
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out_t = thread::spawn(move || {
        let mut s = String::new();
        if let Some(o) = stdout.as_mut() {
            let _ = o.read_to_string(&mut s);
        }
        s
    });
    let err_t = thread::spawn(move || {
        let mut s = String::new();
        if let Some(e) = stderr.as_mut() {
            let _ = e.read_to_string(&mut s);
        }
        s
    });
    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("klark msg timed out after {}s", timeout.as_secs()));
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(format!("klark msg: {e}")),
        }
    };
    let stdout = out_t.join().unwrap_or_default();
    let stderr = err_t.join().unwrap_or_default();
    Ok((status.success(), stdout, stderr))
}

fn msg_result(id: String, reply: MsgReply, outcome: RunOutcome) -> AgentMessage {
    let (result, error) = match outcome {
        Err(e) => (None, Some(e)),
        Ok((false, _, stderr)) => {
            let e = stderr.trim();
            (
                None,
                Some(if e.is_empty() {
                    "klark msg failed".into()
                } else {
                    e.to_string()
                }),
            )
        }
        Ok((true, stdout, _)) => match reply {
            MsgReply::Delivered => (Some(json!({"delivered": true})), None),
            MsgReply::Json => match serde_json::from_str::<Value>(stdout.trim()) {
                Ok(v) => (Some(v), None),
                Err(e) => (None, Some(format!("klark msg ls: bad JSON: {e}"))),
            },
        },
    };
    AgentMessage::Result { id, result, error }
}

/// How often a subscribed pane's size is checked for a resize.
const SIZE_POLL: Duration = Duration::from_secs(1);

/// A running pane observer; killed when dropped, and then silent (no
/// `closed`, no `pane_size`) since the pane is no longer subscribed.
pub struct Observer {
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
}

impl Drop for Observer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        kill_slot(&self.child);
    }
}

fn kill_slot(slot: &Mutex<Option<Child>>) {
    if let Some(mut child) = slot.lock().unwrap_or_else(|e| e.into_inner()).take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// TIOCGWINSZ on a tty path: (cols, rows), or None.
#[cfg(unix)]
pub fn tty_size(tty: &Path) -> Option<(u16, u16)> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK)
        .open(tty)
        .ok()?;
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::ioctl(f.as_raw_fd(), libc::TIOCGWINSZ, &mut ws) };
    (rc == 0 && ws.ws_col > 0 && ws.ws_row > 0).then_some((ws.ws_col, ws.ws_row))
}

#[cfg(not(unix))]
pub fn tty_size(_tty: &Path) -> Option<(u16, u16)> {
    None
}

/// The controlling tty of a process: /proc on Linux, `ps` elsewhere.
pub fn tty_of_pid(pid: u32) -> Option<PathBuf> {
    if let Ok(p) = std::fs::read_link(format!("/proc/{pid}/fd/0")) {
        if p.starts_with("/dev/") {
            return Some(p);
        }
    }
    let out = Command::new("ps")
        .args(["-o", "tty=", "-p", &pid.to_string()])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if name.is_empty() || name.contains('?') || name.contains('/') && !name.starts_with("pts/") {
        return None;
    }
    Some(PathBuf::from("/dev").join(name))
}

fn parse_response(method: &str, line: &str) -> Result<Value, String> {
    let resp: Value =
        serde_json::from_str(line.trim()).map_err(|e| format!("decode {method}: {e}"))?;
    if let Some(err) = resp.get("error").filter(|e| !e.is_null()) {
        let msg = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("error");
        let code = err.get("code").and_then(Value::as_str).unwrap_or("");
        return Err(format!("{method}: {code} {msg}").trim().to_string());
    }
    Ok(resp.get("result").cloned().unwrap_or(Value::Null))
}

#[derive(Debug, PartialEq)]
enum ObserveLine {
    /// `size` is the frame's (width, height) when it reports one; `full`
    /// marks a complete repaint.
    Frame {
        bytes: String,
        size: Option<(u16, u16)>,
        full: bool,
    },
    Closed(String),
    Ignore,
}

fn parse_observe_line(line: &str) -> ObserveLine {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return ObserveLine::Ignore;
    };
    if let Some(bytes) = v.get("bytes").and_then(Value::as_str) {
        let dim = |k: &str| {
            v.get(k)
                .and_then(Value::as_u64)
                .and_then(|n| u16::try_from(n).ok())
                .filter(|n| *n > 0)
        };
        return ObserveLine::Frame {
            bytes: bytes.to_string(),
            size: dim("width").zip(dim("height")),
            full: v.get("full").and_then(Value::as_bool).unwrap_or(false),
        };
    }
    if v.get("type").and_then(Value::as_str) == Some("terminal.closed") {
        let reason = v.get("reason").and_then(Value::as_str).unwrap_or("closed");
        return ObserveLine::Closed(reason.to_string());
    }
    ObserveLine::Ignore
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_error_is_surfaced() {
        let err = parse_response(
            "pane.read",
            r#"{"id":"1","error":{"code":"not_found","message":"no pane"}}"#,
        )
        .unwrap_err();
        assert_eq!(err, "pane.read: not_found no pane");
        let ok = parse_response("ping", r#"{"id":"1","result":{"version":"0.9.5"}}"#).unwrap();
        assert_eq!(ok["version"], "0.9.5");
    }

    #[cfg(unix)]
    #[test]
    fn msg_runs_answer_the_hub() {
        let sh = Path::new("/bin/sh");
        let args = |s: &str| vec!["-c".to_string(), s.to_string()];
        let ok = run_with_timeout(
            sh,
            &args("echo '[{\"session\":\"work\"}]'"),
            Duration::from_secs(5),
        );
        assert_eq!(
            msg_result("1".into(), MsgReply::Json, ok),
            AgentMessage::Result {
                id: "1".into(),
                result: Some(json!([{"session": "work"}])),
                error: None
            }
        );
        let ok = run_with_timeout(sh, &args("true"), Duration::from_secs(5));
        assert_eq!(
            msg_result("2".into(), MsgReply::Delivered, ok),
            AgentMessage::Result {
                id: "2".into(),
                result: Some(json!({"delivered": true})),
                error: None
            }
        );
        let fail = run_with_timeout(
            sh,
            &args("echo 'agent is blocked' >&2; exit 1"),
            Duration::from_secs(5),
        );
        assert_eq!(
            msg_result("3".into(), MsgReply::Delivered, fail),
            AgentMessage::Result {
                id: "3".into(),
                result: None,
                error: Some("agent is blocked".into())
            }
        );
        let slow = run_with_timeout(sh, &args("sleep 5"), Duration::from_millis(200));
        assert!(slow.unwrap_err().contains("timed out"));
    }

    #[test]
    fn deliver_args_shape() {
        assert_eq!(
            crate::deliver_args("work", "reviewer", "m/default/cli", "-n hi"),
            [
                "msg",
                "deliver",
                "--session",
                "work",
                "--from",
                "m/default/cli",
                "reviewer",
                "--",
                "-n hi"
            ]
        );
    }

    #[test]
    fn observe_lines() {
        assert_eq!(
            parse_observe_line(r#"{"bytes":"aGk="}"#),
            ObserveLine::Frame {
                bytes: "aGk=".into(),
                size: None,
                full: false
            }
        );
        assert_eq!(
            parse_observe_line(
                r#"{"type":"terminal.frame","seq":3,"encoding":"ansi","width":97,"height":31,"full":true,"bytes":"aGk="}"#
            ),
            ObserveLine::Frame {
                bytes: "aGk=".into(),
                size: Some((97, 31)),
                full: true
            }
        );
        assert_eq!(
            parse_observe_line(r#"{"type":"terminal.closed","reason":"exited"}"#),
            ObserveLine::Closed("exited".into())
        );
        assert_eq!(parse_observe_line("garbage"), ObserveLine::Ignore);
    }

    #[cfg(unix)]
    #[test]
    fn call_over_unix_socket() {
        use std::os::unix::net::UnixListener;
        let dir = std::env::temp_dir().join(format!("klark-link-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("api.sock");
        let _ = std::fs::remove_file(&sock);
        let listener = UnixListener::bind(&sock).unwrap();
        let server = thread::spawn(move || {
            let (conn, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(conn.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let req: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(req["method"], "session.snapshot");
            let mut conn = conn;
            writeln!(
                conn,
                r#"{{"id":{},"result":{{"snapshot":{{"panes":[]}}}}}}"#,
                req["id"]
            )
            .unwrap();
        });
        let up = Upstream::new(sock, PathBuf::from("klark"));
        assert_eq!(up.snapshot().unwrap(), json!({"panes": []}));
        server.join().unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }
}
