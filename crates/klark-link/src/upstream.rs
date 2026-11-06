//! Talking to the local klark server: one-shot socket API calls and per-pane
//! terminal observers. Ported from klark-expose's Go `internal/upstream`.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
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
}

impl Upstream {
    pub fn new(socket: PathBuf, bin: PathBuf) -> Self {
        Self {
            socket,
            bin,
            timeout: Duration::from_secs(10),
        }
    }

    pub fn socket(&self) -> &Path {
        &self.socket
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

    #[cfg(not(unix))]
    pub fn call(&self, method: &str, _params: Value) -> Result<Value, String> {
        Err(format!(
            "{method}: klark link is not supported on this platform yet"
        ))
    }

    pub fn snapshot(&self) -> Result<Value, String> {
        let result = self.call("session.snapshot", Value::Null)?;
        Ok(result.get("snapshot").cloned().unwrap_or(result))
    }

    /// Start `klark terminal session observe <pane>`; every output chunk is
    /// forwarded as an `AgentMessage::Frame`. Observe never resizes the pane.
    pub fn observe(&self, pane_id: &str, out: Sender<AgentMessage>) -> Result<Observer, String> {
        let mut child = Command::new(&self.bin)
            .args(["terminal", "session", "observe", pane_id])
            .env("KLARK_SOCKET_PATH", &self.socket)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("observe {pane_id}: {e}"))?;
        let stdout = child.stdout.take().ok_or("observe: no stdout")?;
        let pane = pane_id.to_string();
        thread::Builder::new()
            .name(format!("klark-link-observe-{pane}"))
            .spawn(move || {
                let mut reason = "eof".to_string();
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    match parse_observe_line(&line) {
                        ObserveLine::Bytes(bytes) => {
                            if out
                                .send(AgentMessage::Frame {
                                    pane_id: pane.clone(),
                                    bytes,
                                })
                                .is_err()
                            {
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
                let _ = out.send(AgentMessage::Closed {
                    pane_id: pane,
                    reason,
                });
            })
            .map_err(|e| e.to_string())?;
        Ok(Observer { child })
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

/// A running pane observer; killed when dropped.
pub struct Observer {
    child: Child,
}

impl Drop for Observer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
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
    Bytes(String),
    Closed(String),
    Ignore,
}

fn parse_observe_line(line: &str) -> ObserveLine {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return ObserveLine::Ignore;
    };
    if let Some(bytes) = v.get("bytes").and_then(Value::as_str) {
        return ObserveLine::Bytes(bytes.to_string());
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
            ObserveLine::Bytes("aGk=".into())
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
