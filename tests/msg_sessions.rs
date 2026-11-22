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
    let deadline = Instant::now() + Duration::from_secs(45);
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
        Duration::from_secs(30),
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
    // The sender was a plain CLI, not a pane, so the reply is saved in the
    // message log for it (klark msg send --wait-reply reads it there).
    let reply = env.msg(Some("work"), Some(&pane), &["reply", "done"]);
    assert!(reply.status.success(), "{}", text(&reply));
    assert!(text(&reply).contains("reply was saved"), "{}", text(&reply));

    // Broadcast: --all is this session only (default has no agents);
    // --all-sessions reaches the agent in `work`.
    let none = env.msg(None, None, &["send", "--all", "hello-mine"]);
    assert!(!none.status.success());
    assert!(
        String::from_utf8_lossy(&none.stderr).contains("no other agents in session default"),
        "{}",
        text(&none)
    );
    let all = env.msg(None, None, &["send", "--all-sessions", "hello-everyone"]);
    assert!(all.status.success(), "{}", text(&all));
    assert!(String::from_utf8_lossy(&all.stdout).contains("delivered to testbox/work/rev"));
    // Inside `work`, --all skips the sender's own pane.
    let mine = env.msg(
        Some("work"),
        Some(&pane),
        &["send", "--all", "only-me-here"],
    );
    assert!(!mine.status.success(), "{}", text(&mine));
    wait_for(
        "the broadcast to appear in the pane",
        Duration::from_secs(30),
        || {
            let read = call(
                &work,
                "pane.read",
                json!({"pane_id": pane, "source": "recent", "lines": 50}),
            );
            read["result"]["read"]["text"]
                .as_str()
                .unwrap_or("")
                .contains("hello-everyone")
                .then_some(())
        },
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

/// klark#25: a durable task. Sent with `--task`, answered only by its
/// addressee with `reply --task`, read back from SQLite with `klark task`.
#[test]
fn a_task_is_answered_by_its_addressee_and_read_back() {
    let mut env = Env::new();
    env.start_server(Some("work"));
    let work = env.socket("work");
    let bin_dir = env.base.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let fake = bin_dir.join("claude");
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
    wait_for("the fake agent", Duration::from_secs(45), || {
        call(&work, "agent.list", json!({}))["result"]["agents"]
            .as_array()
            .is_some_and(|a| a.iter().any(|a| a["pane_id"] == json!(pane)))
            .then_some(())
    });
    assert!(env
        .msg(Some("work"), Some(&pane), &["name", "rev"])
        .status
        .success());
    let task = |args: &[&str]| -> Output {
        env.cmd(None)
            .arg("task")
            .args(args)
            .output()
            .expect("klark task")
    };

    // Sent as a task: the id comes back, and the agent sees it with the hint.
    let sent = env.msg(
        None,
        None,
        &[
            "send",
            "--task",
            "new",
            "--json",
            "work/rev",
            "check the build",
        ],
    );
    assert!(sent.status.success(), "{}", text(&sent));
    let sent: Value = serde_json::from_slice(&sent.stdout).unwrap();
    let id = sent["task"].as_str().expect("task id").to_string();
    assert_eq!(id.len(), 36, "a UUID: {sent}");
    let seen = wait_for("the task prompt", Duration::from_secs(10), || {
        let read = call(
            &work,
            "pane.read",
            json!({"pane_id": pane, "source": "recent", "lines": 40}),
        );
        let t = read["result"]["read"]["text"]
            .as_str()
            .unwrap_or("")
            .to_string();
        t.contains(&format!("[task {id}] check the build"))
            .then_some(t)
    });
    assert!(
        seen.contains(&format!("klark msg reply --task {id}")),
        "{seen}"
    );
    let got: Value = serde_json::from_slice(&task(&["get", &id, "--json"]).stdout).unwrap();
    assert_eq!(got["status"], "running", "{got}");
    assert!(got["started"].as_i64().is_some(), "{got}");
    assert_eq!(got["to"], "testbox/work/rev");

    // Only the addressee may answer.
    let other = env.msg(
        Some("work"),
        Some("w99:p9"),
        &["reply", "--task", &id, "not mine"],
    );
    assert!(!other.status.success(), "{}", text(&other));
    assert!(
        text(&other).contains("only that agent may answer"),
        "{}",
        text(&other)
    );
    let ok = env.msg(
        Some("work"),
        Some(&pane),
        &["reply", "--task", &id, "build", "is", "green"],
    );
    assert!(ok.status.success(), "{}", text(&ok));
    let got: Value = serde_json::from_slice(&task(&["get", &id, "--json"]).stdout).unwrap();
    assert_eq!(got["status"], "done", "{got}");
    assert_eq!(got["source"], "agent");
    assert_eq!(got["result"], "build is green");
    assert_eq!(got["answered_by"], "testbox/work/rev");
    assert_eq!(got["truncated"], false);
    let waited = task(&["wait", &id, "--timeout", "1", "--json"]);
    assert!(waited.status.success(), "{}", text(&waited));

    // --wait-reply with --task waits for that task's own answer.
    let id2 = "t_wait_1".to_string();
    let config = env.config.clone();
    let runtime = env.runtime.clone();
    let waiter = thread::spawn(move || {
        Command::new(BIN)
            .env("XDG_CONFIG_HOME", config)
            .env("XDG_RUNTIME_DIR", runtime)
            .env("KLARK_MACHINE_NAME", "testbox")
            .env_remove("KLARK_SESSION")
            .env_remove("KLARK_PANE_ID")
            .env_remove("KLARK_SOCKET_PATH")
            .args([
                "msg",
                "send",
                "--task",
                "t_wait_1",
                "--wait-reply",
                "30",
                "work/rev",
                "and the tests?",
            ])
            .output()
            .unwrap()
    });
    wait_for("task 2 delivered", Duration::from_secs(15), || {
        let out = task(&["get", &id2, "--json"]);
        serde_json::from_slice::<Value>(&out.stdout)
            .ok()
            .filter(|v| v["status"] == "running")
    });
    assert!(env
        .msg(
            Some("work"),
            Some(&pane),
            &["reply", "--task", &id2, "412 passed"]
        )
        .status
        .success());
    let out = waiter.join().unwrap();
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "412 passed");

    // Unknown ids: exit 1 with a JSON error.
    let missing = task(&["get", "t_nope", "--json"]);
    assert_eq!(missing.status.code(), Some(1));
    let err: Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(err["code"], "task_not_found", "{err}");

    // Every change is a line in tasks.jsonl, for klark feed --follow.
    let events = ["klark", "klark-dev"]
        .iter()
        .find_map(|app| fs::read_to_string(env.config.join(app).join("tasks.jsonl")).ok())
        .expect("tasks.jsonl");
    assert!(
        events.contains(&format!("\"id\":\"{id}\"")) && events.contains("\"done\""),
        "{events}"
    );
    assert!(
        !events.contains("build is green"),
        "events never carry results"
    );

    // Created queued by a caller (dmcp), delivered later, finished by the
    // creator from the screen when the agent never answered; owners isolated.
    let made = task(&[
        "create",
        "--to",
        "work/rev",
        "--owner",
        "u@x",
        "--from",
        "dmcp:demo",
        "--correlation",
        "c1",
        "--json",
        "summarise",
        "the",
        "logs",
    ]);
    assert!(made.status.success(), "{}", text(&made));
    let made: Value = serde_json::from_slice(&made.stdout).unwrap();
    let id3 = made["id"].as_str().unwrap().to_string();
    assert_eq!(made["status"], "queued");
    let again = task(&[
        "create",
        "--to",
        "work/rev",
        "--owner",
        "u@x",
        "--from",
        "dmcp:demo",
        "--id",
        &id3,
        "--json",
        "summarise",
        "the",
        "logs",
    ]);
    let again: Value = serde_json::from_slice(&again.stdout).unwrap();
    assert_eq!(again["id"], made["id"], "idempotent create");
    let conflict = task(&[
        "create", "--to", "work/rev", "--owner", "u@x", "--id", &id3, "--json", "other",
    ]);
    assert_eq!(conflict.status.code(), Some(1));
    let conflict: Value = serde_json::from_slice(&conflict.stdout).unwrap();
    assert_eq!(conflict["code"], "task_conflict");
    let queued = task(&[
        "list",
        "--status",
        "queued,blocked",
        "--from",
        "dmcp:demo",
        "--json",
    ]);
    let queued: Value = serde_json::from_slice(&queued.stdout).unwrap();
    assert_eq!(queued.as_array().unwrap().len(), 1, "{queued}");
    let sent = env.msg(
        None,
        None,
        &[
            "send",
            "--task",
            &id3,
            "--owner",
            "u@x",
            "--from",
            "dmcp:demo",
            "--json",
            "work/rev",
            "summarise the logs",
        ],
    );
    assert!(sent.status.success(), "{}", text(&sent));
    let resend = env.msg(
        None,
        None,
        &[
            "send",
            "--task",
            &id3,
            "--owner",
            "u@x",
            "--from",
            "dmcp:demo",
            "--json",
            "work/rev",
            "summarise the logs",
        ],
    );
    let resend: Value = serde_json::from_slice(&resend.stdout).unwrap();
    assert_eq!(resend["already"], true, "not typed twice: {resend}");
    let hidden = task(&["get", &id3, "--owner", "intruder", "--json"]);
    assert_eq!(hidden.status.code(), Some(1));
    let not_creator = task(&[
        "update", &id3, "--status", "done", "--from", "someone", "--result", "x", "--json",
    ]);
    assert_eq!(not_creator.status.code(), Some(1));
    let done = task(&[
        "update",
        &id3,
        "--status",
        "done",
        "--from",
        "dmcp:demo",
        "--source",
        "screen",
        "--result",
        "3 errors in the logs",
        "--json",
    ]);
    assert!(done.status.success(), "{}", text(&done));
    let got: Value =
        serde_json::from_slice(&task(&["get", &id3, "--owner", "u@x", "--json"]).stdout).unwrap();
    assert_eq!(got["status"], "done");
    assert_eq!(got["source"], "screen");
    assert!(got.get("answered_by").is_none(), "{got}");
    let mine = task(&["events", "--owner", "u@x"]);
    let mine = String::from_utf8_lossy(&mine.stdout).to_string();
    assert!(
        mine.contains(&id3) && !mine.contains(&id),
        "owner-scoped events: {mine}"
    );

    // Tasks are local only for now.
    let remote = env.msg(
        None,
        None,
        &["send", "--task", "new", "elsewhere/work/rev", "x"],
    );
    assert!(!remote.status.success());
    assert!(
        text(&remote).contains("this machine only"),
        "{}",
        text(&remote)
    );
}
