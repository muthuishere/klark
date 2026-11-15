//! `klark feed` end to end on Windows (ADR 0015): a headless server reached
//! over its named pipe, a stand-in agent, and every source mode.
//!
//! The agent is a small echo program named `claude.exe` (built here with
//! `rustc`, so the test needs nothing beyond the toolchain that runs it):
//! klark detects it as an agent, and every message the feed types into its
//! pane shows up in the pane's text. `cmd.exe` would do, but it would also
//! run `<`, `>` and `&` in the messages as redirections.

#![cfg(windows)]

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use interprocess::os::windows::named_pipe::{pipe_mode::Bytes, DuplexPipeStream};
use interprocess::ConnectWaitMode;
use serde_json::{json, Value};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

const BIN: &str = env!("CARGO_BIN_EXE_klark");
const APP: &str = if cfg!(debug_assertions) {
    "klark-dev"
} else {
    "klark"
};

struct Env {
    base: PathBuf,
    server: Option<Child>,
}

impl Drop for Env {
    fn drop(&mut self) {
        // Detached feed workers are not our children: end them first.
        let _ = self.cmd().args(["feed", "stop", "--all"]).output();
        self.stop_server();
        let _ = fs::remove_dir_all(&self.base);
    }
}

impl Env {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let base = std::env::temp_dir().join(format!(
            "kfw-{tag}-{}-{}",
            std::process::id(),
            nanos % 1_000_000_000
        ));
        for app in ["klark", "klark-dev"] {
            fs::create_dir_all(base.join(app)).unwrap();
            fs::write(base.join(app).join("config.toml"), "onboarding = false\n").unwrap();
        }
        Env { base, server: None }
    }

    fn cmd(&self) -> Command {
        // KLARK_TEST_BIN: run a prebuilt klark (a test binary built elsewhere).
        let bin = std::env::var("KLARK_TEST_BIN").unwrap_or_else(|_| BIN.into());
        let mut c = Command::new(bin);
        c.env("XDG_CONFIG_HOME", &self.base)
            .env("XDG_STATE_HOME", &self.base)
            .env("XDG_DATA_HOME", self.base.join("data"))
            .env("XDG_RUNTIME_DIR", self.base.join("runtime"))
            .env("KLARK_MACHINE_NAME", "testbox")
            .env("KLARK_FEED_TEST", "1")
            .current_dir(&self.base)
            .creation_flags(CREATE_NO_WINDOW);
        for name in [
            "KLARK_SOCKET_PATH",
            "KLARK_CLIENT_SOCKET_PATH",
            "KLARK_CONFIG_PATH",
            "KLARK_PANE_ID",
            "KLARK_SESSION",
            "KLARK_HUB_URL",
            "KLARK_HUB_JWT",
            "KLARK_HUB_TOKEN",
        ] {
            c.env_remove(name);
        }
        c
    }

    fn start_server(&mut self) {
        let child = self
            .cmd()
            .args(["--session", "work", "server"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn klark server");
        self.server = Some(child);
        wait_for("the server's pipe", Duration::from_secs(30), || {
            connect(&self.socket()).ok().map(|_| ())
        });
    }

    fn stop_server(&mut self) {
        if let Some(mut c) = self.server.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    fn socket(&self) -> PathBuf {
        self.base
            .join(APP)
            .join("sessions")
            .join("work")
            .join("klark.sock")
    }

    fn call(&self, method: &str, params: Value) -> Value {
        let mut s = connect(&self.socket()).expect("connect to the server's pipe");
        let mut line = json!({"id": "t", "method": method, "params": params.clone()}).to_string();
        line.push('\n');
        // A server that never answers must fail the test, not hang the job.
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let r = s.write_all(line.as_bytes()).and_then(|_| {
                let mut out = String::new();
                BufReader::new(s).read_line(&mut out).map(|_| out)
            });
            let _ = tx.send(r);
        });
        let out = match rx.recv_timeout(Duration::from_secs(20)) {
            Ok(r) => r.unwrap_or_else(|e| panic!("{method}: {e}")),
            Err(_) => panic!("{method}: no answer from the server in 20 s ({params})"),
        };
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{method}: {e}: {out}"))
    }

    /// A stand-in agent named `rev`; returns its pane id.
    fn fake_agent(&self) -> String {
        let fake = echo_agent();
        let created = self.call("workspace.create", json!({"cwd": self.base, "focus": true}));
        let pane = created["result"]["root_pane"]["pane_id"]
            .as_str()
            .unwrap_or_else(|| panic!("workspace.create: {created}"))
            .to_string();
        // Let the pane's shell come up before typing into it.
        thread::sleep(Duration::from_secs(2));
        self.call(
            "pane.send_text",
            // Unquoted: PowerShell treats a quoted path as a string, not a
            // command. The temp path has no spaces (an assert below says so).
            json!({"pane_id": pane, "text": fake.display().to_string()}),
        );
        self.call(
            "pane.send_keys",
            json!({"pane_id": pane, "keys": ["Enter"]}),
        );
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let list = self.call("agent.list", json!({}));
            if list["result"]["agents"]
                .as_array()
                .is_some_and(|a| a.iter().any(|a| a["pane_id"] == json!(pane)))
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the fake agent was never detected.\nagent.list: {list}\npane:\n{}",
                self.pane_text(&pane)
            );
            thread::sleep(Duration::from_millis(200));
        }
        let named = self
            .cmd()
            .env("KLARK_SESSION", "work")
            .env("KLARK_PANE_ID", &pane)
            .args(["msg", "name", "rev"])
            .output()
            .unwrap();
        assert!(named.status.success(), "{}", text(&named));
        pane
    }

    fn pane_text(&self, pane: &str) -> String {
        let read = self.call(
            "pane.read",
            json!({"pane_id": pane, "source": "recent", "lines": 400}),
        );
        read["result"]["read"]["text"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }

    fn feed(&self, args: &[&str]) -> Output {
        self.cmd()
            .env("KLARK_SESSION", "work")
            .arg("feed")
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn feed_ok(&self, args: &[&str]) -> String {
        let o = self.feed(args);
        assert!(o.status.success(), "feed {args:?}: {}", text(&o));
        String::from_utf8_lossy(&o.stdout).into_owned()
    }

    fn list(&self) -> Vec<Value> {
        serde_json::from_str(&self.feed_ok(&["list", "--json"])).unwrap()
    }

    fn feed_state(&self, id: &str) -> Option<(bool, String)> {
        self.list()
            .into_iter()
            .find(|f| f["id"] == json!(id))
            .map(|f| {
                (
                    f["running"].as_bool().unwrap_or(false),
                    f["state"].as_str().unwrap_or("").to_string(),
                )
            })
    }

    fn wait_seen(&self, pane: &str, marker: &str, secs: u64) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let t = self.pane_text(pane);
            if t.contains(marker) {
                return;
            }
            if Instant::now() >= deadline {
                let logs = self
                    .list()
                    .iter()
                    .filter_map(|f| f["id"].as_str().map(str::to_string))
                    .map(|id| format!("--- {id}\n{}", text(&self.feed(&["logs", &id]))))
                    .collect::<Vec<_>>()
                    .join("\n");
                panic!("{marker} never reached the agent.\npane:\n{t}\nfeeds:\n{logs}");
            }
            thread::sleep(Duration::from_millis(200));
        }
    }
}

fn connect(path: &Path) -> std::io::Result<DuplexPipeStream<Bytes>> {
    let name = format!(r"\\.\pipe\{}", path.display());
    DuplexPipeStream::<Bytes>::connect_by_path_with_wait_mode(
        name.as_str(),
        ConnectWaitMode::Timeout(Duration::from_millis(500)),
    )
}

/// `claude.exe`: prints every line it reads, prefixed, so delivery shows.
fn echo_agent() -> PathBuf {
    static AGENT: OnceLock<PathBuf> = OnceLock::new();
    AGENT
        .get_or_init(|| {
            // KLARK_TEST_AGENT: a prebuilt claude.exe, for machines with no linker.
            if let Ok(p) = std::env::var("KLARK_TEST_AGENT") {
                return PathBuf::from(p);
            }
            let dir = std::env::temp_dir().join(format!("kfw-agent-{}", std::process::id()));
            fs::create_dir_all(&dir).unwrap();
            let src = dir.join("echo_agent.rs");
            fs::write(
                &src,
                "use std::io::BufRead;\n\
                 fn main() {\n\
                     println!(\"echo agent ready\");\n\
                     for line in std::io::stdin().lock().lines() {\n\
                         let Ok(line) = line else { break };\n\
                         println!(\"got: {line}\");\n\
                     }\n\
                 }\n",
            )
            .unwrap();
            let exe = dir.join("claude.exe");
            assert!(
                !exe.to_string_lossy().contains(' '),
                "the agent path is typed unquoted: {}",
                exe.display()
            );
            let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
            let out = Command::new(rustc)
                .arg(&src)
                .arg("-o")
                .arg(&exe)
                .output()
                .expect("rustc for the stand-in agent");
            assert!(out.status.success(), "rustc: {}", text(&out));
            exe
        })
        .clone()
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

fn append(path: &Path, line: &str) {
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(f, "{line}").unwrap();
}

fn unique(tag: &str) -> String {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros()
        % 1_000_000_000;
    format!("{tag}{n}")
}

/// --follow: an appended line arrives; after truncation, and after the file
/// is replaced (rotation), new lines still arrive. Unicode survives.
#[test]
fn follow_delivers_appends_truncation_and_rotation() {
    let mut env = Env::new("follow");
    env.start_server();
    let pane = env.fake_agent();
    let log = env.base.join("app log.txt"); // a space in the path on purpose
    fs::write(&log, "old line, not sent\n").unwrap();
    let log_s = log.to_string_lossy().to_string();
    env.feed_ok(&[
        "add",
        "work/rev",
        "Look at new log lines.",
        "--follow",
        &log_s,
        "--message-gap",
        "1",
        "--name",
        "flw",
    ]);
    thread::sleep(Duration::from_secs(2));

    let m1 = unique("FOLLOWAPPEND");
    append(&log, &format!("{m1} héllo ✓"));
    env.wait_seen(&pane, &m1, 30);
    let t = env.pane_text(&pane);
    assert!(t.contains("Look at new log lines."), "{t}");
    assert!(t.contains("héllo"), "unicode: {t}");
    assert!(!t.contains("old line, not sent"), "{t}");

    // Truncation: the file starts over.
    fs::write(&log, "").unwrap();
    thread::sleep(Duration::from_secs(2));
    let m2 = unique("FOLLOWTRUNC");
    append(&log, &m2);
    env.wait_seen(&pane, &m2, 30);

    // Rotation: renamed away, a new file in its place.
    fs::rename(&log, env.base.join("app log.txt.1")).unwrap();
    thread::sleep(Duration::from_secs(1));
    let m3 = unique("FOLLOWROTATE");
    fs::write(&log, format!("{m3}\n")).unwrap();
    env.wait_seen(&pane, &m3, 30);

    env.feed_ok(&["stop", "flw"]);
    assert!(env.feed_state("flw").is_none(), "{:?}", env.list());
}

/// --stream: a command that keeps printing; its lines arrive. Stopping the
/// feed ends the command's whole tree (it stops writing its tick file).
#[test]
fn stream_delivers_and_stop_ends_the_tree() {
    let mut env = Env::new("stream");
    env.start_server();
    let pane = env.fake_agent();
    let ticks = env.base.join("ticks.txt");
    let cmd = format!(
        "for /L %i in (1,1,10000) do @(echo STREAMTICK-%i & echo %i>>\"{}\" & ping -n 2 127.0.0.1 >NUL)",
        ticks.display()
    );
    env.feed_ok(&[
        "add",
        "work/rev",
        "Watch the ticks.",
        "--stream",
        &cmd,
        "--message-gap",
        "1",
        "--name",
        "strm",
    ]);
    env.wait_seen(&pane, "STREAMTICK-", 30);
    let (running, _) = env.feed_state("strm").expect("listed");
    assert!(running, "{:?}", env.list());

    env.feed_ok(&["stop", "strm"]);
    assert!(env.feed_state("strm").is_none(), "{:?}", env.list());
    thread::sleep(Duration::from_secs(2));
    let before = fs::read_to_string(&ticks).unwrap_or_default();
    thread::sleep(Duration::from_secs(5));
    let after = fs::read_to_string(&ticks).unwrap_or_default();
    assert_eq!(before, after, "the command still runs after stop");
}

/// --poll: only what changed is sent; --filter drops a batch it rejects.
#[test]
fn poll_sends_changes_and_filter_drops() {
    let mut env = Env::new("poll");
    env.start_server();
    let pane = env.fake_agent();
    let state = env.base.join("state.txt");
    fs::write(&state, "steady\n").unwrap();
    let poll = format!("type \"{}\"", state.display());
    // The filter rejects any batch with DROPME in it (findstr exits 0 on a
    // match, so the filter's exit is inverted by printing false).
    let filter = "findstr DROPME >NUL && (echo false) || (echo true)";
    env.feed_ok(&[
        "add",
        "work/rev",
        "Report state changes.",
        "--poll",
        &poll,
        "--interval",
        "1",
        "--filter",
        filter,
        "--name",
        "pll",
    ]);
    thread::sleep(Duration::from_secs(3));

    let m1 = unique("POLLCHANGE");
    fs::write(&state, format!("steady\n{m1}\n")).unwrap();
    env.wait_seen(&pane, &m1, 30);

    let dropped = unique("DROPME");
    fs::write(&state, format!("steady\n{m1}\n{dropped}\n")).unwrap();
    thread::sleep(Duration::from_secs(8));
    let m2 = unique("POLLAFTER");
    fs::write(&state, format!("steady\n{m1}\n{dropped}\n{m2}\n")).unwrap();
    env.wait_seen(&pane, &m2, 30);
    let t = env.pane_text(&pane);
    assert!(!t.contains(&dropped), "the filter should drop it: {t}");

    env.feed_ok(&["stop", "pll"]);
}

/// A feed survives the server going away: after a restart the server
/// resumes it and new lines arrive again.
#[test]
fn feed_resumes_after_server_restart() {
    let mut env = Env::new("resume");
    env.start_server();
    let pane = env.fake_agent();
    let log = env.base.join("r.log");
    fs::write(&log, "").unwrap();
    let log_s = log.to_string_lossy().to_string();
    env.feed_ok(&[
        "add",
        "work/rev",
        "Watch.",
        "--follow",
        &log_s,
        "--message-gap",
        "1",
        "--name",
        "rsm",
    ]);
    thread::sleep(Duration::from_secs(2));
    let m1 = unique("BEFORERESTART");
    append(&log, &m1);
    env.wait_seen(&pane, &m1, 30);

    env.stop_server();
    // The worker notices the server is gone and ends.
    wait_for("the worker to end", Duration::from_secs(30), || {
        env.feed_state("rsm")
            .is_some_and(|(running, _)| !running)
            .then_some(())
    });

    env.start_server();
    let pane2 = env.fake_agent();
    wait_for("the feed to resume", Duration::from_secs(60), || {
        env.feed_state("rsm")
            .is_some_and(|(running, _)| running)
            .then_some(())
    });
    thread::sleep(Duration::from_secs(2));
    let m2 = unique("AFTERRESTART");
    append(&log, &m2);
    env.wait_seen(&pane2, &m2, 60);
    let _ = pane;
    env.feed_ok(&["stop", "rsm"]);
}
