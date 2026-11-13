//! `klark feed` against a headless server and a stand-in agent (ADR 0015).
//!
//! The agent is a copy of `cat` named `claude` (as in `msg_sessions.rs`), so
//! klark detects it as an agent and whatever it is prompted with shows up in
//! its pane. `KLARK_FEED_TEST=1` lets the tests use gaps below 5 seconds.

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
    children: Vec<Child>,
}

impl Drop for Env {
    fn drop(&mut self) {
        // Detached feed workers are not our children: end them first.
        let _ = self.cmd().args(["feed", "stop", "--all"]).output();
        for c in &mut self.children {
            let _ = c.kill();
            let _ = c.wait();
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
        // macOS clocks tick in microseconds: tests starting together would
        // share a base (and one's cleanup would remove the other's).
        static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // Kept short: a pane line naming a path must not wrap.
        let base = PathBuf::from(format!(
            "/tmp/kf-{}-{seq}-{}",
            std::process::id(),
            nanos % 1_000_000_000_000
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
            children: Vec::new(),
        }
    }

    fn cmd(&self) -> Command {
        let mut c = Command::new(BIN);
        c.env("XDG_CONFIG_HOME", &self.config)
            .env("XDG_RUNTIME_DIR", &self.runtime)
            .env("XDG_DATA_HOME", self.base.join("data"))
            .env("SHELL", "/bin/sh")
            .env("KLARK_MACHINE_NAME", "testbox")
            .env("KLARK_FEED_TEST", "1")
            .env_remove("KLARK_SOCKET_PATH")
            .env_remove("KLARK_PANE_ID")
            .env_remove("KLARK_SESSION")
            .env_remove("KLARK_HUB_URL")
            .env_remove("KLARK_HUB_JWT")
            .env_remove("KLARK_HUB_TOKEN");
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
    }

    fn socket(&self, session: &str) -> PathBuf {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            for app in ["klark", "klark-dev"] {
                let sock = self
                    .config
                    .join(app)
                    .join("sessions")
                    .join(session)
                    .join("klark.sock");
                if UnixStream::connect(&sock).is_ok() {
                    return sock;
                }
            }
            assert!(
                Instant::now() < deadline,
                "socket for {session} never appeared"
            );
            thread::sleep(Duration::from_millis(50));
        }
    }

    /// A stand-in agent named `rev` in `session`; returns its pane id.
    fn fake_agent(&self, session: &str) -> String {
        self.fake_agent_named(session, "rev")
    }

    /// A stand-in agent named `name` in `session`; returns its pane id.
    fn fake_agent_named(&self, session: &str, name: &str) -> String {
        let sock = self.socket(session);
        let bin_dir = self.base.join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let fake = bin_dir.join("claude");
        if !fake.exists() {
            if cfg!(target_os = "macos") {
                std::os::unix::fs::symlink("/bin/cat", &fake).unwrap();
            } else {
                fs::copy("/usr/bin/perl", &fake).unwrap();
            }
        }
        let created = call(
            &sock,
            "workspace.create",
            json!({"cwd": self.base, "focus": true}),
        );
        let pane = created["result"]["root_pane"]["pane_id"]
            .as_str()
            .unwrap_or_else(|| panic!("workspace.create: {created}"))
            .to_string();
        let line = if cfg!(target_os = "macos") {
            fake.display().to_string()
        } else {
            format!("{} -pe ''", fake.display())
        };
        call(
            &sock,
            "pane.send_text",
            json!({"pane_id": pane, "text": line}),
        );
        call(
            &sock,
            "pane.send_keys",
            json!({"pane_id": pane, "keys": ["Enter"]}),
        );
        wait_for(
            "the fake agent to be detected",
            Duration::from_secs(45),
            || {
                let list = call(&sock, "agent.list", json!({}));
                list["result"]["agents"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|a| a["pane_id"] == json!(pane)))
                    .then_some(())
            },
        );
        let named = self
            .cmd()
            .env("KLARK_SESSION", session)
            .env("KLARK_PANE_ID", &pane)
            .args(["msg", "name", name])
            .output()
            .unwrap();
        assert!(named.status.success(), "{}", text(&named));
        pane
    }

    fn feed(&self, args: &[&str]) -> Command {
        let mut c = self.cmd();
        c.current_dir(&self.base).arg("feed").args(args);
        c
    }

    fn pane_text(&self, session: &str, pane: &str) -> String {
        let read = call(
            &self.socket(session),
            "pane.read",
            json!({"pane_id": pane, "source": "recent", "lines": 200}),
        );
        read["result"]["read"]["text"]
            .as_str()
            .unwrap_or("")
            .to_string()
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

fn wait_exit(child: &mut Child, timeout: Duration) -> std::process::ExitStatus {
    wait_for("the feed to exit", timeout, || child.try_wait().unwrap())
}

fn process_running(marker: &str) -> bool {
    Command::new("pgrep")
        .args(["-f", &format!("^{marker}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// I1 + I5: a lone line arrives quickly; a burst arrives whole, as one
/// message, no sooner than the gap; the end of the command is reported with
/// its exit code and stderr; the feed exits by itself.
#[test]
fn stream_delivers_new_lines_and_reports_the_end() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");

    let script = "echo 'ERROR one'; sleep 4; printf 'burst-a\\nburst-b\\nburst-c\\n'; sleep 4; echo boom-on-stderr >&2; exit 3";
    let started = Instant::now();
    let mut feed = env
        .feed(&[
            "add",
            "work/rev",
            "Diagnose any error.",
            "--stream",
            script,
            "--message-gap",
            "3",
            "--label",
            "app-log",
            "--foreground",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    wait_for("the first line", Duration::from_secs(15), || {
        let t = env.pane_text("work", &pane);
        t.contains("ERROR one").then_some(())
    });
    let first = started.elapsed();
    assert!(first < Duration::from_secs(8), "lone line took {first:?}");
    let t = env.pane_text("work", &pane);
    assert!(t.contains("app-log] Diagnose any error."), "{t}");
    assert!(t.contains("untrusted data"), "{t}");
    assert!(t.contains("klark feed leave"), "{t}");
    assert!(
        !t.contains("sleep 4"),
        "the command line must not be in the message: {t}"
    );

    wait_for("the burst", Duration::from_secs(20), || {
        env.pane_text("work", &pane)
            .contains("burst-c")
            .then_some(())
    });
    let t = env.pane_text("work", &pane);
    if !t.contains("3 new lines") {
        let _ = feed.kill();
        let mut err = String::new();
        std::io::Read::read_to_string(feed.stderr.as_mut().unwrap(), &mut err).unwrap();
        panic!("a burst is one message: {t}\nfeed stderr:\n{err}");
    }

    wait_for("the end notice", Duration::from_secs(30), || {
        env.pane_text("work", &pane)
            .contains("exited with code 3")
            .then_some(())
    });
    let t = env.pane_text("work", &pane);
    assert!(t.contains("boom-on-stderr"), "{t}");

    let status = wait_exit(&mut feed, Duration::from_secs(15));
    let mut err = String::new();
    std::io::Read::read_to_string(feed.stderr.as_mut().unwrap(), &mut err).unwrap();
    eprintln!("feed stderr:\n{err}");
    assert!(status.success(), "{status:?} {err}");
}

/// I5: a stream that exits 0 is still reported (`kubectl logs -f` does that).
#[test]
fn stream_exit_zero_is_reported() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    let out = env
        .feed(&[
            "add",
            "rev",
            "Watch.",
            "--stream",
            "sleep 2; echo last-line",
            "--label",
            "pods",
            "--foreground",
        ])
        .env("KLARK_SESSION", "work")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    let t = env.pane_text("work", &pane);
    assert!(t.contains("last-line"), "{t}");
    assert!(t.contains("ended: pods exited normally."), "{t}");
}

/// I19: a command that does not exist fails on the terminal; nothing is sent.
#[test]
fn bad_command_fails_on_the_terminal() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    let out = env
        .feed(&[
            "add",
            "work/rev",
            "Watch.",
            "--stream",
            "no-such-command-kf42",
            "--foreground",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success(), "{}", text(&out));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("failed at once"), "{err}");
    thread::sleep(Duration::from_secs(2));
    assert!(!env.pane_text("work", &pane).contains("klark feed"));
}

/// I9 (foreground part): stopping the feed kills the whole pipeline.
#[test]
fn ctrl_c_kills_the_pipeline() {
    let mut env = Env::new();
    env.start_server("work");
    env.fake_agent("work");
    let marker = format!("sleep 91{}", std::process::id() % 1000);
    let mut feed = env
        .feed(&[
            "add",
            "work/rev",
            "Watch.",
            "--stream",
            &format!("{marker} | cat"),
            "--foreground",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_for("the pipeline to start", Duration::from_secs(10), || {
        process_running(&marker).then_some(())
    });
    // SAFETY: plain kill(2) on our own child.
    unsafe { libc::kill(feed.id() as i32, libc::SIGINT) };
    let status = wait_exit(&mut feed, Duration::from_secs(10));
    assert!(status.success(), "{status:?}");
    wait_for("the pipeline to be gone", Duration::from_secs(5), || {
        (!process_running(&marker)).then_some(())
    });
}

/// I24 (part) and argument errors reach the user as usage errors.
#[test]
fn refusals() {
    let mut env = Env::new();
    env.start_server("work");
    env.fake_agent("work");
    let remote = env
        .feed(&[
            "add",
            "otherbox/work/rev",
            "x",
            "--stream",
            "true",
            "--foreground",
        ])
        .output()
        .unwrap();
    assert!(!remote.status.success());
    assert!(
        text(&remote).contains("local agents only"),
        "{}",
        text(&remote)
    );
    let mixed = env
        .feed(&[
            "add",
            "work/rev",
            "x",
            "--stream",
            "true",
            "--interval",
            "30",
        ])
        .output()
        .unwrap();
    assert_eq!(mixed.status.code(), Some(2), "{}", text(&mixed));
}

// ---------------------------------------------------------------- M3: background feeds

impl Env {
    fn feed_out(&self, args: &[&str]) -> Output {
        self.feed(args).output().unwrap()
    }

    /// Runs a feed verb that must succeed; returns stdout + stderr.
    fn feed_ok(&self, args: &[&str]) -> String {
        let out = self.feed_out(args);
        assert!(out.status.success(), "feed {args:?}: {}", text(&out));
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    }

    /// A feed verb run from inside a pane (an agent pane when one runs there).
    fn feed_in_pane(&self, session: &str, pane: &str, args: &[&str]) -> Output {
        self.feed(args)
            .env("KLARK_SESSION", session)
            .env("KLARK_PANE_ID", pane)
            .output()
            .unwrap()
    }

    fn list(&self) -> Vec<Value> {
        let out = self.feed_ok(&["list", "--json"]);
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("{e}: {out}"))
    }

    fn listed(&self, id: &str) -> Option<Value> {
        self.list().into_iter().find(|f| f["id"] == json!(id))
    }

    fn logs(&self, id: &str) -> String {
        let out = self.feed_out(&["logs", id]);
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    /// The worker's "sent" lines: one per delivered message.
    fn sends(&self, id: &str) -> Vec<String> {
        self.logs(id)
            .lines()
            .filter(|l| l.contains(": sent "))
            .map(str::to_string)
            .collect()
    }

    fn wait_text(&self, session: &str, pane: &str, needle: &str, secs: u64) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let last = self.pane_text(session, pane);
            if last.contains(needle) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {needle:?} in the pane:\n{last}"
            );
            thread::sleep(Duration::from_millis(150));
        }
    }

    fn wait_sends(&self, id: &str, n: usize, secs: u64) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let s = self.sends(id);
            if s.len() >= n {
                return s;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {n} deliveries of {id}:\n{}",
                self.logs(id)
            );
            thread::sleep(Duration::from_millis(150));
        }
    }

    fn store_dir(&self) -> PathBuf {
        for app in ["klark", "klark-dev"] {
            let d = self.config.join(app).join("feeds");
            if d.exists() {
                return d;
            }
        }
        panic!("no feed store under {}", self.config.display());
    }

    fn status(&self, id: &str) -> Value {
        let raw = fs::read_to_string(self.store_dir().join(format!("{id}.status.json"))).unwrap();
        serde_json::from_str(&raw).unwrap()
    }

    fn state(&self, id: &str) -> String {
        self.listed(id)
            .map(|f| f["state"].as_str().unwrap_or("").to_string())
            .unwrap_or_else(|| "<gone>".into())
    }

    fn wait_state(&self, id: &str, prefix: &str, secs: u64) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        loop {
            let s = self.state(id);
            if s.starts_with(prefix) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "{id}: state {s:?}, wanted {prefix:?}\n{}",
                self.logs(id)
            );
            thread::sleep(Duration::from_millis(150));
        }
    }

    fn stop_server(&mut self, session: &str) {
        let sock = self.socket(session);
        let _ = self
            .cmd()
            .env("KLARK_SESSION", session)
            .args(["server", "stop"])
            .output();
        wait_for("the server to stop", Duration::from_secs(10), || {
            UnixStream::connect(&sock).is_err().then_some(())
        });
    }
}

fn append(path: &Path, s: &str) {
    use std::fs::OpenOptions;
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap()
        .write_all(s.as_bytes())
        .unwrap();
}

/// How many processes have a command line starting with `marker`.
fn procs(marker: &str) -> usize {
    Command::new("pgrep")
        .args(["-f", &format!("^{marker}")])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().count())
        .unwrap_or(0)
}

/// A process marker unique to this test run: `sleep <digits>`.
fn marker(test: u32) -> String {
    format!("sleep {test}{:05}", std::process::id() % 100_000)
}

fn wait_procs(marker: &str, n: usize, secs: u64) {
    wait_for(
        &format!("{n} process(es) {marker}"),
        Duration::from_secs(secs),
        || (procs(marker) == n).then_some(()),
    );
}

/// I2: a poll feed sends what is new: the first run all of it, the same
/// output nothing, one new line only that line, a second identical line too.
#[test]
fn poll_sends_only_new_lines() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    let list = env.base.join("list.txt");
    fs::write(&list, "item-a\nitem-b\n").unwrap();
    env.feed_ok(&[
        "add",
        "work/rev",
        "New items.",
        "--poll",
        "cat list.txt",
        "--interval",
        "1",
        "--name",
        "p2",
    ]);
    env.wait_text("work", &pane, "item-b", 15);
    thread::sleep(Duration::from_secs(4));
    assert_eq!(env.sends("p2").len(), 1, "same output twice is sent once");

    fs::write(&list, "item-a\nitem-new\nitem-b\n").unwrap();
    env.wait_text("work", &pane, "item-new", 15);
    let s = env.wait_sends("p2", 2, 5);
    assert!(s[1].contains("(1 line(s)"), "{s:?}");

    fs::write(&list, "item-a\nitem-new\nitem-b\nitem-a\n").unwrap();
    let s = env.wait_sends("p2", 3, 15);
    assert!(
        s[2].contains("(1 line(s)"),
        "a second identical line is sent: {s:?}"
    );
    thread::sleep(Duration::from_secs(3));
    assert_eq!(env.sends("p2").len(), 3);
}

/// I3: a `--diff` command gets the previous and current output; what it
/// prints is what arrives; silence sends nothing; a failing script is
/// reported and the previous file is kept.
#[test]
fn poll_diff_command() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    let data = env.base.join("v.txt");
    fs::write(&data, "v1\n").unwrap();
    fs::write(
        env.base.join("d.sh"),
        "grep -q BREAK \"$2\" && { echo broken >&2; exit 3; }\n\
         [ \"$KLARK_FEED_PREV\" = \"$1\" ] || echo env-mismatch\n\
         cmp -s \"$1\" \"$2\" || echo \"CHANGED $(tail -n1 \"$1\")->$(tail -n1 \"$2\")\"\n",
    )
    .unwrap();
    env.feed_ok(&[
        "add",
        "work/rev",
        "Diffed.",
        "--poll",
        "cat v.txt",
        "--interval",
        "1",
        "--diff",
        "sh d.sh",
        "--name",
        "p3",
    ]);
    env.wait_text("work", &pane, "CHANGED ->v1", 15);
    thread::sleep(Duration::from_secs(3));
    assert_eq!(env.sends("p3").len(), 1, "no output sends nothing");

    fs::write(&data, "v2\n").unwrap();
    env.wait_text("work", &pane, "CHANGED v1->v2", 15);
    fs::write(&data, "BREAK\n").unwrap();
    env.wait_text("work", &pane, "--diff failed", 15);
    env.wait_text("work", &pane, "broken", 5);
    fs::write(&data, "v3\n").unwrap();
    env.wait_text("work", &pane, "CHANGED v2->v3", 15);
    assert!(!env.pane_text("work", &pane).contains("env-mismatch"));
}

/// I4: a failing poll is reported once, a different exit code again, then
/// `recovered` without resending old items; a run past the timeout is killed.
#[test]
fn poll_errors_and_recovery() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    let mode = env.base.join("mode");
    fs::write(&mode, "ok\n").unwrap();
    fs::write(env.base.join("items"), "old-item\n").unwrap();
    fs::write(
        env.base.join("p.sh"),
        "case $(cat mode) in\n\
         ok) cat items ;;\n\
         fail3) echo three-broke >&2; exit 3 ;;\n\
         fail4) exit 4 ;;\n\
         hang) sleep 30 ;;\n\
         esac\n",
    )
    .unwrap();
    env.feed_ok(&[
        "add",
        "work/rev",
        "Watch.",
        "--poll",
        "sh p.sh",
        "--interval",
        "1",
        "--label",
        "inbox",
        "--name",
        "p4",
    ]);
    env.wait_text("work", &pane, "old-item", 15);

    fs::write(&mode, "fail3\n").unwrap();
    env.wait_text("work", &pane, "inbox failed with exit code 3", 15);
    env.wait_text("work", &pane, "three-broke", 5);
    thread::sleep(Duration::from_secs(4));
    assert_eq!(env.sends("p4").len(), 2, "reported once, not every run");

    fs::write(&mode, "fail4\n").unwrap();
    env.wait_text("work", &pane, "exit code 4", 15);

    fs::write(env.base.join("items"), "old-item\nfresh-item\n").unwrap();
    fs::write(&mode, "ok\n").unwrap();
    env.wait_text("work", &pane, "recovered: inbox", 15);
    env.wait_text("work", &pane, "fresh-item", 5);
    let s = env.wait_sends("p4", 4, 5);
    assert!(
        s.iter().any(|l| l.contains("(1 line(s)")),
        "only the new item after a failure: {s:?}"
    );

    fs::write(&mode, "hang\n").unwrap();
    env.wait_text("work", &pane, "killed after 5 s", 30);
}

/// I8: a second add of the same command joins (one process); two adds at
/// once make one feed; each agent has its own instruction; a joiner's flags
/// are ignored with a note; `leave` as printed unsubscribes only the caller;
/// the last one leaving ends the command and the feed.
#[test]
fn subscribers_join_and_leave() {
    let mut env = Env::new();
    env.start_server("work");
    let p1 = env.fake_agent_named("work", "rev");
    let p2 = env.fake_agent_named("work", "rev2");
    let m = marker(8);
    let cmd = format!("{m} | cat & tail -n0 -F s.log; wait");
    fs::write(env.base.join("s.log"), "").unwrap();
    env.feed_ok(&[
        "add",
        "work/rev",
        "instr-one",
        "--stream",
        &cmd,
        "--message-gap",
        "1",
        "--name",
        "j8",
    ]);
    let joined = env.feed_ok(&[
        "add",
        "work/rev2",
        "instr-two",
        "--stream",
        &cmd,
        "--message-gap",
        "1",
        "--max-bytes",
        "9999",
    ]);
    assert!(joined.contains("joined"), "{joined}");
    assert!(
        joined.contains("ignored") && joined.contains("--max-bytes"),
        "{joined}"
    );
    assert_eq!(env.list().len(), 1);
    wait_procs(&m, 1, 10);

    // The worker reads the subscriber list once a second.
    thread::sleep(Duration::from_millis(1500));
    append(&env.base.join("s.log"), "line-x\n");
    env.wait_text("work", &p1, "line-x", 15);
    env.wait_text("work", &p2, "line-x", 15);
    assert!(env.pane_text("work", &p1).contains("instr-one"));
    let t2 = env.pane_text("work", &p2);
    assert!(
        t2.contains("instr-two") && !t2.contains("instr-one"),
        "{t2}"
    );
    assert_eq!(procs(&m), 1, "one process for two agents");

    // Leave as printed in the message, from the agent's own pane.
    let out = env.feed_in_pane("work", &p1, &["leave", "j8"]);
    assert!(out.status.success(), "{}", text(&out));
    let agents = env.listed("j8").unwrap()["agents"].to_string();
    assert!(
        agents.contains("rev2") && !agents.contains("work/rev\""),
        "{agents}"
    );
    assert_eq!(procs(&m), 1);
    append(&env.base.join("s.log"), "line-y\n");
    env.wait_text("work", &p2, "line-y", 15);
    assert!(!env.pane_text("work", &p1).contains("line-y"));

    let out = env.feed_in_pane("work", &p2, &["leave", "j8"]);
    assert!(out.status.success(), "{}", text(&out));
    assert!(env.listed("j8").is_none());
    wait_procs(&m, 0, 10);

    // Two adds at the same moment: one feed.
    let m2 = marker(18);
    let cmd2 = m2.clone();
    let a = env
        .feed(&["add", "work/rev", "a", "--stream", &cmd2])
        .spawn()
        .unwrap();
    let b = env
        .feed(&["add", "work/rev2", "b", "--stream", &cmd2])
        .spawn()
        .unwrap();
    for mut c in [a, b] {
        assert!(c.wait().unwrap().success());
    }
    let feeds = env.list();
    assert_eq!(feeds.len(), 1, "{feeds:?}");
    assert_eq!(feeds[0]["agents"].as_array().unwrap().len(), 2);
    wait_procs(&m2, 1, 10);
}

/// I9: `stop` kills a pipeline, leaves no orphan, sends nothing, and the feed
/// does not come back after a server restart; `stop --all` covers every
/// session.
#[test]
fn stop_is_final() {
    let mut env = Env::new();
    env.start_server("work");
    env.start_server("play");
    env.fake_agent("work");
    let pp = env.fake_agent("play");
    let m1 = marker(9);
    let m2 = marker(19);
    let m3 = marker(29);
    let pipe1 = format!("{m1} | cat | cat");
    env.feed_ok(&["add", "work/rev", "w", "--stream", &pipe1, "--name", "s1"]);
    let pipe2 = format!("{m2} | cat");
    env.feed_ok(&["add", "work/rev", "w", "--stream", &pipe2, "--name", "s2"]);
    let pipe3 = format!("{m3} | cat");
    env.feed_ok(&["add", "play/rev", "p", "--stream", &pipe3, "--name", "s3"]);
    wait_procs(&m1, 1, 10);

    env.feed_ok(&["stop", "s1"]);
    wait_procs(&m1, 0, 5);
    assert!(env.listed("s1").is_none());
    assert!(!env.store_dir().join("s1.json").exists());

    env.stop_server("work");
    wait_procs(&m2, 0, 20);
    env.start_server("work");
    env.ensure_agent("work", "rev");
    // s2 comes back by itself (feed resume); s1 never does.
    wait_procs(&m2, 1, 60);
    assert!(
        env.listed("s1").is_none(),
        "a stopped feed never comes back"
    );
    assert_eq!(procs(&m1), 0);

    env.feed_ok(&["stop", "--all"]);
    wait_procs(&m2, 0, 10);
    wait_procs(&m3, 0, 10);
    assert!(env.list().is_empty());
    assert!(!env.pane_text("play", &pp).contains("klark feed"));
    let pw = env.ensure_agent("work", "rev");
    assert!(!env.pane_text("work", &pw).contains("klark feed"));
}

/// I11: a large batch arrives as one message with the newest lines; the
/// printed `klark feed batch` gives all of it; only the last 5 are kept.
#[test]
fn large_batches() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    let script = "for b in 1 2 3 4 5 6 7; do i=0; while [ $i -lt 60 ]; do echo \"b$b-line-$i\"; i=$((i+1)); done; sleep 4; done; sleep 1000";
    env.feed_ok(&[
        "add",
        "work/rev",
        "Big.",
        "--stream",
        script,
        "--message-gap",
        "1",
        "--max-bytes",
        "256",
        "--name",
        "big",
    ]);
    env.wait_text("work", &pane, "klark feed batch big 1", 20);
    let t = env.pane_text("work", &pane);
    assert!(t.contains("b1-line-59"), "{t}");
    assert!(
        !t.contains("b1-line-0\n") && !t.contains("b1-line-1\n"),
        "{t}"
    );
    let s = env.wait_sends("big", 7, 60);
    assert!(s.iter().all(|l| l.contains("(60 line(s)")), "{s:?}");
    let out = env.feed_ok(&["batch", "big", "7"]);
    assert_eq!(out.lines().count(), 60, "{out}");
    assert!(out.starts_with("b7-line-0\n"));
    assert!(!env.feed_out(&["batch", "big", "2"]).status.success());
    assert!(env.feed_out(&["batch", "big", "3"]).status.success());
}

/// I12 + I28 (user shell part): the rate limit stops the subscription with
/// the message; `feed start` runs it again; a second rate stop within 24 h
/// is refused from an agent pane and allowed from a user shell.
#[test]
fn rate_limit_stops_and_second_stop_is_user_only() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    let m = marker(12);
    let script = format!("{m} & while true; do echo tick; sleep 3; done");
    env.feed_ok(&[
        "add",
        "work/rev",
        "Ticks.",
        "--stream",
        &script,
        "--message-gap",
        "1",
        "--max-per-hour",
        "2",
        "--name",
        "r12",
    ]);
    env.wait_text("work", &pane, "too many updates (3 in the last hour)", 40);
    env.wait_state("r12", "stopped:", 10);
    wait_procs(&m, 0, 10);

    env.feed_ok(&["start", "r12"]);
    wait_procs(&m, 1, 10);
    env.wait_text("work", &pane, "the second time today", 40);
    env.wait_state("r12", "stopped:", 10);
    let refused = env.feed_in_pane("work", &pane, &["start", "r12"]);
    assert!(!refused.status.success(), "{}", text(&refused));
    assert!(
        text(&refused).contains("only the user"),
        "{}",
        text(&refused)
    );
    env.feed_ok(&["start", "r12"]);
    wait_procs(&m, 1, 10);
}

/// I12: a silent feed stops after `--idle-stop` and `feed start` runs it
/// again; error / recovered flapping is stopped by the limit.
#[test]
fn idle_stop_and_flapping() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    let m = marker(22);
    env.feed_ok(&[
        "add",
        "work/rev",
        "Quiet.",
        "--stream",
        &m,
        "--idle-stop",
        "3s",
        "--name",
        "i12",
    ]);
    env.wait_text("work", &pane, "nothing new for 3s", 20);
    env.wait_state("i12", "stopped: nothing new", 10);
    wait_procs(&m, 0, 10);
    env.feed_ok(&["start", "i12"]);
    wait_procs(&m, 1, 10);
    env.wait_state("i12", "running", 10);
    env.feed_ok(&["stop", "i12"]);

    fs::write(
        env.base.join("flap.sh"),
        "if [ -f flip ]; then rm flip; exit 5; else touch flip; echo fine; fi\n",
    )
    .unwrap();
    env.feed_ok(&[
        "add",
        "work/rev",
        "Flap.",
        "--poll",
        "sh flap.sh",
        "--interval",
        "1",
        "--max-per-hour",
        "3",
        "--name",
        "f12",
    ]);
    env.wait_text("work", &pane, "too many updates (4 in the last hour)", 60);
    env.wait_state("f12", "stopped:", 10);
}

/// I14: a followed file: appended lines arrive; a new feed starts at the end;
/// rotation and truncation are followed; a file that appears later is read.
#[test]
fn follow_a_file() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    let log = env.base.join("app.log");
    fs::write(&log, "old-line\n").unwrap();
    env.feed_ok(&[
        "add",
        "work/rev",
        "Log.",
        "--follow",
        "app.log",
        "--message-gap",
        "1",
        "--name",
        "fo",
    ]);
    append(&log, "new-1\n");
    env.wait_text("work", &pane, "new-1", 15);
    assert!(!env.pane_text("work", &pane).contains("old-line"));

    env.wait_sends("fo", 1, 10);
    fs::rename(&log, env.base.join("app.log.1")).unwrap();
    append(&log, "rot-2\n");
    env.wait_text("work", &pane, "rot-2", 15);

    env.wait_sends("fo", 2, 10);
    fs::write(&log, "").unwrap();
    thread::sleep(Duration::from_millis(1500));
    append(&log, "trunc-3\n");
    env.wait_text("work", &pane, "trunc-3", 15);

    env.feed_ok(&[
        "add",
        "work/rev",
        "Later.",
        "--follow",
        "later.log",
        "--message-gap",
        "1",
        "--name",
        "fl",
    ]);
    thread::sleep(Duration::from_secs(1));
    append(&env.base.join("later.log"), "late-1\n");
    env.wait_text("work", &pane, "late-1", 15);
}

/// I18: a worker killed with SIGKILL leaves its command; `feed start` reaps
/// it, so exactly one runs; SIGTERM to the worker takes the tree down.
#[test]
fn worker_death() {
    let mut env = Env::new();
    env.start_server("work");
    env.fake_agent("work");
    let m = marker(28);
    let pipe = format!("{m} | cat");
    env.feed_ok(&["add", "work/rev", "w", "--stream", &pipe, "--name", "w18"]);
    wait_procs(&m, 1, 10);
    let pid = env.status("w18")["worker_pid"].as_i64().unwrap() as i32;
    // SAFETY: kill(2) on the feed worker this test started.
    unsafe { libc::kill(pid, libc::SIGKILL) };
    thread::sleep(Duration::from_millis(500));
    assert_eq!(procs(&m), 1, "the command outlives a SIGKILLed worker");
    env.feed_ok(&["start", "w18"]);
    thread::sleep(Duration::from_secs(1));
    wait_procs(&m, 1, 10);

    let pid = env.status("w18")["worker_pid"].as_i64().unwrap() as i32;
    // SAFETY: as above.
    unsafe { libc::kill(pid, libc::SIGTERM) };
    wait_procs(&m, 0, 10);
}

/// I20: the filter drops on exit 1 or a printed false, sends on exit 0 or a
/// printed true; a crashing or hanging one lets the batch through and is
/// reported once; `feed list` counts the drops; a notice passes a filter that
/// always says false.
#[test]
fn filter_decides() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    fs::write(
        env.base.join("f.sh"),
        "in=$(cat)\n\
         case $in in\n\
         *KEEP*) exit 0 ;;\n\
         *DROPWORD*) echo false ;;\n\
         *SAYTRUE*) echo thinking; echo TRUE; exit 1 ;;\n\
         *CRASH*) exit 7 ;;\n\
         *HANG*) sleep 60 ;;\n\
         *) exit 1 ;;\n\
         esac\n",
    )
    .unwrap();
    let log = env.base.join("f.log");
    fs::write(&log, "").unwrap();
    env.feed_ok(&[
        "add",
        "work/rev",
        "Judge.",
        "--follow",
        "f.log",
        "--message-gap",
        "1",
        "--filter",
        "sh f.sh",
        "--name",
        "fi",
    ]);
    let dropped = |n: usize| {
        wait_for("a filtered batch", Duration::from_secs(15), || {
            (env.listed("fi").unwrap()["filtered"] == json!(n)).then_some(())
        })
    };
    append(&log, "KEEP-1\n");
    env.wait_text("work", &pane, "KEEP-1", 15);
    append(&log, "drop-2\n");
    dropped(1);
    append(&log, "DROPWORD-3\n");
    dropped(2);
    append(&log, "SAYTRUE-4\n");
    env.wait_text("work", &pane, "SAYTRUE-4", 15);
    append(&log, "CRASH-5\n");
    env.wait_text("work", &pane, "CRASH-5", 15);
    env.wait_text("work", &pane, "filter error", 5);
    append(&log, "CRASH-6\n");
    env.wait_text("work", &pane, "CRASH-6", 15);
    // KEEP-1, SAYTRUE-4, CRASH-5 (with the error notice), CRASH-6.
    let notices = env
        .wait_sends("fi", 4, 10)
        .iter()
        .filter(|l| l.contains("1 notice(s)"))
        .count();
    assert_eq!(notices, 1, "reported once");
    append(&log, "HANG-7\n");
    env.wait_text("work", &pane, "HANG-7", 20);
    let t = env.pane_text("work", &pane);
    assert!(!t.contains("drop-2") && !t.contains("DROPWORD-3"), "{t}");
    assert_eq!(env.listed("fi").unwrap()["filtered"], json!(2));

    env.feed_ok(&[
        "add",
        "work/rev",
        "Never.",
        "--stream",
        "sleep 1; echo nope-x; exit 2",
        "--message-gap",
        "1",
        "--filter",
        "echo false",
        "--name",
        "fn",
    ]);
    env.wait_text("work", &pane, "exited with code 2", 20);
    assert!(!env.pane_text("work", &pane).contains("nope-x"));
}

/// I21: a closed agent pane makes the subscription wait (the command stops);
/// the agent coming back resumes delivery.
#[test]
fn agent_away_and_back() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    let m = marker(21);
    let cmd = format!("{m} & tail -n0 -F a.log; wait");
    fs::write(env.base.join("a.log"), "").unwrap();
    env.feed_ok(&[
        "add",
        "work/rev",
        "Away.",
        "--stream",
        &cmd,
        "--message-gap",
        "1",
        "--name",
        "aw",
    ]);
    append(&env.base.join("a.log"), "a-1\n");
    env.wait_text("work", &pane, "a-1", 15);
    env.wait_sends("aw", 1, 10);

    call(&env.socket("work"), "pane.close", json!({"pane_id": pane}));
    wait_procs(&m, 0, 15);
    env.wait_state("aw", "waiting", 5);
    let f = env.listed("aw").unwrap();
    assert!(
        f["agents"].to_string().contains("waiting"),
        "{f}\n{}\n{}",
        env.logs("aw"),
        env.status("aw")
    );
    assert!(f["state"].as_str().unwrap().starts_with("waiting"), "{f}");

    let pane2 = env.fake_agent("work");
    wait_procs(&m, 1, 15);
    thread::sleep(Duration::from_millis(500));
    append(&env.base.join("a.log"), "a-3\n");
    env.wait_text("work", &pane2, "a-3", 15);
}

/// I23: `--diff unified` on a changed file; the first run sends nothing.
#[test]
fn poll_unified_diff() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    let conf = env.base.join("conf.txt");
    fs::write(&conf, "l1\nl2\nl3\nport 80\nl5\nl6\nl7\nl8\n").unwrap();
    env.feed_ok(&[
        "add",
        "work/rev",
        "Config.",
        "--poll",
        "cat conf.txt",
        "--diff",
        "unified",
        "--interval",
        "1",
        "--name",
        "u23",
    ]);
    thread::sleep(Duration::from_secs(3));
    assert!(env.sends("u23").is_empty(), "the first run sends nothing");
    fs::write(&conf, "l1\nl2\nl3\nport 8080\nl5\nl6\nl7\nl8\n").unwrap();
    env.wait_text("work", &pane, "+port 8080", 15);
    assert!(env.pane_text("work", &pane).contains("-port 80"));
}

/// I24: agent names stay unique in a session (the server refuses a second
/// `dup`, so a feed always finds its agent again; the worker's own check for
/// an ambiguous name is unit-tested); a `machine/agent` address is refused.
#[test]
fn agent_names_must_be_unique() {
    let mut env = Env::new();
    env.start_server("work");
    let first = env.fake_agent_named("work", "dup");
    let second = env.fake_agent_named("work", "other");
    let renamed = env
        .cmd()
        .env("KLARK_SESSION", "work")
        .env("KLARK_PANE_ID", &second)
        .args(["msg", "name", "dup"])
        .output()
        .unwrap();
    assert!(!renamed.status.success(), "{}", text(&renamed));
    env.feed_ok(&[
        "add",
        "work/dup",
        "x",
        "--stream",
        "sleep 100",
        "--name",
        "n24",
    ]);
    let s = env.store_dir().join("n24.json");
    let def: Value = serde_json::from_str(&fs::read_to_string(s).unwrap()).unwrap();
    assert_eq!(def["subs"][0]["agent"], json!("dup"), "stored by name");
    let _ = first;
    let out = env.feed_out(&["add", "otherbox/work/dup", "x", "--stream", "sleep 100"]);
    assert!(text(&out).contains("local agents only"), "{}", text(&out));
}

/// I25: `--stale`: one notice after the silent period, none again until
/// output resumed.
#[test]
fn stale_notice() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    fs::write(env.base.join("st.log"), "").unwrap();
    env.feed_ok(&[
        "add",
        "work/rev",
        "Stale.",
        "--follow",
        "st.log",
        "--message-gap",
        "1",
        "--stale",
        "2",
        "--label",
        "stlog",
        "--name",
        "st",
    ]);
    env.wait_text(
        "work",
        &pane,
        "stale: stlog has printed nothing for 2 s",
        15,
    );
    thread::sleep(Duration::from_secs(4));
    assert_eq!(env.sends("st").len(), 1, "{}", env.logs("st"));
    append(&env.base.join("st.log"), "alive\n");
    env.wait_text("work", &pane, "alive", 15);
    let s = env.wait_sends("st", 3, 15);
    assert!(s[2].contains("1 notice(s)"), "{s:?}");
}

/// I26 + I27: `list` and `show` are right, the command (with its token) is in
/// neither a message nor `messages.jsonl`, and a feed is not a reply target.
#[test]
fn list_show_and_not_msg() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    let cmd = "echo hello-26; sleep 1000 # token=sekret-26";
    env.feed_ok(&[
        "add",
        "work/rev",
        "Show me.",
        "--stream",
        cmd,
        "--message-gap",
        "2",
        "--max-bytes",
        "2048",
        "--name",
        "ls26",
    ]);
    env.wait_text("work", &pane, "hello-26", 15);
    env.wait_sends("ls26", 1, 10);
    let f = env.listed("ls26").unwrap();
    assert_eq!(f["mode"], json!("stream"));
    assert_eq!(f["label"], json!("echo"));
    assert_eq!(f["state"], json!("running"));
    assert_eq!(f["created_by"], json!("user"));
    assert_eq!(f["agents"], json!(["work/rev"]));
    assert!(f["last_output"].as_u64().is_some(), "{f}");
    wait_for("last sent", Duration::from_secs(5), || {
        env.listed("ls26").unwrap()["last_sent"].as_u64()
    });
    let table = env.feed_ok(&["list"]);
    assert!(
        table.contains("CREATED BY") && table.contains("ls26"),
        "{table}"
    );
    let show = env.feed_ok(&["show", "ls26"]);
    for want in [
        "sekret-26",
        "message gap",
        "2 s",
        "max bytes          2048",
        "Show me.",
    ] {
        assert!(show.contains(want), "{want} in {show}");
    }
    assert!(!env.pane_text("work", &pane).contains("sekret-26"));

    // An agent pane is recorded as the creator.
    let out = env.feed_in_pane(
        "work",
        &pane,
        &[
            "add",
            "rev",
            "Mine.",
            "--stream",
            "sleep 1001",
            "--name",
            "ag26",
        ],
    );
    assert!(out.status.success(), "{}", text(&out));
    assert_eq!(
        env.listed("ag26").unwrap()["created_by"],
        json!("agent work/rev")
    );

    // I27: not `klark msg`.
    let found = Command::new("grep")
        .args(["-rl", "hello-26", "--include=messages.jsonl"])
        .arg(&env.base)
        .output()
        .unwrap();
    assert!(
        found.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&found.stdout)
    );
    let reply = env
        .cmd()
        .env("KLARK_SESSION", "work")
        .env("KLARK_PANE_ID", &pane)
        .args(["msg", "reply", "hi"])
        .output()
        .unwrap();
    assert!(!reply.status.success(), "{}", text(&reply));
    assert!(
        text(&reply).contains("nobody has messaged"),
        "{}",
        text(&reply)
    );
}

/// I28: `feed start` from an agent pane restarts only that agent's
/// subscription; from a user shell the feed and all stopped subscriptions.
#[test]
fn start_scope() {
    let mut env = Env::new();
    env.start_server("work");
    let p1 = env.fake_agent_named("work", "rev");
    let p2 = env.fake_agent_named("work", "rev2");
    let script = "while true; do echo tick; sleep 2; done";
    env.feed_ok(&[
        "add",
        "work/rev",
        "One.",
        "--stream",
        script,
        "--message-gap",
        "1",
        "--max-per-hour",
        "1",
        "--name",
        "sc",
    ]);
    env.feed_ok(&["agent", "add", "sc", "work/rev2", "Two."]);
    env.wait_text("work", &p1, "too many updates", 30);
    env.wait_text("work", &p2, "too many updates", 30);
    env.wait_state("sc", "stopped:", 15);

    let out = env.feed_in_pane("work", &p1, &["start", "sc"]);
    assert!(out.status.success(), "{}", text(&out));
    env.wait_state("sc", "running", 10);
    let agents = env.listed("sc").unwrap()["agents"].clone();
    assert_eq!(agents[0], json!("work/rev"), "{agents}");
    assert!(agents[1].as_str().unwrap().contains("stopped"), "{agents}");

    env.feed_ok(&["start", "sc"]);
    let agents = env.listed("sc").unwrap()["agents"].clone();
    assert_eq!(agents, json!(["work/rev", "work/rev2"]));

    env.feed_ok(&["agent", "remove", "sc", "work/rev2"]);
    assert_eq!(env.listed("sc").unwrap()["agents"], json!(["work/rev"]));
}

/// I18 (poll): a worker sent SIGTERM while a poll run is in flight takes that
/// run down too; it is not left running with no worker.
#[test]
fn worker_sigterm_kills_a_running_poll() {
    let mut env = Env::new();
    env.start_server("work");
    env.fake_agent("work");
    let m = marker(38);
    fs::write(
        env.base.join("pp.sh"),
        format!("if [ -f once ]; then {m}; else touch once; echo first; fi\n"),
    )
    .unwrap();
    env.feed_ok(&[
        "add",
        "work/rev",
        "w",
        "--poll",
        "sh pp.sh",
        "--interval",
        "2",
        "--name",
        "pk",
    ]);
    wait_procs(&m, 1, 10);
    let pid = env.status("pk")["worker_pid"].as_i64().unwrap() as i32;
    // SAFETY: kill(2) on the feed worker this test started.
    unsafe { libc::kill(pid, libc::SIGTERM) };
    // The run's own timeout is 10 s; it must go with the worker, not then.
    wait_procs(&m, 0, 4);
}

/// I12 (poll): a poll feed that stops itself (idle stop) while a run is in
/// flight does not leave that run behind.
#[test]
fn self_stop_kills_a_running_poll() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    let m = marker(39);
    fs::write(
        env.base.join("pi.sh"),
        format!("if [ -f once ]; then {m}; else touch once; echo first; fi\n"),
    )
    .unwrap();
    env.feed_ok(&[
        "add",
        "work/rev",
        "w",
        "--poll",
        "sh pi.sh",
        "--interval",
        "2",
        "--idle-stop",
        "3s",
        "--name",
        "pi",
    ]);
    wait_procs(&m, 1, 10);
    env.wait_text("work", &pane, "nothing new for 3s", 20);
    env.wait_state("pi", "stopped:", 10);
    wait_procs(&m, 0, 4);
}

// ---------------------------------------------------------------- M4: restart and settings

impl Env {
    /// A server with extra environment variables (the "server environment" a
    /// resumed feed must see).
    fn start_server_with(&mut self, session: &str, vars: &[(&str, &str)]) {
        let mut c = self.cmd();
        c.env("KLARK_SESSION", session);
        for (k, v) in vars {
            c.env(k, v);
        }
        let child = c
            .arg("server")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn klark server");
        support::register_spawned_klark_pid(Some(child.id()));
        self.children.push(child);
    }

    /// The agent named `name` in `session`, started again when the restored
    /// session does not have it running.
    fn ensure_agent(&self, session: &str, name: &str) -> String {
        let list = call(&self.socket(session), "agent.list", json!({}));
        let found = list["result"]["agents"].as_array().and_then(|a| {
            a.iter()
                .find(|a| a["name"] == json!(name))
                .and_then(|a| a["pane_id"].as_str().map(str::to_string))
        });
        found.unwrap_or_else(|| self.fake_agent_named(session, name))
    }

    fn write_config(&self, body: &str) {
        for app in ["klark", "klark-dev"] {
            fs::write(
                self.config.join(app).join("config.toml"),
                format!("onboarding = false\n{body}"),
            )
            .unwrap();
        }
    }

    fn config_text(&self) -> String {
        ["klark", "klark-dev"]
            .iter()
            .map(|app| {
                fs::read_to_string(self.config.join(app).join("config.toml")).unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn workers(&self, id: &str) -> usize {
        Command::new("pgrep")
            .args(["-f", &format!("klark feed _worker {id}( |$)")])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).lines().count())
            .unwrap_or(0)
    }
}

/// I13: stopping the server kills the command (no orphan, no worker); starting
/// it again runs the feed without `feed add`, once, even with two sessions
/// starting together; with `restore` off the feed stays stopped.
#[test]
fn restart_resumes_feeds() {
    let mut env = Env::new();
    env.start_server("work");
    env.start_server("play");
    env.fake_agent("work");
    env.fake_agent("play");
    let m = marker(13);
    let pipe = format!("{m} | cat");
    env.feed_ok(&["add", "work/rev", "w", "--stream", &pipe, "--name", "r13"]);
    env.feed_ok(&["agent", "add", "r13", "play/rev", "p"]);
    let m2 = marker(23);
    let pipe2 = format!("{m2} | cat");
    env.feed_ok(&[
        "add",
        "work/rev",
        "w",
        "--stream",
        &pipe2,
        "--name",
        "n13",
        "--no-restore",
    ]);
    wait_procs(&m, 1, 10);
    wait_procs(&m2, 1, 10);

    env.stop_server("work");
    // One subscriber's server is still up: the feed keeps running.
    thread::sleep(Duration::from_secs(5));
    assert_eq!(procs(&m), 1, "play/rev still listens");
    env.stop_server("play");
    wait_procs(&m, 0, 20);
    wait_procs(&m2, 0, 20);
    wait_for("the workers to end", Duration::from_secs(20), || {
        (env.workers("r13") == 0 && env.workers("n13") == 0).then_some(())
    });
    assert!(!env.listed("r13").unwrap()["running"].as_bool().unwrap());

    // Both sessions at once: one feed, one command.
    env.start_server("work");
    env.start_server("play");
    env.ensure_agent("work", "rev");
    env.ensure_agent("play", "rev");
    wait_procs(&m, 1, 60);
    thread::sleep(Duration::from_secs(3));
    assert_eq!(procs(&m), 1, "resumed once");
    assert_eq!(env.workers("r13"), 1);
    env.wait_state("n13", "stopped: klark restarted", 15);
    assert_eq!(procs(&m2), 0);
    assert_eq!(env.workers("n13"), 0);

    env.feed_ok(&["start", "n13"]);
    wait_procs(&m2, 1, 15);
}

/// I15: lines appended to a followed file while klark was down arrive once;
/// after a long stop the catch-up is cut to the cap, with the marker.
#[test]
fn follow_across_a_restart() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    let log = env.base.join("f.log");
    fs::write(&log, "old\n").unwrap();
    env.feed_ok(&[
        "add",
        "work/rev",
        "Follow.",
        "--follow",
        "f.log",
        "--message-gap",
        "1",
        "--name",
        "f15",
    ]);
    thread::sleep(Duration::from_millis(1500));
    append(&log, "up-1\n");
    env.wait_text("work", &pane, "up-1", 15);
    // Delivered and recorded (a stop in between may send it again: delivery
    // is at least once).
    env.wait_sends("f15", 1, 10);

    env.stop_server("work");
    wait_for("the worker to end", Duration::from_secs(20), || {
        (env.workers("f15") == 0).then_some(())
    });
    append(&log, "down-1\ndown-2\n");
    env.start_server("work");
    let pane = env.ensure_agent("work", "rev");
    env.wait_text("work", &pane, "down-2", 60);
    thread::sleep(Duration::from_secs(3));
    let t = env.pane_text("work", &pane);
    assert!(!t.contains("up-1"), "nothing sent twice: {t}");
    let s = env.sends("f15");
    assert_eq!(s.len(), 2, "down-1 and down-2 in one message: {s:?}");
    assert!(s[1].contains("(2 line(s)"), "{s:?}");

    env.stop_server("work");
    wait_for("the worker to end", Duration::from_secs(20), || {
        (env.workers("f15") == 0).then_some(())
    });
    let big: String = (0..2000)
        .map(|i| format!("bulk-line-{i:05}-xxxxxxxxxxxxxxxxxxxxxxxx\n"))
        .collect();
    assert!(big.len() > 64 * 1024);
    append(&log, &big);
    append(&log, "after-bulk\n");
    env.start_server("work");
    let pane = env.ensure_agent("work", "rev");
    env.wait_text("work", &pane, "after-bulk", 60);
    let s = env.wait_sends("f15", 3, 10);
    assert!(s[2].contains("line(s)"), "{s:?}");
    // The marker heads a long message: read far enough back to see it.
    let read = call(
        &env.socket("work"),
        "pane.read",
        json!({"pane_id": pane, "source": "recent", "lines": 2000}),
    );
    let t = read["result"]["read"]["text"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(
        t.contains("bytes written while klark was not running were skipped"),
        "{t}"
    );
    let batch = env.feed_ok(&["batch", "f15", "1"]);
    assert!(batch.contains("bulk-line-01999"), "{batch}");
    assert!(!batch.contains("bulk-line-00000"), "cut to the cap");
}

/// I17: output queued for an agent survives a server restart and arrives
/// once.
#[test]
fn pending_survives_a_restart() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    fs::write(env.base.join("p.log"), "").unwrap();
    env.feed_ok(&[
        "add",
        "work/rev",
        "Pending.",
        "--stream",
        "tail -n0 -F p.log",
        "--message-gap",
        "300",
        "--name",
        "q17",
    ]);
    append(&env.base.join("p.log"), "first-line\n");
    env.wait_text("work", &pane, "first-line", 15);
    env.wait_sends("q17", 1, 10);
    // Within the 300 s gap: this one waits, on disk.
    append(&env.base.join("p.log"), "queued-line\n");
    wait_for("the line to be queued", Duration::from_secs(10), || {
        fs::read_to_string(env.store_dir().join("q17.0.pending"))
            .unwrap_or_default()
            .contains("queued-line")
            .then_some(())
    });
    assert!(!env.pane_text("work", &pane).contains("queued-line"));

    env.stop_server("work");
    wait_for("the worker to end", Duration::from_secs(20), || {
        (env.workers("q17") == 0).then_some(())
    });
    env.start_server("work");
    let pane = env.ensure_agent("work", "rev");
    env.wait_text("work", &pane, "queued-line", 60);
    thread::sleep(Duration::from_secs(4));
    let t = env.pane_text("work", &pane);
    assert!(!t.contains("first-line"), "{t}");
    let sends = env.sends("q17");
    assert_eq!(sends.len(), 2, "queued-line once: {sends:?}");
    assert!(sends[1].contains("(1 line(s)"), "{sends:?}");
}

/// I10 + I22: the first run sees the caller's variables, user and cwd;
/// after a restart the command sees what a new pane's shell sees
/// (server environment, `[terminal] default_shell` / `shell_mode`, the
/// profile of a login shell), in the same cwd; not the creator's variables.
#[test]
fn resumed_command_gets_a_new_panes_environment() {
    let mut env = Env::new();
    let home = env.base.join("home");
    fs::create_dir_all(&home).unwrap();
    fs::write(home.join(".bash_profile"), "export PROFILE_SEEN=yes\n").unwrap();
    env.write_config("[terminal]\ndefault_shell = \"/bin/bash\"\nshell_mode = \"login\"\n");
    let home_s = home.display().to_string();
    // A server started from inside another klark pane carries that pane's
    // ids; a resumed command must not (a new pane gets its own).
    let vars = [
        ("FEEDVAR", "server"),
        ("HOME", home_s.as_str()),
        ("KLARK_WORKSPACE_ID", "ws-leak"),
        ("KLARK_TAB_ID", "tab-leak"),
        ("KLARK_PANE_ID", "pane-leak"),
    ];
    env.start_server_with("work", &vars);
    let pane = env.fake_agent("work");
    let sub = env.base.join("sub");
    fs::create_dir_all(&sub).unwrap();
    let cmd = "echo \"E22 v=$FEEDVAR p=${PROFILE_SEEN:-none} t=${TERM_PROGRAM:-none} pane=${KLARK_PANE_ID:-none} ws=${KLARK_WORKSPACE_ID:-none} tab=${KLARK_TAB_ID:-none} u=$(id -un) d=$(pwd)\"; exec sleep 1000";
    let out = env
        .feed(&[
            "add",
            "work/rev",
            "Env.",
            "--stream",
            cmd,
            "--message-gap",
            "1",
            "--cwd",
            "sub",
            "--name",
            "e22",
        ])
        .env("FEEDVAR", "creator")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out));
    let real_sub = fs::canonicalize(&sub).unwrap().display().to_string();
    env.wait_text("work", &pane, "E22 v=creator", 15);
    env.wait_sends("e22", 1, 10);
    // I10: the first run has the caller's variables, user and cwd.
    let me = Command::new("id").arg("-un").output().unwrap();
    let me = String::from_utf8_lossy(&me.stdout).trim().to_string();
    let t = env.pane_text("work", &pane);
    let first = t.lines().find(|l| l.contains("E22 v=creator")).unwrap();
    assert!(first.contains(&format!("u={me} d={real_sub}")), "{first}");

    env.stop_server("work");
    wait_for("the worker to end", Duration::from_secs(20), || {
        (env.workers("e22") == 0).then_some(())
    });
    env.start_server_with("work", &vars);
    let pane = env.ensure_agent("work", "rev");
    env.wait_text("work", &pane, "E22 v=server", 60);
    let t = env.pane_text("work", &pane);
    let line = t
        .lines()
        .find(|l| l.contains("E22 v=server"))
        .unwrap()
        .to_string();
    assert!(
        line.contains("p=yes"),
        "a login shell read the profile: {line}"
    );
    assert!(line.contains("t=klark"), "a pane's environment: {line}");
    assert!(line.contains("pane=none"), "{line}");
    assert!(
        line.contains("ws=none") && line.contains("tab=none"),
        "{line}"
    );
    assert!(t.contains(&format!("d={real_sub}")), "same cwd: {t}");
    assert!(!fs::read_dir(env.store_dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| fs::read_to_string(e.path())
            .unwrap_or_default()
            .contains("creator")
            && !e.file_name().to_string_lossy().contains(".pending")
            && !e.file_name().to_string_lossy().contains(".log")));
}

/// I29: `feed set` changes `[feeds]` and prints it back; a new feed takes the
/// defaults; a flag wins; an existing feed keeps its values; no message
/// carries more than the feed's `max-bytes` of lines.
#[test]
fn settings() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    let all = env.feed_ok(&["set"]);
    assert!(
        all.contains("[feeds]") && all.contains("max_bytes = 4096"),
        "{all}"
    );
    let out = env.feed_ok(&["set", "max-bytes", "300"]);
    assert!(out.contains("max_bytes = 300"), "{out}");
    env.feed_ok(&["set", "idle-stop", "30d"]);
    env.feed_ok(&["set", "max_per_hour", "60"]);
    let cfg = env.config_text();
    assert!(
        cfg.contains("[feeds]") && cfg.contains("max_bytes = 300"),
        "{cfg}"
    );
    assert!(cfg.contains("idle_stop = \"30d\""), "{cfg}");
    assert!(env
        .feed_ok(&["set", "max-bytes"])
        .contains("max_bytes = 300"));
    let bad = env.feed_out(&["set", "colour", "red"]);
    assert_eq!(bad.status.code(), Some(2), "{}", text(&bad));
    assert!(text(&bad).contains("max-bytes"), "{}", text(&bad));
    let bad = env.feed_out(&["set", "max-bytes", "10"]);
    assert_eq!(bad.status.code(), Some(2), "{}", text(&bad));

    let script = "i=0; while [ $i -lt 40 ]; do echo \"big-line-$i-xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\"; i=$((i+1)); done; exec sleep 1000";
    env.feed_ok(&[
        "add",
        "work/rev",
        "Big.",
        "--stream",
        script,
        "--message-gap",
        "1",
        "--name",
        "s29",
    ]);
    env.feed_ok(&[
        "add",
        "work/rev",
        "Flag.",
        "--stream",
        "exec sleep 999",
        "--max-bytes",
        "9999",
        "--name",
        "t29",
    ]);
    let show = env.feed_ok(&["show", "s29"]);
    assert!(show.contains("max bytes          300"), "{show}");
    assert!(show.contains("max per hour       60"), "{show}");
    assert!(show.contains("idle stop          30d"), "{show}");
    assert!(env
        .feed_ok(&["show", "t29"])
        .contains("max bytes          9999"));

    env.feed_ok(&["set", "max-bytes", "5000"]);
    assert!(env
        .feed_ok(&["show", "s29"])
        .contains("max bytes          300"));

    env.wait_text("work", &pane, "big-line-39", 15);
    let t = env.pane_text("work", &pane);
    assert!(t.contains("klark feed batch s29"), "the batch was cut: {t}");
    // The pane shows each message twice (the tty echo, then `cat`).
    let shown: std::collections::BTreeSet<&str> = t
        .lines()
        .map(str::trim)
        // Only whole lines: on Linux the echo and `cat` can interleave
        // mid-line, and the pieces are not lines the message held.
        .filter(|l| {
            l.strip_prefix("big-line-")
                .and_then(|r| r.split_once('-'))
                .is_some_and(|(n, x)| n.parse::<u32>().is_ok() && x == "x".repeat(34))
        })
        .collect();
    let lines_bytes: usize = shown.iter().map(|l| l.len() + 1).sum();
    assert!(lines_bytes <= 300, "{lines_bytes} bytes of lines: {t}");
}

/// I30: with `agents_may_create = false`, `feed add` from an agent pane is
/// refused; from a user shell it is allowed.
#[test]
fn agents_may_not_create() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    env.feed_ok(&["set", "agents-may-create", "off"]);
    let refused = env.feed_in_pane(
        "work",
        &pane,
        &[
            "add",
            "rev",
            "x",
            "--stream",
            "exec sleep 998",
            "--name",
            "a30",
        ],
    );
    assert!(!refused.status.success(), "{}", text(&refused));
    assert!(
        text(&refused).contains("agents_may_create"),
        "{}",
        text(&refused)
    );
    assert!(env.listed("a30").is_none());
    // Nor may the agent turn the setting back on.
    let flip = env.feed_in_pane("work", &pane, &["set", "agents-may-create", "on"]);
    assert!(!flip.status.success(), "{}", text(&flip));
    assert!(env.config_text().contains("agents_may_create = false"));
    env.feed_ok(&[
        "add",
        "work/rev",
        "x",
        "--stream",
        "exec sleep 998",
        "--name",
        "a30",
    ]);
    assert!(env.listed("a30").is_some());
}

/// A1 + A3 (server side): the app's socket API `feed.list | start | stop`
/// does what `klark feed list | start | stop` does, answered by the server
/// (the client may be remote, so the overlay never reads the store itself).
#[test]
fn app_socket_api_lists_starts_and_stops_feeds() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    let sock = env.socket("work");
    let m = marker(48);
    let script = format!("{m} & while true; do echo tick; sleep 2; done");
    env.feed_ok(&[
        "add",
        "work/rev",
        "Look.",
        "--stream",
        &script,
        "--message-gap",
        "1",
        "--max-per-hour",
        "1",
        "--name",
        "ap",
    ]);
    env.wait_text("work", &pane, "too many updates", 30);
    env.wait_state("ap", "stopped:", 15);

    let listed = call(&sock, "feed.list", json!({}));
    let feeds = listed["result"]["feeds"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let ap = feeds
        .iter()
        .find(|f| f["id"] == json!("ap"))
        .unwrap_or_else(|| panic!("feed.list: {listed}"));
    assert_eq!(listed["result"]["type"], "feed_list");
    assert_eq!(ap["kind"], "stream");
    assert!(
        ap["state"].as_str().unwrap().starts_with("stopped:"),
        "{ap}"
    );
    assert!(
        ap["agents"][0].as_str().unwrap().starts_with("work/rev"),
        "{ap}"
    );
    assert!(ap["last_delivery"].as_u64().is_some(), "{ap}");

    let started = call(&sock, "feed.start", json!({"id": "ap"}));
    assert_eq!(started["result"]["type"], "feed_started", "{started}");
    env.wait_state("ap", "running", 10);
    wait_procs(&m, 1, 10);

    let missing = call(&sock, "feed.start", json!({"id": "nope"}));
    assert!(
        missing["error"]["message"]
            .as_str()
            .unwrap_or("")
            .contains("no feed nope"),
        "{missing}"
    );

    let stopped = call(&sock, "feed.stop", json!({"id": "ap"}));
    assert_eq!(stopped["result"]["type"], "feed_stopped", "{stopped}");
    wait_procs(&m, 0, 10);
    assert!(env.listed("ap").is_none());
    let listed = call(&sock, "feed.list", json!({}));
    assert_eq!(listed["result"]["feeds"], json!([]), "{listed}");
}

/// The `klark feed …` commands of the skill's "Feeds" section, in order:
/// each from a ```bash block, `\` continuations joined.
fn skill_feed_examples() -> Vec<String> {
    let skill = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("skills/klark/references/feed.md"),
    )
    .unwrap();
    let section = skill
        .split("\n## ")
        .find(|s| s.starts_with("Feeds"))
        .expect("references/feed.md has a Feeds section");
    let mut out = Vec::new();
    let mut in_block = false;
    let mut cur = String::new();
    for line in section.lines() {
        if line.starts_with("```") {
            in_block = !in_block && line == "```bash";
            continue;
        }
        if !in_block {
            continue;
        }
        let line = line.trim();
        match line.strip_suffix('\\') {
            Some(head) => {
                cur.push_str(head);
                continue;
            }
            None => cur.push_str(line),
        }
        let whole = std::mem::take(&mut cur);
        if whole.starts_with("klark feed ") {
            out.push(whole);
        }
    }
    out
}

/// I16: every `klark feed` example in the skill's Feeds section runs as
/// written, through `sh -c`, from the pane of an agent named `reviewer`,
/// with a second agent named `triage`. What the examples assume is put in
/// place, and the only text changed is the placeholder `<id>`:
/// - `app.log`, `deploy.log` and `nginx.conf` exist in the working directory;
/// - `himalaya`, `curl` and `jevx` are stubs on `PATH` (no mail, no network,
///   no model), `klark` is the binary under test, and `STATUS_TOKEN` is set;
/// - `<id>` becomes the first feed `feed list` shows with `reviewer` among
///   its agents, at the moment the example runs.
#[test]
fn skill_examples_run_as_written() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent_named("work", "reviewer");
    env.fake_agent_named("work", "triage");
    for f in ["app.log", "deploy.log", "nginx.conf"] {
        fs::write(env.base.join(f), "").unwrap();
    }
    let bin = env.base.join("skillbin");
    fs::create_dir_all(&bin).unwrap();
    std::os::unix::fs::symlink(BIN, bin.join("klark")).unwrap();
    let stubs = [
        ("himalaya", "echo '1  alice  Lunch?'"),
        (
            "curl",
            "[ -n \"$STATUS_TOKEN\" ] || exit 3; echo '{\"ok\":true}'",
        ),
        ("jevx", "cat >/dev/null; echo 'yes 0.9'"),
    ];
    for (name, body) in stubs {
        use std::os::unix::fs::PermissionsExt;
        let p = bin.join(name);
        fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path = format!("{}:/usr/bin:/bin:/usr/sbin:/sbin", bin.display());

    let examples = skill_feed_examples();
    assert!(
        examples
            .iter()
            .filter(|e| e.starts_with("klark feed add "))
            .count()
            >= 5,
        "{examples:#?}"
    );
    assert!(
        examples[0].contains("grep --line-buffered"),
        "the filtered log comes first"
    );
    let mut adds = 0;
    for example in &examples {
        let line = if example.contains("<id>") {
            let id = env
                .list()
                .into_iter()
                .find(|f| f["agents"].to_string().contains("reviewer"))
                .and_then(|f| f["id"].as_str().map(str::to_string))
                .unwrap_or_else(|| panic!("no feed of reviewer left for {example}"));
            example.replace("<id>", &id)
        } else {
            example.clone()
        };
        let base = env.cmd();
        let mut c = Command::new("/bin/sh");
        for (k, v) in base.get_envs() {
            match v {
                Some(v) => c.env(k, v),
                None => c.env_remove(k),
            };
        }
        let out = c
            .current_dir(&env.base)
            .env("PATH", &path)
            .env("STATUS_TOKEN", "not-a-real-token")
            .env("KLARK_SESSION", "work")
            .env("KLARK_PANE_ID", &pane)
            .args(["-c", &line])
            .output()
            .unwrap();
        assert!(out.status.success(), "{line}\n{}", text(&out));
        if example.starts_with("klark feed add ") {
            adds += 1;
            if adds == 1 {
                // The first example works, not just parses.
                thread::sleep(Duration::from_millis(1500));
                append(&env.base.join("app.log"), "INFO fine\nERROR skill-boom\n");
                env.wait_text("work", &pane, "skill-boom", 40);
                assert!(!env.pane_text("work", &pane).contains("INFO fine"));
            }
        }
    }
    assert!(env.list().len() < adds, "leave and stop ended feeds");
}

// ---------------------------------------------------------------- gate: I1, I6, I7

/// I1: a stream that never pauses is held to one message per gap, never
/// sooner; a stream that prints nothing sends nothing.
#[test]
fn stream_gap_holds_and_silence_sends_nothing() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    env.feed_ok(&[
        "add",
        "work/rev",
        "Silent.",
        "--stream",
        "exec sleep 1000",
        "--message-gap",
        "1",
        "--name",
        "q1",
    ]);
    env.feed_ok(&[
        "add",
        "work/rev",
        "Steady.",
        "--stream",
        "while true; do echo steady; sleep 0.3; done",
        "--message-gap",
        "4",
        "--name",
        "c1",
    ]);
    env.wait_sends("c1", 1, 20);
    let from = Instant::now();
    let before = env.sends("c1").len();
    thread::sleep(Duration::from_secs(20));
    let during = env.sends("c1").len() - before;
    let secs = from.elapsed().as_secs();
    // One message per 4 s gap at most (plus one at the edge of the window),
    // and the stream is not starved by never being quiet for 2 s. (Each
    // delivery to the stand-in also waits ~2 s to see it start working.)
    assert!(during as u64 <= secs / 4 + 1, "{during} in {secs} s");
    assert!(during >= 2, "{during} in {secs} s:\n{}", env.logs("c1"));
    assert!(env.sends("q1").is_empty(), "{}", env.logs("q1"));
    assert!(!env.pane_text("work", &pane).contains("Silent."));
}

impl Env {
    fn report_agent(&self, session: &str, pane: &str, state: &str) {
        let r = call(
            &self.socket(session),
            "pane.report_agent",
            json!({"pane_id": pane, "source": "test", "agent": "claude", "state": state}),
        );
        assert!(r.get("error").is_none(), "{r}");
        wait_for(
            &format!("the agent to be {state}"),
            Duration::from_secs(10),
            || {
                let list = call(&self.socket(session), "agent.list", json!({}));
                list["result"]["agents"]
                    .as_array()
                    .is_some_and(|a| {
                        a.iter().any(|a| {
                            a["pane_id"] == json!(pane) && a["agent_status"] == json!(state)
                        })
                    })
                    .then_some(())
            },
        );
    }
}

/// I6: nothing is delivered while the agent works; when it turns idle it
/// gets everything pending in one message, nothing lost or doubled.
#[test]
fn busy_agent_gets_one_combined_message() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    fs::write(env.base.join("b.log"), "").unwrap();
    env.feed_ok(&[
        "add",
        "work/rev",
        "Busy.",
        "--follow",
        "b.log",
        "--message-gap",
        "1",
        "--name",
        "b6",
    ]);
    thread::sleep(Duration::from_millis(1500));
    append(&env.base.join("b.log"), "warm-1\n");
    env.wait_text("work", &pane, "warm-1", 15);
    env.wait_sends("b6", 1, 10);

    env.report_agent("work", &pane, "working");
    for l in ["busy-1\n", "busy-2\n", "busy-3\n"] {
        append(&env.base.join("b.log"), l);
        thread::sleep(Duration::from_millis(1200));
    }
    thread::sleep(Duration::from_secs(3));
    assert_eq!(env.sends("b6").len(), 1, "{}", env.logs("b6"));
    assert!(!env.pane_text("work", &pane).contains("busy-1"));

    env.report_agent("work", &pane, "idle");
    env.wait_text("work", &pane, "busy-3", 15);
    env.wait_sends("b6", 2, 15);
    thread::sleep(Duration::from_secs(2));
    let s = env.sends("b6");
    assert_eq!(
        s.len(),
        2,
        "one combined message: {s:?}\n{}\n{}",
        env.logs("b6"),
        env.pane_text("work", &pane)
    );
    assert!(s[1].contains("(3 line(s)"), "{s:?}");
}

/// A client attached to `session`'s client socket, its frames drained.
fn attach_client(env: &Env, session: &str) -> UnixStream {
    let sock = env.socket(session).with_file_name("klark-client.sock");
    let mut s = UnixStream::connect(&sock).unwrap();
    support::client_shell_handshake(
        &mut s,
        support::CURRENT_ENDPOINT_PROTOCOL_GENERATION,
        100,
        30,
    )
    .unwrap();
    let mut drain = s.try_clone().unwrap();
    thread::spawn(move || while support::read_server_message(&mut drain).is_ok() {});
    s
}

fn varint(mut v: u32, out: &mut Vec<u8>) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

/// One `ClientShellPaneInput` event, as the client sends a keystroke.
fn client_input(s: &mut UnixStream, pane: &str, event: &[u8]) {
    let mut p = Vec::new();
    varint(13, &mut p); // ClientShellPaneInput
    varint(pane.len() as u32, &mut p);
    p.extend_from_slice(pane.as_bytes());
    varint(1, &mut p);
    p.extend_from_slice(event);
    let mut framed = (p.len() as u32).to_le_bytes().to_vec();
    framed.extend_from_slice(&p);
    s.write_all(&framed).unwrap();
    s.flush().unwrap();
}

fn typed(text: &str) -> Vec<u8> {
    let mut e = Vec::new();
    varint(1, &mut e); // TextCommit
    varint(text.len() as u32, &mut e);
    e.extend_from_slice(text.as_bytes());
    e
}

/// A key press with no text (`code`: 1 Enter, 2 Left).
fn key(code: u32) -> Vec<u8> {
    let mut e = Vec::new();
    varint(0, &mut e); // Key
    varint(code, &mut e);
    e.push(0); // no modifiers
    varint(0, &mut e); // Press
    varint(1, &mut e); // repeat count
    e.extend_from_slice(&[0, 0, 0, 0, 0]);
    e
}

/// I7: a draft in the agent's input is lifted out for a delivery and typed
/// back unsent; mid-edit nothing is delivered until the user pauses.
#[test]
fn delivery_keeps_the_users_draft() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    fs::write(env.base.join("d.log"), "").unwrap();
    env.feed_ok(&[
        "add",
        "work/rev",
        "Draft.",
        "--follow",
        "d.log",
        "--message-gap",
        "1",
        "--name",
        "d7",
    ]);
    let mut client = attach_client(&env, "work");
    client_input(&mut client, &pane, &typed("DRAFT7"));
    env.wait_text("work", &pane, "DRAFT7", 10);

    append(&env.base.join("d.log"), "d-line-1\n");
    env.wait_text("work", &pane, "d-line-1", 15);
    env.wait_sends("d7", 1, 10);
    let last = wait_for("the draft typed back", Duration::from_secs(10), || {
        let t = env.pane_text("work", &pane);
        let last = t
            .lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty())?
            .to_string();
        (last == "DRAFT7").then_some(t)
    });
    assert!(
        !last
            .lines()
            .any(|l| l.contains("DRAFT7") && l.contains("klark feed")),
        "the draft was not sent with the message: {last}"
    );

    // Mid-edit (an arrow key: the draft is unknowable) holds the delivery.
    client_input(&mut client, &pane, &key(1));
    client_input(&mut client, &pane, &key(2));
    let edited = Instant::now();
    append(&env.base.join("d.log"), "d-line-2\n");
    thread::sleep(Duration::from_secs(3));
    if edited.elapsed() < Duration::from_secs(4) {
        assert!(!env.pane_text("work", &pane).contains("d-line-2"));
    }
    env.wait_text("work", &pane, "d-line-2", 20);
    assert!(edited.elapsed() >= Duration::from_secs(5));
}

/// I19 (background, the default): a command that dies at once, stream or
/// poll, fails on the terminal and no feed is saved.
#[test]
fn dead_command_saves_no_feed() {
    let mut env = Env::new();
    env.start_server("work");
    env.fake_agent("work");
    for source in ["--stream", "--poll"] {
        let out = env.feed_out(&[
            "add",
            "work/rev",
            "Watch.",
            source,
            "no-such-command-kf19",
            "--name",
            "dead19",
        ]);
        assert!(!out.status.success(), "{source}: {}", text(&out));
        assert!(env.list().is_empty(), "{source}: {:?}", env.list());
        assert!(!env.store_dir().join("dead19.json").exists(), "{source}");
    }
}

/// A poll feed's later subscriber gets the current output once, then only
/// what is new (ADR 0015, "An agent that joins later gets the current output
/// once").
#[test]
fn poll_joiner_gets_the_current_output_once() {
    let mut env = Env::new();
    env.start_server("work");
    let p1 = env.fake_agent_named("work", "rev");
    let p2 = env.fake_agent_named("work", "rev2");
    let list = env.base.join("j.txt");
    fs::write(&list, "item-1\nitem-2\n").unwrap();
    let args = |agent: &'static str| {
        [
            "add",
            agent,
            "Items.",
            "--poll",
            "cat j.txt",
            "--interval",
            "1",
            "--name",
            "pj",
        ]
    };
    env.feed_ok(&args("work/rev"));
    env.wait_text("work", &p1, "item-2", 15);
    let joined = env.feed_ok(&args("work/rev2"));
    assert!(joined.contains("joined"), "{joined}");
    env.wait_text("work", &p2, "item-2", 15);
    thread::sleep(Duration::from_secs(3));
    fs::write(&list, "item-1\nitem-2\nitem-3\n").unwrap();
    env.wait_text("work", &p2, "item-3", 15);
    env.wait_text("work", &p1, "item-3", 15);
    env.wait_sends("pj", 4, 15);
    thread::sleep(Duration::from_secs(2));
    let s = env.sends("pj");
    // rev: item-1+2, item-3; rev2: item-1+2 once, item-3.
    assert_eq!(s.len(), 4, "{s:?}");
    let to_rev2: Vec<_> = s.iter().filter(|l| l.ends_with("work/rev2")).collect();
    assert_eq!(to_rev2.len(), 2, "{s:?}");
    assert!(to_rev2[0].contains("(2 line(s)") && to_rev2[1].contains("(1 line(s)"));
}

// ---------------------------------------------------------------- edge cases

/// A filter that records each batch it is asked about and lets it through:
/// the exact lines a message carries, before the terminal renders them.
const CAPTURE: &str = "cat >> cap.txt; echo true";

impl Env {
    fn captured(&self) -> String {
        fs::read_to_string(self.base.join("cap.txt")).unwrap_or_default()
    }

    fn wait_captured(&self, needle: &str, secs: u64) -> String {
        wait_for(
            &format!("{needle:?} in a batch"),
            Duration::from_secs(secs),
            || {
                let c = self.captured();
                c.contains(needle).then_some(c)
            },
        )
    }

    /// Files of feed `id` left in the store.
    fn feed_files(&self, id: &str) -> Vec<String> {
        fs::read_dir(self.store_dir())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with(&format!("{id}.")))
            .collect()
    }
}

/// Lines summed over the worker's "sent" log lines.
fn sent_lines(sends: &[String]) -> usize {
    sends
        .iter()
        .filter_map(|l| {
            let rest = l.split("bytes (").nth(1)?;
            rest.split(' ').next()?.parse::<usize>().ok()
        })
        .sum()
}

/// What a stream prints reaches the agent clean: CRLF without the `\r`,
/// ANSI escapes removed, invalid UTF-8 replaced, a line over 4 KiB cut with
/// the marker, a whitespace line kept, and a last line without a newline
/// delivered when the command exits.
#[test]
fn stream_input_is_cleaned() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    fs::write(
        env.base.join("in.sh"),
        "printf 'crlf-1\\r\\ncrlf-2\\r\\n'\n\
         printf '\\033[31mansi-red\\033[0m\\n'\n\
         printf 'bad-\\377\\376-utf8\\n'\n\
         head -c 10000 /dev/zero | tr '\\0' L; printf '\\n'\n\
         printf '   \\n'\n\
         sleep 1\n\
         printf 'no-newline-at-exit'\n",
    )
    .unwrap();
    env.feed_ok(&[
        "add",
        "work/rev",
        "Clean.",
        "--stream",
        "sh in.sh",
        "--message-gap",
        "1",
        "--filter",
        CAPTURE,
        "--name",
        "xe1",
    ]);
    env.wait_text("work", &pane, "exited normally", 30);
    let c = env.wait_captured("no-newline-at-exit", 5);
    assert!(c.contains("crlf-1\ncrlf-2\n"), "{c:?}");
    assert!(!c.contains('\r') && !c.contains('\x1b'), "{c:?}");
    assert!(c.contains("\nansi-red\n"), "{c:?}");
    assert!(c.contains("bad-\u{fffd}\u{fffd}-utf8"), "{c:?}");
    let long = format!("{} … [cut {} bytes]\n", "L".repeat(4096), 10_000 - 4096);
    assert!(c.contains(&long), "the long line is cut once");
    assert!(c.contains("\n   \n"), "{c:?}");
    assert!(c.ends_with("no-newline-at-exit\n"), "{c:?}");
}

/// Poll output that shrinks to nothing sends nothing; when it comes back it
/// is new again. With `--diff unified` too: an empty previous output is not
/// a first run (it was recorded), so the return is sent as a diff.
#[test]
fn poll_output_that_goes_empty_and_comes_back() {
    let mut env = Env::new();
    env.start_server("work");
    env.fake_agent("work");
    let list = env.base.join("l.txt");
    let doc = env.base.join("d.txt");
    fs::write(&list, "a-item\r\nb-item\r\n").unwrap();
    fs::write(&doc, "").unwrap();
    for (name, file, diff) in [("xpl", "l.txt", "lines"), ("xpu", "d.txt", "unified")] {
        env.feed_ok(&[
            "add",
            "work/rev",
            "Items.",
            "--poll",
            &format!("cat {file}"),
            "--interval",
            "1",
            "--diff",
            diff,
            "--filter",
            CAPTURE,
            "--name",
            name,
        ]);
    }
    env.wait_sends("xpl", 1, 15);
    assert!(env.captured().contains("a-item\nb-item\n"), "no \\r");
    // pu's first run (empty) records only.
    fs::write(&list, "").unwrap();
    fs::write(&doc, "doc-v1\n").unwrap();
    env.wait_captured("+doc-v1", 15);
    thread::sleep(Duration::from_secs(3));
    assert_eq!(env.sends("xpl").len(), 1, "empty output sends nothing");

    fs::write(&doc, "").unwrap();
    env.wait_captured("-doc-v1", 15);
    let before = env.sends("xpu").len();
    fs::write(&list, "a-item\n").unwrap();
    fs::write(&doc, "doc-v2\n").unwrap();
    env.wait_sends("xpl", 2, 15);
    let c = env.wait_captured("+doc-v2", 15);
    assert!(env.sends("xpu").len() > before, "{c}");
    assert_eq!(c.matches("a-item").count(), 2, "back after empty: new");
}

/// Several feeds to one agent and one feed to three agents: each agent gets
/// its own instruction; one leaving or removed does not touch the others,
/// and leaves no pending file behind while output keeps coming.
#[test]
fn many_feeds_and_many_agents() {
    let mut env = Env::new();
    env.start_server("work");
    let p1 = env.fake_agent_named("work", "rev");
    let p2 = env.fake_agent_named("work", "ag2");
    let p3 = env.fake_agent_named("work", "ag3");
    for f in ["m.log", "xs1.log", "xs2.log"] {
        fs::write(env.base.join(f), "").unwrap();
    }
    let follow = |file: &str, name: &str, gap: &str| {
        env.feed_ok(&[
            "add",
            "work/rev",
            &format!("For rev from {name}."),
            "--follow",
            file,
            "--message-gap",
            gap,
            "--max-per-hour",
            "0",
            "--name",
            name,
        ]);
    };
    follow("m.log", "xmm", "300");
    follow("xs1.log", "xs1", "1");
    follow("xs2.log", "xs2", "1");
    env.feed_ok(&["agent", "add", "xmm", "work/ag2", "For ag2."]);
    env.feed_ok(&["agent", "add", "xmm", "work/ag3", "For ag3."]);
    thread::sleep(Duration::from_millis(1500));

    append(&env.base.join("m.log"), "shared-1\n");
    append(&env.base.join("xs1.log"), "only-s1\n");
    append(&env.base.join("xs2.log"), "only-s2\n");
    for (p, who) in [(&p1, "rev from xmm"), (&p2, "ag2"), (&p3, "ag3")] {
        env.wait_text("work", p, "shared-1", 15);
        env.wait_text("work", p, &format!("For {who}."), 5);
    }
    env.wait_text("work", &p1, "only-s1", 15);
    env.wait_text("work", &p1, "only-s2", 15);
    for p in [&p2, &p3] {
        let t = env.pane_text("work", p);
        assert!(!t.contains("only-s") && !t.contains("For rev"), "{t}");
    }

    // Within mm's 300 s gap: queued for all three, on disk.
    append(&env.base.join("m.log"), "shared-2\n");
    wait_for("shared-2 queued", Duration::from_secs(10), || {
        fs::read_to_string(env.store_dir().join("xmm.1.pending"))
            .unwrap_or_default()
            .contains("shared-2")
            .then_some(())
    });
    let out = env.feed_in_pane("work", &p2, &["leave", "xmm"]);
    assert!(out.status.success(), "{}", text(&out));
    env.feed_ok(&["agent", "remove", "xmm", "work/ag3"]);
    for i in 3..6 {
        append(&env.base.join("m.log"), &format!("shared-{i}\n"));
        thread::sleep(Duration::from_millis(700));
    }
    thread::sleep(Duration::from_secs(2));
    let files = env.feed_files("xmm");
    assert!(
        !files
            .iter()
            .any(|f| f == "xmm.1.pending" || f == "xmm.2.pending"),
        "no pending file for agents that left: {files:?}"
    );
    let pending = fs::read_to_string(env.store_dir().join("xmm.0.pending")).unwrap();
    assert!(pending.contains("shared-5"), "rev still queues: {pending}");
    append(&env.base.join("xs1.log"), "s1-after\n");
    env.wait_text("work", &p1, "s1-after", 15);
    for p in [&p2, &p3] {
        assert!(!env.pane_text("work", p).contains("shared-2"));
    }
}

/// `feed stop` while output keeps coming: it ends at once, kills the
/// command, removes every file and nothing more arrives; a second stop and
/// an unknown id are errors; `feed add` to a pane that is gone is refused.
#[test]
fn stop_edge_cases() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    let script = format!("tick51-{}.sh", std::process::id());
    fs::write(
        env.base.join(&script),
        "i=0; while :; do i=$((i+1)); echo tick-$i; sleep 0.05; done\n",
    )
    .unwrap();
    let busy = format!("sh {script}");
    let ticking = || {
        Command::new("pgrep")
            .args(["-f", &script])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    env.feed_ok(&[
        "add",
        "work/rev",
        "Ticks.",
        "--stream",
        &busy,
        "--message-gap",
        "1",
        "--max-per-hour",
        "0",
        "--name",
        "xst",
    ]);
    env.wait_sends("xst", 2, 20);
    let started = Instant::now();
    let out = env.feed_ok(&["stop", "xst"]);
    assert!(out.contains("stopped and removed"), "{out}");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(env.workers("xst"), 0);
    assert!(!ticking(), "the loop is gone");
    assert!(
        env.feed_files("xst").is_empty(),
        "{:?}",
        env.feed_files("xst")
    );
    let frames = env
        .pane_text("work", &pane)
        .matches("[klark feed xst")
        .count();
    thread::sleep(Duration::from_secs(3));
    assert_eq!(
        env.pane_text("work", &pane)
            .matches("[klark feed xst")
            .count(),
        frames,
        "nothing after the stop"
    );

    let again = env.feed_out(&["stop", "xst"]);
    assert!(!again.status.success());
    assert!(text(&again).contains("no feed xst"), "{}", text(&again));
    let unknown = env.feed_out(&["stop", "nope-kf"]);
    assert!(!unknown.status.success());
    assert!(
        text(&unknown).contains("no feed nope-kf"),
        "{}",
        text(&unknown)
    );
    assert!(env.feed_ok(&["stop", "--all"]).contains("no feeds"));

    call(&env.socket("work"), "pane.close", json!({"pane_id": pane}));
    thread::sleep(Duration::from_secs(1));
    for to in [pane.as_str(), "work/rev"] {
        let out = env.feed_out(&["add", to, "x", "--stream", "sleep 30"]);
        assert!(!out.status.success(), "{to}: {}", text(&out));
    }
    assert!(env.list().is_empty());
}

/// `--follow` of a path that turns into a directory: reported once; the
/// file coming back is followed again, with `recovered`.
#[test]
fn follow_path_becomes_a_directory() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    let log = env.base.join("dir.log");
    fs::write(&log, "").unwrap();
    env.feed_ok(&[
        "add",
        "work/rev",
        "Follow.",
        "--follow",
        "dir.log",
        "--message-gap",
        "1",
        "--name",
        "xfd",
    ]);
    thread::sleep(Duration::from_millis(1500));
    append(&log, "before-dir\n");
    env.wait_text("work", &pane, "before-dir", 15);
    fs::remove_file(&log).unwrap();
    fs::create_dir(&log).unwrap();
    env.wait_text("work", &pane, "cannot be read", 15);
    thread::sleep(Duration::from_secs(3));
    let notices = env
        .sends("xfd")
        .iter()
        .filter(|l| l.contains("1 notice(s)"))
        .count();
    assert_eq!(notices, 1, "reported once: {}", env.logs("xfd"));
    fs::remove_dir(&log).unwrap();
    append(&log, "file-again\n");
    env.wait_text("work", &pane, "file-again", 15);
    env.wait_text("work", &pane, "recovered", 5);
}

/// The worker log is rotated at 1 MiB (one old copy, private) and the
/// server's `resume.log` is removed at server start once over 256 KiB.
#[test]
fn logs_are_capped() {
    let mut env = Env::new();
    env.start_server("work");
    env.fake_agent("work");
    let m = marker(52);
    env.feed_ok(&["add", "work/rev", "x", "--stream", &m, "--name", "xlg"]);
    wait_procs(&m, 1, 10);
    env.stop_server("work");
    wait_for("the worker to end", Duration::from_secs(20), || {
        (env.workers("xlg") == 0).then_some(())
    });
    let dir = env.store_dir();
    append(&dir.join("xlg.log"), &"w".repeat(1024 * 1024 + 10));
    append(&dir.join("resume.log"), &"r".repeat(256 * 1024 + 10));
    env.start_server("work");
    env.ensure_agent("work", "rev");
    wait_procs(&m, 1, 60);
    let old = fs::metadata(dir.join("xlg.log.1")).expect("xlg.log.1");
    assert!(old.len() > 1024 * 1024);
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(old.permissions().mode() & 0o777, 0o600);
    assert!(fs::metadata(dir.join("xlg.log")).unwrap().len() < 64 * 1024);
    assert!(env.logs("xlg").contains("running"));
    let resume = fs::metadata(dir.join("resume.log")).unwrap();
    assert!(resume.len() < 64 * 1024, "{}", resume.len());
}

/// A command's background children go with it: a stream that started one
/// and exited, a stopped stream, and a poll run.
#[test]
fn background_children_are_killed() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    let (ma, mb, mc, md) = (marker(53), marker(54), marker(55), marker(56));
    env.feed_ok(&[
        "add",
        "work/rev",
        "x",
        "--stream",
        &format!("({ma} &) ; echo started-a; sleep 2"),
        "--name",
        "xba",
    ]);
    env.wait_text("work", &pane, "exited normally", 30);
    wait_procs(&ma, 0, 10);

    env.feed_ok(&[
        "add",
        "work/rev",
        "x",
        "--stream",
        &format!("{mb} & {mc}"),
        "--name",
        "xbb",
    ]);
    wait_procs(&mb, 1, 10);
    wait_procs(&mc, 1, 10);
    env.feed_ok(&["stop", "xbb"]);
    wait_procs(&mb, 0, 5);
    wait_procs(&mc, 0, 5);

    env.feed_ok(&[
        "add",
        "work/rev",
        "x",
        "--poll",
        &format!("{md} & echo polled"),
        "--interval",
        "1",
        "--name",
        "xbd",
    ]);
    env.wait_text("work", &pane, "polled", 15);
    thread::sleep(Duration::from_secs(3));
    assert_eq!(procs(&md), 0, "each run's leftovers go");
}

/// A flood (100k lines at once) is bounded by the pending cap: the message
/// says how many older lines were dropped, the saved batch holds the newest,
/// the pending file stays small.
#[test]
fn flood_is_capped_and_counted() {
    let mut env = Env::new();
    env.start_server("work");
    let pane = env.fake_agent("work");
    let m = marker(57);
    env.feed_ok(&[
        "add",
        "work/rev",
        "Flood.",
        "--stream",
        &format!("seq 1 100000; {m}"),
        "--message-gap",
        "1",
        "--max-bytes",
        "256",
        "--name",
        "xfl",
    ]);
    let s = env.wait_sends("xfl", 1, 30);
    let shown = s[0]
        .split("bytes (")
        .nth(1)
        .and_then(|r| r.split(' ').next())
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap();
    env.wait_text("work", &pane, "older lines were dropped", 10);
    let t = env.pane_text("work", &pane);
    let dropped: usize = t
        .split("(… ")
        .nth(1)
        .and_then(|r| r.split(' ').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("{t}"));
    assert_eq!(dropped + shown, 100_000, "{t}");
    assert!(shown * 6 <= 64 * 1024 + 6, "within the cap: {shown}");
    assert!(t.contains("\n100000\n") || t.contains("100000"), "{t}");
    let batch = env.feed_ok(&["batch", "xfl", "1"]);
    assert!(batch.ends_with("100000\n"));
    assert_eq!(batch.lines().count(), shown);
    let pending = fs::metadata(env.store_dir().join("xfl.0.pending"))
        .map(|m| m.len())
        .unwrap_or(0);
    assert!(pending < 64 * 1024, "{pending}");
}

/// A server restart in the middle of a steady flood into a followed file:
/// every line arrives, in order, and none is delivered twice.
#[test]
fn restart_mid_flood_loses_and_doubles_nothing() {
    let mut env = Env::new();
    env.start_server("work");
    env.fake_agent("work");
    let log = env.base.join("flood.log");
    fs::write(&log, "").unwrap();
    env.feed_ok(&[
        "add",
        "work/rev",
        "Flood.",
        "--follow",
        "flood.log",
        "--message-gap",
        "1",
        "--max-per-hour",
        "0",
        "--filter",
        CAPTURE,
        "--name",
        "xrf",
    ]);
    thread::sleep(Duration::from_millis(1500));
    const N: usize = 1200;
    let path = log.clone();
    let writer = thread::spawn(move || {
        for i in 1..=N {
            append(&path, &format!("n{i:05}\n"));
            thread::sleep(Duration::from_millis(8));
        }
    });
    env.wait_captured("n00200", 30);
    env.stop_server("work");
    wait_for("the worker to end", Duration::from_secs(20), || {
        (env.workers("xrf") == 0).then_some(())
    });
    env.start_server("work");
    env.ensure_agent("work", "rev");
    writer.join().unwrap();
    env.wait_captured(&format!("n{N:05}"), 60);
    thread::sleep(Duration::from_secs(3));
    let c = env.captured();
    // The filter may see a batch twice (one held back and retried); the
    // first sight of each line is in order and covers every line.
    let mut seen = std::collections::HashSet::new();
    let order: Vec<usize> = c
        .lines()
        .filter_map(|l| l.strip_prefix('n')?.parse().ok())
        .filter(|n| seen.insert(*n))
        .collect();
    assert_eq!(order, (1..=N).collect::<Vec<_>>());
    let sends = env.sends("xrf");
    assert_eq!(sent_lines(&sends), N, "delivered once each: {sends:?}");
}
