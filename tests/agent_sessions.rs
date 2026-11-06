//! `klark agent` across klark sessions on one machine.
//!
//! Three headless servers share one config dir: `default`, `work` and `ops`.
//! `work` and `ops` each run a stand-in agent (a symlink to `cat` named
//! `claude`, detected as an agent by process name). From the default session:
//! `agent list -a` sees every session, `agent read|prompt work/rev` reach the
//! other session, a bare name found in exactly one other session is used, and
//! a bare name found in two is an error naming both full addresses.

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
            "/tmp/klark-agent-sess-{}-{nanos}",
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

    /// `klark <args>` from the default session.
    fn klark(&self, args: &[&str]) -> Output {
        self.cmd(None).args(args).output().expect("run klark")
    }

    /// Start a stand-in agent in a new workspace of `socket` and name it.
    fn fake_agent(&self, socket: &Path, name: &str) -> String {
        let fake = self.base.join("bin").join("claude");
        if !fake.exists() {
            fs::create_dir_all(fake.parent().unwrap()).unwrap();
            // A symlink, not a copy: macOS kills a copied platform binary.
            std::os::unix::fs::symlink("/bin/cat", &fake).unwrap();
        }
        let created = call(
            socket,
            "workspace.create",
            json!({"cwd": self.base, "focus": true}),
        );
        let pane = created["result"]["root_pane"]["pane_id"]
            .as_str()
            .unwrap_or_else(|| panic!("workspace.create: {created}"))
            .to_string();
        call(
            socket,
            "pane.send_text",
            json!({"pane_id": pane, "text": fake.display().to_string()}),
        );
        call(
            socket,
            "pane.send_keys",
            json!({"pane_id": pane, "keys": ["Enter"]}),
        );
        wait_for("fake agent detected", Duration::from_secs(15), || {
            let list = call(socket, "agent.list", json!({}));
            list["result"]["agents"]
                .as_array()
                .is_some_and(|a| a.iter().any(|a| a["pane_id"] == json!(pane)))
                .then_some(())
        });
        let renamed = call(
            socket,
            "agent.rename",
            json!({"target": pane, "name": name}),
        );
        assert!(renamed.get("error").is_none(), "{renamed}");
        pane
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

fn stdout_json(o: &Output) -> Value {
    assert!(o.status.success(), "{}", text(o));
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("{e}: {}", text(o)))
}

fn stderr_json(o: &Output) -> Value {
    assert!(!o.status.success(), "{}", text(o));
    serde_json::from_slice(&o.stderr).unwrap_or_else(|e| panic!("{e}: {}", text(o)))
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
fn agent_commands_reach_agents_in_other_sessions() {
    let mut env = Env::new();
    env.start_server(None);
    env.start_server(Some("work"));
    let _default = env.socket("default");
    let work = env.socket("work");
    let work_pane = env.fake_agent(&work, "rev");
    let coder_pane = env.fake_agent(&work, "coder");

    // Default session alone: unchanged shape, plus session/address fields.
    let here = stdout_json(&env.klark(&["agent", "list"]));
    assert_eq!(here["result"]["agents"], json!([]), "{here}");

    // -a: every running session, each agent carrying its session and address.
    let all = stdout_json(&env.klark(&["agent", "list", "-a"]));
    assert_eq!(
        all["result"]["sessions"],
        json!(["default", "work"]),
        "{all}"
    );
    let agents = all["result"]["agents"].as_array().unwrap();
    let rev = agents
        .iter()
        .find(|a| a["name"] == "rev")
        .unwrap_or_else(|| panic!("{all}"));
    assert_eq!(rev["session"], "work");
    assert_eq!(rev["address"], "testbox/work/rev");
    assert_eq!(rev["pane_id"], json!(work_pane));
    let long = stdout_json(&env.klark(&["agent", "list", "--all-sessions"]));
    assert_eq!(long["result"]["agents"].as_array().unwrap().len(), 2);

    // session/agent and machine/session/agent targets reach `work`.
    let got = stdout_json(&env.klark(&["agent", "get", "work/rev"]));
    assert_eq!(got["result"]["agent"]["pane_id"], json!(work_pane));
    let got = stdout_json(&env.klark(&["agent", "get", "testbox/work/coder"]));
    assert_eq!(got["result"]["agent"]["pane_id"], json!(coder_pane));

    let prompted = env.klark(&["agent", "prompt", "work/rev", "hello-from-default"]);
    assert!(prompted.status.success(), "{}", text(&prompted));
    wait_for("prompt echoed in work/rev", Duration::from_secs(10), || {
        let read = env.klark(&["agent", "read", "work/rev", "--lines", "50"]);
        (read.status.success()
            && String::from_utf8_lossy(&read.stdout).contains("hello-from-default"))
        .then_some(())
    });

    // A bare name the current session lacks is found in the one session that has it.
    let bare = stdout_json(&env.klark(&["agent", "get", "coder"]));
    assert_eq!(bare["result"]["agent"]["pane_id"], json!(coder_pane));

    // Clear failures.
    let ghost = stderr_json(&env.klark(&["agent", "get", "ghost"]));
    assert_eq!(ghost["error"]["code"], "agent_not_found");
    let message = ghost["error"]["message"].as_str().unwrap();
    assert!(message.contains("klark agent list -a"), "{message}");
    let stopped = stderr_json(&env.klark(&["agent", "get", "nosuch/rev"]));
    assert_eq!(stopped["error"]["code"], "session_not_running");
    let other = stderr_json(&env.klark(&["agent", "read", "otherbox/work/rev"]));
    assert_eq!(other["error"]["code"], "invalid_agent_target");
    assert!(other["error"]["message"]
        .as_str()
        .unwrap()
        .contains("klark msg send otherbox/work/rev"));

    // `session list` counts agents per running session.
    let sessions = stdout_json(&env.klark(&["session", "list", "--json"]));
    let work_row = sessions["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "work")
        .unwrap();
    assert_eq!(work_row["agents"], 2, "{sessions}");

    // A second session with the same name makes the bare name ambiguous.
    env.start_server(Some("ops"));
    let ops = env.socket("ops");
    env.fake_agent(&ops, "rev");
    let amb = stderr_json(&env.klark(&["agent", "get", "rev"]));
    assert_eq!(amb["error"]["code"], "agent_target_ambiguous");
    let message = amb["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("testbox/work/rev") && message.contains("testbox/ops/rev"),
        "{message}"
    );
    // Inside `work`, the bare name still means work's own agent, as before.
    let inside = env
        .cmd(Some("work"))
        .args(["agent", "get", "rev"])
        .output()
        .unwrap();
    assert_eq!(
        stdout_json(&inside)["result"]["agent"]["pane_id"],
        json!(work_pane)
    );
}
