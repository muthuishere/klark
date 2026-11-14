//! Global start/stop hooks across sessions (issue #11): start hooks run once
//! per machine with the first server, keep running while any server runs, and
//! the stop hooks run after the last server stops.

#![cfg(unix)]

pub mod support;

use std::fs;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

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
            || {
                ["klark", "klark-dev"].iter().any(|app| {
                    UnixStream::connect(
                        self.config
                            .join(app)
                            .join("sessions")
                            .join(session)
                            .join("klark.sock"),
                    )
                    .is_ok()
                })
            },
        );
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
