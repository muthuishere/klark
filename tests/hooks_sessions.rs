//! Global start/stop hooks across sessions (issue #11): start hooks run once
//! per machine with the first server, keep running while any server runs, and
//! the stop hooks run after the last server stops.

#![cfg(unix)]

pub mod support;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_klark");

struct Env {
    base: PathBuf,
    config: PathBuf,
    runtime: PathBuf,
    children: Vec<Child>,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = self.cmd().args(["hooks", "stop"]).output();
        for c in &mut self.children {
            let _ = c.kill();
            let _ = c.wait();
        }
        support::cleanup_test_base(&self.base);
    }
}

impl Env {
    fn new(hooks: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let base = PathBuf::from(format!(
            "/tmp/kh-{}-{seq}-{}",
            std::process::id(),
            nanos % 1_000_000_000_000
        ));
        let config = base.join("config");
        let runtime = base.join("runtime");
        let hooks = hooks.replace("$BASE", &base.display().to_string());
        for app in ["klark", "klark-dev"] {
            fs::create_dir_all(config.join(app)).unwrap();
            fs::write(
                config.join(app).join("config.toml"),
                format!("onboarding = false\n{hooks}"),
            )
            .unwrap();
        }
        fs::create_dir_all(&runtime).unwrap();
        support::register_runtime_dir(&runtime);
        Env {
            base,
            config,
            runtime,
            children: Vec::new(),
        }
    }

    fn cmd(&self) -> Command {
        let mut c = Command::new(BIN);
        c.env("XDG_CONFIG_HOME", &self.config)
            .env("XDG_RUNTIME_DIR", &self.runtime)
            .env("XDG_DATA_HOME", self.base.join("data"))
            .env("HOME", &self.base)
            .env("SHELL", "/bin/sh")
            .env("KLARK_HOOKS_GRACE_MS", "1500")
            .env("KLARK_MACHINE_NAME", "testbox")
            .env_remove("KLARK_HUB_URL")
            .env_remove("KLARK_HUB_JWT")
            .env_remove("KLARK_HUB_TOKEN")
            .env_remove("KLARK_SOCKET_PATH")
            .env_remove("KLARK_PANE_ID")
            .env_remove("KLARK_SESSION");
        c
    }

    fn start_server(&mut self, session: &str) {
        let child = self
            .cmd()
            .env("KLARK_SESSION", session)
            .arg("server")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn klark server");
        support::register_spawned_klark_pid(Some(child.id()));
        self.children.push(child);
        self.wait_socket(session);
    }

    fn wait_socket(&self, session: &str) {
        wait_for(
            &format!("server {session}"),
            Duration::from_secs(10),
            || self.socket(session).is_some(),
        );
    }

    fn socket(&self, session: &str) -> Option<PathBuf> {
        ["klark", "klark-dev"]
            .iter()
            .map(|app| {
                self.config
                    .join(app)
                    .join("sessions")
                    .join(session)
                    .join("klark.sock")
            })
            .find(|s| UnixStream::connect(s).is_ok())
    }

    fn stop_server(&self, session: &str) {
        let out = self
            .cmd()
            .env("KLARK_SESSION", session)
            .args(["server", "stop"])
            .output()
            .unwrap();
        assert!(out.status.success(), "server stop {session}: {out:?}");
    }

    fn hooks(&self) -> Value {
        let out = self
            .cmd()
            .args(["hooks", "list", "--json"])
            .output()
            .unwrap();
        serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("hooks list --json: {e}: {out:?}"))
    }

    fn hook_state(&self, name: &str) -> String {
        self.hooks()["hooks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["name"] == name)
            .map(|h| h["state"].as_str().unwrap_or("").to_string())
            .unwrap_or_default()
    }

    fn lines(&self, file: &str) -> usize {
        fs::read_to_string(self.base.join(file))
            .map(|s| s.lines().count())
            .unwrap_or(0)
    }
}

fn wait_for(what: &str, within: Duration, mut cond: impl FnMut() -> bool) {
    let deadline = Instant::now() + within;
    while !cond() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(100));
    }
}

const HOOKS: &str = r#"
[[hooks.start]]
name = "svc"
run = "echo $$ >> $BASE/started; exec sleep 600"

[[hooks.stop]]
name = "flush"
run = "echo done >> $BASE/stopped"
"#;

#[test]
fn hooks_start_once_survive_one_session_and_stop_after_the_last() {
    let mut env = Env::new(HOOKS);
    env.start_server("a");
    wait_for("the start hook", Duration::from_secs(10), || {
        env.lines("started") == 1
    });
    assert_eq!(env.hook_state("svc"), "running");

    env.start_server("b");
    thread::sleep(Duration::from_secs(2));
    assert_eq!(
        env.lines("started"),
        1,
        "a second session must not start it again"
    );

    env.stop_server("a");
    // Longer than the grace: b still runs, so nothing stops.
    thread::sleep(Duration::from_secs(3));
    assert_eq!(env.hook_state("svc"), "running");
    assert_eq!(env.lines("stopped"), 0);
    assert_eq!(env.lines("started"), 1);

    env.stop_server("b");
    wait_for("the stop hook", Duration::from_secs(15), || {
        env.lines("stopped") == 1
    });
    wait_for("the supervisor to end", Duration::from_secs(10), || {
        env.hooks()["supervisor"].is_null()
    });
    let pid: i32 = fs::read_to_string(env.base.join("started"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // SAFETY: signal 0 only checks whether the process exists.
    assert_ne!(
        unsafe { libc::kill(pid, 0) },
        0,
        "the start hook must be gone after the last server stops"
    );
}

#[test]
fn hooks_restart_by_policy_and_stop_by_hand() {
    let env = Env::new(
        r#"
[[hooks.start]]
name = "flaky"
run = "echo run >> $BASE/runs; exit 1"

[[hooks.start]]
name = "steady"
run = "exec sleep 600"
"#,
    );
    let out = env.cmd().args(["hooks", "start"]).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    // The default policy restarts a failing hook (backoff 1 s, 2 s, ...).
    wait_for("restarts", Duration::from_secs(10), || {
        env.lines("runs") >= 3
    });
    wait_for("steady running", Duration::from_secs(5), || {
        env.hook_state("steady") == "running"
    });

    let out = env
        .cmd()
        .args(["hooks", "stop", "steady"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert_eq!(env.hook_state("steady"), "stopped");
    let out = env
        .cmd()
        .args(["hooks", "start", "steady"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    wait_for("steady again", Duration::from_secs(5), || {
        env.hook_state("steady") == "running"
    });

    let logs = env
        .cmd()
        .args(["hooks", "logs", "flaky", "-n", "2"])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&logs.stdout).contains("klark hooks: start flaky"));
}

/// The dmcp case: with restart = "on-failure" an exit 0 is a clean stop and is
/// not restarted; `restart` and `start` run it again; hooks get KLARK_ENV=1;
/// `hooks list --json` answers with or without a supervisor.
#[test]
fn on_failure_clean_exit_stays_exited_and_can_be_run_again() {
    let env = Env::new(
        r#"
[[hooks.start]]
name = "dmcp"
run = "echo $KLARK_ENV > $BASE/env; echo run >> $BASE/runs; exit 0"
restart = "on-failure"
"#,
    );
    let hook = |env: &Env| -> Value {
        env.hooks()["hooks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["name"] == "dmcp")
            .cloned()
            .expect("dmcp in hooks list --json")
    };

    // No supervisor yet: the list still answers, exit 0, supervisor null.
    let out = env
        .cmd()
        .args(["hooks", "list", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    assert!(env.hooks()["supervisor"].is_null());
    assert_eq!(hook(&env)["state"], "not running");

    let out = env.cmd().args(["hooks", "start"]).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    wait_for("the first run", Duration::from_secs(10), || {
        env.lines("runs") == 1 && hook(&env)["state"] == "exited"
    });
    let h = hook(&env);
    assert_eq!(h["last_exit"], 0, "{h}");
    assert_eq!(h["restart"], "on-failure", "{h}");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let exited_at = h["exited_at"].as_u64().unwrap_or_else(|| panic!("{h}"));
    assert!(
        now.abs_diff(exited_at) < 60,
        "exited_at {exited_at}, now {now}"
    );
    let table = env.cmd().args(["hooks", "list"]).output().unwrap();
    let table = String::from_utf8_lossy(&table.stdout);
    assert!(
        table.contains("RESTART") && table.contains("EXITED"),
        "{table}"
    );
    assert!(table.contains("on-failure"), "{table}");
    assert!(table.contains(" ago"), "{table}");
    // A clean exit under on-failure is still announced, in the log a
    // `klark feed add <agent> ... --hook dmcp` follows.
    let logs = env.cmd().args(["hooks", "logs", "dmcp"]).output().unwrap();
    let logs = String::from_utf8_lossy(&logs.stdout);
    assert!(
        logs.contains("klark hook dmcp: exited (code 0) at ")
            && logs.contains("restart policy on-failure; not restarting")
            && logs.contains("Logs: klark hooks logs dmcp"),
        "{logs}"
    );
    assert_eq!(
        fs::read_to_string(env.base.join("env")).unwrap().trim(),
        "1",
        "hooks get KLARK_ENV=1"
    );
    // Longer than the first backoff: a clean exit is not restarted.
    thread::sleep(Duration::from_secs(3));
    assert_eq!(env.lines("runs"), 1, "exit 0 must not be restarted");

    for (verb, runs) in [("restart", 2), ("start", 3)] {
        let out = env.cmd().args(["hooks", verb, "dmcp"]).output().unwrap();
        assert!(out.status.success(), "hooks {verb} dmcp: {out:?}");
        wait_for(
            &format!("the run after {verb}"),
            Duration::from_secs(10),
            || env.lines("runs") == runs && hook(&env)["state"] == "exited",
        );
    }
    thread::sleep(Duration::from_secs(2));
    assert_eq!(env.lines("runs"), 3);
}

/// `notify = [...]` reaches an agent the way `klark msg send` does: a hook
/// under on-failure that exits 0 (the 2026-11-17 dmcp incident) is reported
/// to the agent in another session.
#[test]
fn notify_tells_an_agent_when_a_hook_exits_cleanly() {
    let mut env = Env::new(
        r#"
[[hooks.start]]
name = "gate"
run = "while [ ! -f $BASE/go ]; do sleep 0.2; done; exit 0"
restart = "on-failure"
notify = ["work/rev"]
"#,
    );
    env.start_server("work");
    let work = env.socket("work").expect("work socket");

    // A stand-in agent (see tests/msg_sessions.rs): a process named `claude`.
    let bin_dir = env.base.join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let fake = bin_dir.join("claude");
    if cfg!(target_os = "macos") {
        std::os::unix::fs::symlink("/bin/cat", &fake).unwrap();
    } else {
        fs::copy("/usr/bin/perl", &fake).unwrap();
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
    });
    let named = env
        .cmd()
        .env("KLARK_SESSION", "work")
        .env("KLARK_PANE_ID", &pane)
        .args(["msg", "name", "rev"])
        .output()
        .unwrap();
    assert!(named.status.success(), "{named:?}");
    wait_for("gate running", Duration::from_secs(10), || {
        env.hook_state("gate") == "running"
    });

    fs::write(env.base.join("go"), "").unwrap();
    let read = || {
        call(
            &work,
            "pane.read",
            json!({"pane_id": pane, "source": "recent", "lines": 50}),
        )["result"]["read"]["text"]
            .as_str()
            .unwrap_or("")
            .to_string()
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let text = read();
        if text.contains("[klark hook gate] klark hook gate: exited (code 0)") {
            assert!(!text.contains("klark msg reply"), "no reply hint:\n{text}");
            assert!(
                !text.contains("[klark msg from"),
                "not a msg frame:\n{text}"
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the notice never reached the agent\npane:\n{text}\nsupervisor.log:\n{}",
            ["klark", "klark-dev"]
                .iter()
                .filter_map(|app| fs::read_to_string(
                    env.config.join(app).join("hooks").join("supervisor.log")
                )
                .ok())
                .collect::<String>()
        );
        thread::sleep(Duration::from_millis(200));
    }
    assert_eq!(env.hook_state("gate"), "exited");

    // A hook is not an agent: the notice records no sender to reply to, so a
    // reply from that pane cannot go anywhere (not to a session named `hooks`).
    let reply = env
        .cmd()
        .env("KLARK_SESSION", "work")
        .env("KLARK_PANE_ID", &pane)
        .args(["msg", "reply", "thanks"])
        .output()
        .unwrap();
    assert!(!reply.status.success(), "{reply:?}");
    assert!(
        String::from_utf8_lossy(&reply.stderr).contains("nobody has messaged this pane yet"),
        "{reply:?}"
    );
    // Logged once delivery is confirmed, a few seconds after the text shows.
    wait_for(
        "the notice in klark msg log",
        Duration::from_secs(20),
        || {
            let log = env.cmd().args(["msg", "log", "--json"]).output().unwrap();
            String::from_utf8_lossy(&log.stdout).lines().any(|l| {
                l.contains("\"from\":\"klark-hook:gate\"") && l.contains("\"dir\":\"received\"")
            })
        },
    );
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

/// The shell line that starts the stand-in agent: `cat` on macOS, `perl -pe ''` on Linux.
fn fake_command(fake: &Path) -> String {
    if cfg!(target_os = "macos") {
        fake.display().to_string()
    } else {
        format!("{} -pe ''", fake.display())
    }
}

/// `klark hooks restart <name>` runs a running hook as config.toml says now,
/// so a changed definition (a new `notify` list among others) takes effect.
#[test]
fn restart_applies_the_current_definition() {
    let hooks = |word: &str| {
        format!(
            "[[hooks.start]]\nname = \"v\"\nrun = \"echo {word} >> $BASE/ver; exec sleep 600\"\n"
        )
    };
    let env = Env::new(&hooks("one"));
    let out = env.cmd().args(["hooks", "start"]).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    let ver = || fs::read_to_string(env.base.join("ver")).unwrap_or_default();
    wait_for("the first run", Duration::from_secs(10), || {
        ver().contains("one") && env.hook_state("v") == "running"
    });
    let changed = hooks("two").replace("$BASE", &env.base.display().to_string());
    for app in ["klark", "klark-dev"] {
        fs::write(
            env.config.join(app).join("config.toml"),
            format!("onboarding = false\n{changed}"),
        )
        .unwrap();
    }
    let out = env.cmd().args(["hooks", "restart", "v"]).output().unwrap();
    assert!(out.status.success(), "{out:?}");
    wait_for("the run after restart", Duration::from_secs(10), || {
        ver().contains("two")
    });
}
