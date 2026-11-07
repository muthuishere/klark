//! `klark msg` across two klark sessions on one machine (ADR 0009).
//!
//! Starts two headless servers in one config dir: the default session and a
//! session named `work`. In `work`, a pane runs a stand-in agent: a copy of
//! `cat` named `claude`, so klark's process-based detection marks it as an
//! agent and `agent.prompt` accepts it. Then `klark msg send work/rev ...` run
//! from the default session must land in that pane as the ADR's prompt.

#![cfg(unix)]

pub mod support;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_klark");

struct Env {
    base: PathBuf,
    config: PathBuf,
    runtime: PathBuf,
    servers: Vec<Child>,
}

impl Drop for Env {
    fn drop(&mut self) {
        for s in &mut self.servers {
            let _ = s.kill();
            let _ = s.wait();
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
            "/tmp/klark-msg-test-{}-{nanos}",
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
            servers: Vec::new(),
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

    fn start_server(&mut self, session: Option<&str>) {
        let child = self
            .cmd(session)
            .arg("server")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn klark server");
        support::register_spawned_klark_pid(Some(child.id()));
        self.servers.push(child);
    }

    /// The API socket of a session, in whichever app dir this build uses.
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
                "socket for session {session} never appeared"
            );
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn msg(&self, session: Option<&str>, pane: Option<&str>, args: &[&str]) -> Output {
        let mut c = self.cmd(session);
        if let Some(p) = pane {
            c.env("KLARK_PANE_ID", p);
        }
        c.arg("msg").args(args).output().expect("run klark msg")
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

fn text(o: &Output) -> String {
    format!(
        "status={:?}\nstdout={}\nstderr={}",
        o.status.code(),
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn wait_for<T>(what: &str, timeout: Duration, mut f: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(v) = f() {
            return v;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn msg_send_crosses_sessions_and_lands_as_agent_prompt() {
    let mut env = Env::new();
    env.start_server(None);
    env.start_server(Some("work"));
    let work = env.socket("work");
    let _default = env.socket("default");

    // Both sessions are seen; nothing is an agent yet.
    let ls = env.msg(None, None, &["ls", "--local", "--json"]);
    assert!(ls.status.success(), "{}", text(&ls));
    assert_eq!(
        serde_json::from_slice::<Value>(&ls.stdout).unwrap(),
        json!([])
    );
    let me = env.msg(None, None, &["whoami"]);
    assert_eq!(
        String::from_utf8_lossy(&me.stdout).trim(),
        "testbox/default/cli",
        "{}",
        text(&me)
    );

    // A stand-in agent in session `work`.
    let bin_dir = env.base.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let fake = bin_dir.join("claude");
    // The process must be named `claude`. macOS kills a copied platform binary,
    // so link `cat` there. On Linux the name comes from the file executed and
    // coreutils may be one multi-call binary, so copy `perl` and run it as `-pe ''`.
    if cfg!(target_os = "macos") {
        std::os::unix::fs::symlink("/bin/cat", &fake).unwrap();
    } else {
        std::fs::copy("/usr/bin/perl", &fake).unwrap();
    }
    let created = call(
        &work,
        "workspace.create",
        json!({"cwd": env.base, "focus": true}),
    );
    let pane = created["result"]["root_pane"]["pane_id"]
        .as_str()
        .unwrap_or_else(|| panic!("workspace.create: {created}"))
        .to_string();
    call(
        &work,
        "pane.send_text",
        json!({"pane_id": pane, "text": fake_command(&fake)}),
    );
    call(
        &work,
        "pane.send_keys",
        json!({"pane_id": pane, "keys": ["Enter"]}),
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let list = call(&work, "agent.list", json!({}));
        let found = list["result"]["agents"]
            .as_array()
            .is_some_and(|a| a.iter().any(|a| a["pane_id"] == json!(pane)));
        if found {
            break;
        }
        if Instant::now() > deadline {
            let read = call(
                &work,
                "pane.read",
                json!({"pane_id": pane, "source": "recent", "lines": 20}),
            );
            let info = call(&work, "pane.process_info", json!({"pane_id": pane}));
            panic!("fake agent never detected\nagent.list={list}\nread={read}\nprocess={info}");
        }
        thread::sleep(Duration::from_millis(100));
    }

    // It names itself, the way an agent would with `klark msg name`.
    let named = env.msg(Some("work"), Some(&pane), &["name", "rev"]);
    assert!(named.status.success(), "{}", text(&named));
    assert_eq!(
        String::from_utf8_lossy(&named.stdout).trim(),
        "testbox/work/rev"
    );

    let ls = env.msg(None, None, &["ls", "--local", "--json"]);
    let agents: Value = serde_json::from_slice(&ls.stdout).unwrap();
    assert_eq!(agents[0]["session"], "work");
    assert_eq!(agents[0]["name"], "rev");
    assert_eq!(agents[0]["pane_id"], json!(pane));
    assert_eq!(agents[0]["address"], "testbox/work/rev");

    // Send from the default session.
    let sent = env.msg(
        None,
        None,
        &["send", "work/rev", "please", "review", "PR-12", "--json"],
    );
    assert!(sent.status.success(), "{}", text(&sent));
    assert!(String::from_utf8_lossy(&sent.stdout).contains("delivered to testbox/work/rev"));

    // `cat` echoes what it was prompted with.
    wait_for(
        "the prompt to appear in the pane",
        Duration::from_secs(10),
        || {
            let read = call(
                &work,
                "pane.read",
                json!({"pane_id": pane, "source": "recent", "lines": 50}),
            );
            let t = read["result"]["read"]["text"]
                .as_str()
                .unwrap_or("")
                .to_string();
            t.contains("[klark msg from testbox/default/cli] please review PR-12 --json")
                .then_some(())
        },
    );

    // Both sides logged; the receiver knows whom to answer.
    let log = env.msg(None, None, &["log", "--json"]);
    let lines: Vec<Value> = String::from_utf8_lossy(&log.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert!(lines
        .iter()
        .any(|l| l["dir"] == "received" && l["ok"] == true && l["to"] == "testbox/work/rev"));
    assert!(lines
        .iter()
        .any(|l| l["dir"] == "sent" && l["to"] == "testbox/work/rev"));
    // The sender was a plain CLI, not an agent, so the reply has nowhere to go
    // and says so, naming the recorded sender's agent.
    let reply = env.msg(Some("work"), Some(&pane), &["reply", "done"]);
    assert!(!reply.status.success());
    assert!(
        String::from_utf8_lossy(&reply.stderr).contains("no agent cli"),
        "{}",
        text(&reply)
    );

    // Unknown and badly formed targets fail clearly.
    let ghost = env.msg(None, None, &["send", "work/ghost", "hi"]);
    assert!(!ghost.status.success());
    assert!(String::from_utf8_lossy(&ghost.stderr).contains("no agent ghost in session work"));
    let remote = env.msg(None, None, &["send", "otherbox/work/rev", "hi"]);
    assert!(
        String::from_utf8_lossy(&remote.stderr).contains("no hub configured"),
        "{}",
        text(&remote)
    );
}

/// The shell line that starts the stand-in agent: `cat` on macOS, `perl -pe ''` on Linux.
fn fake_command(fake: &Path) -> String {
    if cfg!(target_os = "macos") {
        fake.display().to_string()
    } else {
        format!("{} -pe ''", fake.display())
    }
}
