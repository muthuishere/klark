//! The feed worker (ADR 0015): one process per feed, holding `<id>.lock` for
//! its whole life. It runs the source (a stream command, a followed file or a
//! poll command), queues new lines for every subscriber (on disk, so nothing
//! waiting is lost across a restart), and delivers under the message-gap rule.
//! It is the only writer of `<id>.status.json`; it changes the definition
//! only under the store lock (a subscription that stopped itself, a joiner
//! that has had its first output, the feed stopping itself).

use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::agent_index::{call, session_socket};
use crate::feed::filter::{self, Answer};
use crate::feed::follow::Follower;
use crate::feed::lines::{clean, LineSplitter};
use crate::feed::message::{self, Frame};
use crate::feed::model::{format_duration, FeedDef, Mode, Status, SubStatus};
use crate::feed::queue::{Batch, Queue};
use crate::feed::schedule::{Rate, SendCheck};
use crate::feed::store::Store;
use crate::feed::{PENDING_CAP_BYTES, QUIET_MS};

const STDERR_TAIL: usize = 20;
/// A last line without a newline is released after this long.
const PARTIAL_MS: u64 = 5_000;
/// How long a stopping feed keeps trying to deliver its last messages.
const FINAL_DELIVERY_MS: u64 = 120_000;
/// Worker log and stderr copy: one rotation at this size.
const LOG_CAP: u64 = 1024 * 1024;
/// Most 8 KiB reads of a stream's output waiting for the worker.
const STREAM_CHUNKS: usize = 256;
/// A sleep/wake jump counts at most this much toward the idle stop.
const IDLE_STEP_MS: u64 = 60_000;

pub(crate) fn test_mode() -> bool {
    std::env::var("KLARK_FEED_TEST").as_deref() == Ok("1")
}

pub(crate) fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub(crate) fn status_name(id: &str) -> String {
    format!("{id}.status.json")
}

pub(crate) fn def_name(id: &str) -> String {
    format!("{id}.json")
}

pub(crate) fn pending_name(id: &str, n: u32) -> String {
    format!("{id}.{n}.pending")
}

pub(crate) fn log_name(id: &str) -> String {
    format!("{id}.log")
}

/// A poll run longer than this is killed and counts as failed.
fn poll_timeout(interval_secs: u64) -> Duration {
    let floor = if test_mode() { 0 } else { 60 };
    Duration::from_secs((5 * interval_secs).max(floor))
}

fn filter_timeout() -> Duration {
    Duration::from_secs(if test_mode() { 3 } else { 30 })
}

/// How often a worker with nobody reachable looks again.
fn away_check_ms() -> u64 {
    if test_mode() {
        1_000
    } else {
        10_000
    }
}

// ---------------------------------------------------------------- processes

/// The command through the user's shell, in its own process group so the
/// whole pipeline can be stopped together.
pub(crate) fn spawn_shell(command: &str, cwd: &str, stdin: bool) -> std::io::Result<Child> {
    let mut c = shell_command(command);
    if !cwd.is_empty() {
        c.current_dir(cwd);
    }
    c.stdin(if stdin { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    #[cfg(windows)]
    {
        crate::platform::prepare_feed_command(&mut c);
        let mut child = c.spawn()?;
        crate::platform::attach_feed_job(&mut child)?;
        Ok(child)
    }
    #[cfg(not(windows))]
    c.spawn()
}

/// Set in a resumed worker: its commands start the way a new pane's shell
/// does (`[terminal] default_shell`, `shell_mode`, a pane's environment),
/// since the shell that created the feed is gone (ADR 0015).
static PANE_SHELL: std::sync::OnceLock<(String, crate::config::ShellModeConfig)> =
    std::sync::OnceLock::new();

/// How long a resumed worker gives the agents to appear before a server that
/// does not answer counts as down.
const RESUME_GRACE_MS: u64 = 60_000;
/// Every subscriber's server must look down this long before the worker
/// ends (a live handoff closes the socket for a moment).
pub(crate) const DOWN_EXIT_MS: u64 = 3_000;

/// How often the worker asks the servers about its agents: every second
/// while one is reachable, and while every server looks down (it must look
/// again before it ends `DOWN_EXIT_MS` later: a live handoff closes the
/// socket only for a moment); otherwise `away_check_ms`.
fn check_every_ms(reachable: bool, all_down: bool) -> u64 {
    if reachable || all_down {
        1_000
    } else {
        away_check_ms()
    }
}

#[cfg(unix)]
pub(crate) fn shell_command(command: &str) -> Command {
    if let Some((shell, mode)) = PANE_SHELL.get() {
        let config = crate::pane::PaneShellConfig::new(shell, *mode);
        match crate::pane::pane_shell_run_command(config, command) {
            Ok(c) => return c,
            Err(e) => eprintln!("klark feed: the pane shell: {e}; using $SHELL"),
        }
    }
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "/bin/sh".into());
    let mut c = Command::new(shell);
    c.arg("-c").arg(command);
    c
}

#[cfg(windows)]
pub(crate) fn shell_command(command: &str) -> Command {
    let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
    use std::os::windows::process::CommandExt;
    let mut c = Command::new(shell);
    // Raw, not `.arg()`: Rust escapes quotes as \" for the C runtime, which
    // cmd does not understand (`type "a b.txt"` then fails). With /S cmd
    // strips just the outer pair and runs the rest exactly as typed.
    c.raw_arg(format!("/S /C \"{command}\""));
    c
}

/// Stops the command and everything it started: SIGTERM to the group, then
/// SIGKILL after two seconds.
pub(crate) fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    {
        let pgid = child.id() as libc::pid_t;
        // SAFETY: kill(2) with a negative pid signals that process group; the
        // group is the one spawn_shell created for this child.
        unsafe { libc::kill(-pgid, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if matches!(child.try_wait(), Ok(Some(_))) && !group_alive(pgid) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        // SAFETY: as above.
        unsafe { libc::kill(-pgid, libc::SIGKILL) };
        let _ = child.wait();
    }
    #[cfg(windows)]
    crate::platform::close_feed_job(child.id());
    #[cfg(not(unix))]
    {
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(unix)]
pub(crate) fn group_alive(pgid: libc::pid_t) -> bool {
    // SAFETY: signal 0 only checks that the group exists.
    unsafe { libc::kill(-pgid, 0) == 0 }
}

/// `ps` output for one process: its start time (`lstart`) or command line.
#[cfg(unix)]
fn ps_field(pid: i32, field: &str) -> Option<String> {
    let out = Command::new("ps")
        .args(["-o", &format!("{field}="), "-p", &pid.to_string()])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}

/// Kills the process group a previous worker recorded, if it is still that
/// command. Its leader's start time must match; a group whose leader is gone
/// but which still has members is ours, because a live group id is never
/// handed out as a new pid.
#[cfg(unix)]
pub(crate) fn reap_leftover(status: &Status) -> bool {
    let Some(pgid) = status.pgid else {
        return false;
    };
    if pgid <= 1 || !group_alive(pgid) {
        return false;
    }
    if let Some(start) = ps_field(pgid, "lstart") {
        if status.child_start.as_deref() != Some(start.as_str()) {
            return false; // the pid was reused by something else
        }
    }
    // SAFETY: the group was checked above to be the recorded command's.
    unsafe { libc::kill(-pgid, libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && group_alive(pgid) {
        std::thread::sleep(Duration::from_millis(50));
    }
    if group_alive(pgid) {
        // SAFETY: as above.
        unsafe { libc::kill(-pgid, libc::SIGKILL) };
    }
    true
}

#[cfg(not(unix))]
pub(crate) fn reap_leftover(_status: &Status) -> bool {
    false
}

#[derive(Debug, Default)]
pub(crate) struct Capture {
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
    pub spawn_error: Option<String>,
}

impl Capture {
    fn stderr_tail(&self) -> Vec<String> {
        let text = String::from_utf8_lossy(&self.stderr);
        let lines: Vec<String> = text.lines().map(|l| clean(l.as_bytes())).collect();
        lines[lines.len().saturating_sub(STDERR_TAIL)..].to_vec()
    }
}

/// Runs `command` to completion (or `timeout`, or a stop), feeding `stdin`.
/// `started` gets the child's pid as soon as it runs.
pub(crate) fn run_capture(
    command: &str,
    cwd: &str,
    stdin: Option<&[u8]>,
    env: &[(&str, String)],
    timeout: Duration,
    stop: &AtomicBool,
    started: &dyn Fn(u32),
) -> Capture {
    let mut c = shell_command(command);
    if !cwd.is_empty() {
        c.current_dir(cwd);
    }
    for (k, v) in env {
        c.env(k, v);
    }
    c.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    #[cfg(windows)]
    crate::platform::prepare_feed_command(&mut c);
    let spawned = c.spawn();
    #[cfg(windows)]
    let spawned = spawned.and_then(|mut ch| crate::platform::attach_feed_job(&mut ch).map(|()| ch));
    let mut child = match spawned {
        Ok(ch) => ch,
        Err(e) => {
            return Capture {
                spawn_error: Some(e.to_string()),
                ..Capture::default()
            }
        }
    };
    started(child.id());
    if let (Some(data), Some(mut w)) = (stdin, child.stdin.take()) {
        let data = data.to_vec();
        std::thread::spawn(move || {
            use std::io::Write;
            let _ = w.write_all(&data);
        });
    }
    let mut out = child.stdout.take().expect("piped stdout");
    let mut err = child.stderr.take().expect("piped stderr");
    let ho = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out.read_to_end(&mut b);
        b
    });
    let he = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err.read_to_end(&mut b);
        b
    });
    let deadline = Instant::now() + timeout;
    let mut cap = Capture::default();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                cap.code = status.code();
                break;
            }
            Ok(None) => {}
            Err(_) => break,
        }
        if Instant::now() >= deadline || stop.load(Ordering::SeqCst) {
            cap.timed_out = Instant::now() >= deadline;
            kill_tree(&mut child);
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // Whatever the shell left behind in its group goes too, so the pipes close.
    #[cfg(unix)]
    {
        let pgid = child.id() as libc::pid_t;
        if group_alive(pgid) {
            // SAFETY: the group spawn created for this run.
            unsafe { libc::kill(-pgid, libc::SIGKILL) };
        }
    }
    #[cfg(windows)]
    crate::platform::close_feed_job(child.id());
    cap.stdout = ho.join().unwrap_or_default();
    cap.stderr = he.join().unwrap_or_default();
    cap
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The worker log, created 0600 like every other feed file (it holds
/// command output).
#[cfg(unix)]
fn open_log(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}

// ---------------------------------------------------------------- agents

#[derive(Debug, Clone, PartialEq)]
enum AgentState {
    /// Not looked at yet.
    Unknown,
    Idle(String),
    Busy(String),
    /// Its server is up, the agent is not there (or its name is ambiguous).
    Away(String),
    /// Its server is not running.
    Down,
}

impl AgentState {
    fn reachable(&self) -> bool {
        matches!(self, AgentState::Idle(_) | AgentState::Busy(_))
    }
}

fn agent_state(list: &Result<Value, String>, agent: &str) -> AgentState {
    let Ok(list) = list else {
        return AgentState::Down;
    };
    let agents: Vec<&Value> = list
        .get("agents")
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    let s = |a: &Value, k: &str| a.get(k).and_then(Value::as_str).map(str::to_string);
    let by_name: Vec<&&Value> = agents
        .iter()
        .filter(|a| s(a, "name").as_deref() == Some(agent))
        .collect();
    let found = match by_name.as_slice() {
        [one] => Some(**one),
        [] => agents
            .iter()
            .copied()
            .find(|a| s(a, "pane_id").as_deref() == Some(agent)),
        _ => {
            return AgentState::Away(format!(
                "{} agents are named {agent}; rename one",
                by_name.len()
            ))
        }
    };
    let Some(a) = found else {
        return AgentState::Away("agent not found".into());
    };
    let pane = s(a, "pane_id").unwrap_or_default();
    match s(a, "agent_status").as_deref() {
        // `unknown`: klark cannot read this agent's state (the sidebar shows
        // it as idle). Waiting on it would never deliver.
        Some("idle") | Some("done") | Some("unknown") | None => AgentState::Idle(pane),
        Some(_) => AgentState::Busy(pane),
    }
}

// ---------------------------------------------------------------- worker

/// One subscription at run time.
struct SubRt {
    n: u32,
    session: String,
    agent: String,
    instruction: String,
    queue: Queue,
    state: AgentState,
    last_sent: Option<u64>,
    oldest_pending: Option<u64>,
    last_output: u64,
    /// The notice that stops this subscription is queued: (reason, user only).
    stopping: Option<(String, bool)>,
    /// Bytes appended to the pending file since it was last rewritten.
    on_disk: usize,
}

enum Ev {
    Out(Vec<u8>),
    Err(Vec<u8>),
    OutClosed,
}

enum PollMsg {
    Started(u32),
    Done(Capture),
}

struct Stream {
    child: Child,
    rx: mpsc::Receiver<Ev>,
    out: LineSplitter,
    err: LineSplitter,
    stderr_tail: VecDeque<String>,
    partial_since: Option<u64>,
    out_closed: bool,
}

pub(crate) struct Worker {
    store: Store,
    id: String,
    def: FeedDef,
    status: Status,
    subs: Vec<SubRt>,
    stop: Arc<AtomicBool>,
    background: bool,
    start: Instant,
    stream: Option<Stream>,
    follower: Option<Follower>,
    poll_rx: Option<mpsc::Receiver<PollMsg>>,
    /// Stops the poll run in flight (a self-stop, not only a signal).
    poll_kill: Option<Arc<AtomicBool>>,
    next_poll: u64,
    /// Why the whole feed is ending, once its last notices are queued.
    stopping: Option<(String, u64)>,
    last_output: u64,
    stale_sent: bool,
    status_dirty: bool,
    filter_cache: VecDeque<(u64, Answer)>,
    last_tick: u64,
    /// A resumed worker does not end for servers that are down before this
    /// (ms since start), unless an agent was reachable meanwhile.
    grace_ms: u64,
    /// Since when every subscriber's server has looked down.
    down_since: Option<u64>,
}

pub(crate) enum Outcome {
    /// Ran until a stop, the end of the source or an automatic stop.
    Done,
    /// Another worker holds the lock.
    AlreadyRunning,
}

fn hash(lines: &[String]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    lines.hash(&mut h);
    h.finish()
}

/// Runs feed `id` until it ends. `background`: stdio is the feed log (rotated
/// here). Errors that keep a new feed from starting are returned and also
/// written to the status (`state = failed`).
pub(crate) fn run(
    store: Store,
    id: &str,
    background: bool,
    resumed: bool,
) -> Result<Outcome, String> {
    if resumed {
        let terminal = crate::config::Config::load().config.terminal;
        let _ = PANE_SHELL.set((terminal.default_shell, terminal.shell_mode));
    }
    // Before anything is spawned: a stop that arrives now must reach the
    // handlers, not kill the worker and leave the command running.
    let stop = Arc::new(AtomicBool::new(false));
    #[cfg(unix)]
    for sig in [
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
    ] {
        let _ = signal_hook::flag::register(sig, stop.clone());
    }
    #[cfg(not(unix))]
    {
        let s = stop.clone();
        let _ = ctrlc::set_handler(move || s.store(true, Ordering::SeqCst));
    }

    // Patient: `feed list` and friends take the lock for a moment to see
    // whether a worker runs; that must not turn a new worker away.
    let Some(_lock) = store
        .try_lock_feed_for(id, Duration::from_secs(2))
        .map_err(|e| format!("feed lock: {e}"))?
    else {
        return Ok(Outcome::AlreadyRunning);
    };
    let Some(def) = store.read_json::<FeedDef>(&def_name(id))? else {
        return Err(format!("no feed {id}"));
    };
    let mut status: Status = store
        .read_json(&status_name(id))
        .ok()
        .flatten()
        .unwrap_or_default();
    // Our pid first: `feed stop` signals the pid in the status while the lock
    // is held, and reaping below can take seconds.
    status.worker_pid = Some(std::process::id());
    let _ = store.write_json(&status_name(id), &status);
    if reap_leftover(&status) {
        eprintln!(
            "klark feed {id}: killed a command left behind by an earlier worker (group {})",
            status.pgid.unwrap_or(0)
        );
    }
    status.pgid = None;
    status.child_start = None;
    status.child_cmd = None;
    status.state = "starting".into();
    status.detail = None;
    status.worker_pid = Some(std::process::id());
    let mut w = Worker {
        store,
        id: id.to_string(),
        def,
        status,
        subs: Vec::new(),
        stop,
        background,
        start: Instant::now(),
        stream: None,
        follower: None,
        poll_rx: None,
        poll_kill: None,
        next_poll: 0,
        stopping: None,
        last_output: 0,
        stale_sent: false,
        status_dirty: true,
        filter_cache: VecDeque::new(),
        last_tick: 0,
        grace_ms: if resumed { RESUME_GRACE_MS } else { 0 },
        down_since: None,
    };
    w.write_status();
    w.sync_subs();
    match w.begin() {
        Ok(()) => {}
        Err(e) => {
            w.status.state = "failed".into();
            w.status.detail = Some(e.clone());
            w.write_status();
            return Err(e);
        }
    }
    w.status.state = "running".into();
    w.write_status();
    eprintln!(
        "klark feed {}: running ({} {})",
        w.id,
        w.def.mode.as_str(),
        w.def.label
    );
    w.main_loop().map(|_| Outcome::Done)
}

impl Worker {
    fn now(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn gap_ms(&self) -> u64 {
        self.def.timing_secs * 1000
    }

    fn write_status(&mut self) {
        // A feed that was stopped (`feed stop` deleted it) leaves no files.
        if !self.store.path(&def_name(&self.id)).exists() {
            return;
        }
        if let Err(e) = self.store.write_json(&status_name(&self.id), &self.status) {
            eprintln!("klark feed {}: {e}", self.id);
        }
        self.status_dirty = false;
    }

    fn frame<'a>(&'a self, instruction: &'a str) -> Frame<'a> {
        Frame {
            id: &self.id,
            label: &self.def.label,
            instruction,
            use_small_subagent: self.def.use_small_subagent,
        }
    }

    /// Changes the definition under the store lock. `false` when the feed
    /// is gone (stopped).
    fn edit_def(&mut self, f: impl FnOnce(&mut FeedDef)) -> bool {
        let Ok(_g) = self.store.lock_store() else {
            return true;
        };
        match self.store.read_json::<FeedDef>(&def_name(&self.id)) {
            Ok(Some(mut d)) => {
                f(&mut d);
                if let Err(e) = self.store.write_json(&def_name(&self.id), &d) {
                    eprintln!("klark feed {}: {e}", self.id);
                }
                self.def = d;
                true
            }
            _ => false,
        }
    }

    /// The subscriber list follows the definition.
    fn sync_subs(&mut self) {
        let active: Vec<_> = self.def.active_subs().cloned().collect();
        // One that left (`feed leave`, `agent remove`) may have had lines
        // queued since the CLI removed its pending file: remove it again.
        for s in &self.subs {
            if !self.def.subs.iter().any(|d| d.n == s.n) {
                let _ = self.store.remove(&pending_name(&self.id, s.n));
            }
        }
        self.subs.retain(|s| active.iter().any(|a| a.n == s.n));
        for a in active {
            if let Some(s) = self.subs.iter_mut().find(|s| s.n == a.n) {
                s.instruction = a.instruction.clone();
                continue;
            }
            let mut queue = Queue::new(PENDING_CAP_BYTES);
            let raw = self
                .store
                .read(&pending_name(&self.id, a.n))
                .ok()
                .flatten()
                .unwrap_or_default();
            for line in String::from_utf8_lossy(&raw).lines() {
                match serde_json::from_str::<Value>(line) {
                    Ok(v) => {
                        if let Some(n) = v.get("n").and_then(Value::as_str) {
                            queue.push_notice(n.to_string());
                        } else if let Some(l) = v.get("l").and_then(Value::as_str) {
                            queue.push_line(l.to_string());
                        }
                    }
                    Err(_) => continue,
                }
            }
            let t = self.now();
            self.subs.push(SubRt {
                n: a.n,
                session: a.session.clone(),
                agent: a.agent.clone(),
                instruction: a.instruction.clone(),
                oldest_pending: queue.has_lines().then_some(t),
                queue,
                state: AgentState::Unknown,
                last_sent: None,
                last_output: 0,
                stopping: None,
                on_disk: raw.len(),
            });
        }
    }

    /// Queues lines for every live subscription, on disk first.
    fn push_lines(&mut self, lines: Vec<String>, only: Option<&[u32]>) {
        if lines.is_empty() {
            return;
        }
        let t = self.now();
        let body: String = lines
            .iter()
            .map(|l| json!({ "l": l }).to_string() + "\n")
            .collect();
        for i in 0..self.subs.len() {
            let s = &self.subs[i];
            if s.stopping.is_some() || only.is_some_and(|o| !o.contains(&s.n)) {
                continue;
            }
            let name = pending_name(&self.id, s.n);
            if let Err(e) = self.store.append(&name, body.as_bytes()) {
                eprintln!("klark feed {}: {name}: {e}", self.id);
            }
            let s = &mut self.subs[i];
            s.on_disk += body.len();
            for l in &lines {
                s.queue.push_line(l.clone());
            }
            s.oldest_pending.get_or_insert(t);
            s.last_output = t;
            if s.on_disk > 2 * PENDING_CAP_BYTES {
                self.rewrite_pending(i);
            }
        }
    }

    fn push_notice_to(&mut self, i: usize, notice: &str) {
        let name = pending_name(&self.id, self.subs[i].n);
        let body = json!({ "n": notice }).to_string() + "\n";
        if let Err(e) = self.store.append(&name, body.as_bytes()) {
            eprintln!("klark feed {}: {name}: {e}", self.id);
        }
        self.subs[i].on_disk += body.len();
        self.subs[i].queue.push_notice(notice.to_string());
    }

    fn push_notice(&mut self, notice: &str) {
        for i in 0..self.subs.len() {
            if self.subs[i].stopping.is_none() {
                self.push_notice_to(i, notice);
            }
        }
    }

    /// The pending file holds exactly what the queue holds.
    fn rewrite_pending(&mut self, i: usize) {
        let (notices, lines) = self.subs[i].queue.snapshot();
        let mut body = String::new();
        for n in notices {
            body.push_str(&(json!({ "n": n }).to_string() + "\n"));
        }
        for l in lines {
            body.push_str(&(json!({ "l": l }).to_string() + "\n"));
        }
        let name = pending_name(&self.id, self.subs[i].n);
        if let Err(e) = self.store.write(&name, body.as_bytes()) {
            eprintln!("klark feed {}: {name}: {e}", self.id);
        }
        self.subs[i].on_disk = body.len();
    }

    // ------------------------------------------------------------ sources

    /// Starts the source. An error here means the feed cannot start (a
    /// command that died at once, a failing first poll, an unreadable file).
    fn begin(&mut self) -> Result<(), String> {
        match self.def.mode {
            Mode::Stream => {
                self.start_stream()?;
                // A command that dies at once (a typo, exit 127) is the
                // caller's mistake: say so, send nothing.
                let first = Instant::now();
                while first.elapsed() < Duration::from_secs(1) {
                    let st = self.stream.as_mut().expect("stream");
                    if let Ok(Some(status)) = st.child.try_wait() {
                        if status.success() {
                            break; // reported like any stream end
                        }
                        let mut stderr = Vec::new();
                        while let Ok(ev) = st.rx.recv_timeout(Duration::from_millis(300)) {
                            if let Ev::Err(b) = ev {
                                stderr.extend(b);
                            }
                        }
                        self.stream = None;
                        self.clear_child();
                        return Err(format!(
                            "the command failed at once ({status}); no feed was started\n{}",
                            String::from_utf8_lossy(&stderr).trim_end()
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Ok(())
            }
            Mode::Follow => {
                let path = PathBuf::from(&self.def.command);
                let mut f = Follower::new(path, self.status.follow.clone());
                let step = f.start();
                if let Some(e) = step.error {
                    return Err(format!("{}: {e}", self.def.command));
                }
                self.status.follow = f.pos.clone();
                if step.skipped > 0 {
                    // A notice, outside the pending cap: the catch-up that
                    // follows fills the cap and would push a line out.
                    self.push_notice(&format!(
                        "(… {} bytes written while klark was not running were skipped)",
                        step.skipped
                    ));
                }
                self.follower = Some(f);
                Ok(())
            }
            Mode::Poll => {
                // The first run happens now, so a broken command is reported
                // to whoever started the feed.
                let id = self.id.clone();
                let cap = run_capture(
                    &self.def.command,
                    &self.def.cwd,
                    None,
                    &[],
                    poll_timeout(self.def.timing_secs),
                    &self.stop,
                    &|pid| eprintln!("klark feed {id}: first run (pid {pid})"),
                );
                if let Some(e) = cap.spawn_error {
                    return Err(format!("starting {}: {e}", self.def.command));
                }
                if cap.code != Some(0) {
                    return Err(format!(
                        "the command failed at once ({}); no feed was started\n{}",
                        match cap.code {
                            Some(c) => format!("exit code {c}"),
                            None if cap.timed_out => "timed out".into(),
                            None => "killed".into(),
                        },
                        cap.stderr_tail().join("\n")
                    ));
                }
                self.on_poll(cap);
                self.next_poll = self.now() + self.gap_ms();
                Ok(())
            }
        }
    }

    fn clear_child(&mut self) {
        self.status.pgid = None;
        self.status.child_start = None;
        self.status.child_cmd = None;
        self.status_dirty = true;
    }

    fn record_child(&mut self, pid: u32) {
        self.status.pgid = Some(pid as i32);
        #[cfg(unix)]
        {
            self.status.child_start = ps_field(pid as i32, "lstart");
            self.status.child_cmd = ps_field(pid as i32, "command");
        }
        self.write_status();
    }

    fn start_stream(&mut self) -> Result<(), String> {
        let mut child = spawn_shell(&self.def.command, &self.def.cwd, false)
            .map_err(|e| format!("starting {}: {e}", self.def.command))?;
        self.record_child(child.id());
        // Bounded: while the worker is held up (a slow filter, a prompt), a
        // flooding command waits on its pipe instead of filling memory.
        let (tx, rx) = mpsc::sync_channel(STREAM_CHUNKS);
        let mut out = child.stdout.take().expect("piped stdout");
        let mut err = child.stderr.take().expect("piped stderr");
        let tx_out = tx.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = out.read(&mut buf) {
                if n == 0 || tx_out.send(Ev::Out(buf[..n].to_vec())).is_err() {
                    break;
                }
            }
            let _ = tx_out.send(Ev::OutClosed);
        });
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = err.read(&mut buf) {
                if n == 0 || tx.send(Ev::Err(buf[..n].to_vec())).is_err() {
                    break;
                }
            }
        });
        self.stream = Some(Stream {
            child,
            rx,
            out: LineSplitter::default(),
            err: LineSplitter::default(),
            stderr_tail: VecDeque::new(),
            partial_since: None,
            out_closed: false,
        });
        Ok(())
    }

    fn kill_command(&mut self) {
        if let Some(mut st) = self.stream.take() {
            kill_tree(&mut st.child);
            self.clear_child();
        }
        // A poll run in flight goes too (it is not a failure: nothing is
        // reported), and is waited for, so the worker never exits before it.
        if let Some(kill) = self.poll_kill.take() {
            kill.store(true, Ordering::SeqCst);
            if let Some(rx) = self.poll_rx.take() {
                let deadline = Instant::now() + Duration::from_secs(5);
                loop {
                    let left = deadline.saturating_duration_since(Instant::now());
                    match rx.recv_timeout(left) {
                        Ok(PollMsg::Started(_)) => continue,
                        Ok(PollMsg::Done(_)) | Err(_) => break,
                    }
                }
            }
            self.clear_child();
        }
    }

    fn output_seen(&mut self) {
        self.last_output = self.now();
        self.status.last_output = Some(unix_secs());
        self.stale_sent = false;
        self.status_dirty = true;
    }

    /// Takes what the stream printed. Returns false when there was nothing
    /// to wait on (the caller sleeps instead).
    fn pump_stream(&mut self) -> bool {
        let start = self.start;
        let Some(st) = self.stream.as_mut() else {
            return false;
        };
        let mut events = Vec::new();
        if st.out_closed {
            std::thread::sleep(Duration::from_millis(200));
        } else if let Ok(ev) = st.rx.recv_timeout(Duration::from_millis(200)) {
            events.push(ev);
        }
        // Only what is already there, so a flood cannot starve delivery.
        while let Ok(ev) = st.rx.try_recv() {
            events.push(ev);
        }
        let t = start.elapsed().as_millis() as u64;
        let mut lines = Vec::new();
        let mut got = false;
        for ev in events {
            match ev {
                Ev::Out(bytes) => {
                    got = true;
                    lines.extend(st.out.push(&bytes));
                    st.partial_since = st.out.has_partial().then(|| st.partial_since.unwrap_or(t));
                }
                Ev::Err(bytes) => {
                    for line in st.err.push(&bytes) {
                        st.stderr_tail.push_back(line);
                        if st.stderr_tail.len() > STDERR_TAIL {
                            st.stderr_tail.pop_front();
                        }
                    }
                }
                Ev::OutClosed => st.out_closed = true,
            }
        }
        if st
            .partial_since
            .is_some_and(|p| st.out_closed || t.saturating_sub(p) >= PARTIAL_MS)
        {
            lines.extend(st.out.take_partial());
            st.partial_since = None;
        }

        // The command ended: one notice, whatever the exit code.
        let mut ended = None;
        if let Ok(Some(status)) = st.child.try_wait() {
            // Let the readers finish so no last line is lost.
            while !st.out_closed {
                match st.rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(Ev::Out(bytes)) => {
                        got = true;
                        lines.extend(st.out.push(&bytes));
                    }
                    Ok(Ev::Err(bytes)) => {
                        for line in st.err.push(&bytes) {
                            st.stderr_tail.push_back(line);
                        }
                    }
                    Ok(Ev::OutClosed) | Err(_) => st.out_closed = true,
                }
            }
            lines.extend(st.out.take_partial());
            if let Some(line) = st.err.take_partial() {
                st.stderr_tail.push_back(line);
            }
            while st.stderr_tail.len() > STDERR_TAIL {
                st.stderr_tail.pop_front();
            }
            let tail: Vec<String> = st.stderr_tail.iter().cloned().collect();
            ended = Some((status.code(), tail));
        }
        if got {
            self.output_seen();
        }
        self.push_lines(lines, None);
        if let Some((code, tail)) = ended {
            self.kill_command(); // the rest of a pipeline
            let notice = message::ended(&self.id, &self.def.label, code, &tail);
            self.push_notice(&notice);
            let why = match code {
                Some(0) => "ended: exited normally".to_string(),
                Some(c) => format!("ended: exited with code {c}"),
                None => "ended: killed by a signal".to_string(),
            };
            self.stopping = Some((why, t));
        }
        true
    }

    fn pump_follow(&mut self) {
        let Some(f) = self.follower.as_mut() else {
            return;
        };
        let step = f.step(PARTIAL_MS);
        let pos = f.pos.clone();
        // Bytes come before an error of the same step (the end of a rotated
        // file, then a new one that cannot be read): the offset moved past them.
        if !step.bytes.is_empty() {
            let mut sp = LineSplitter::default();
            let lines = sp.push(&step.bytes);
            self.output_seen();
            self.push_lines(lines, None);
        }
        if pos != self.status.follow {
            // After the lines are on disk: a crash in between sends them
            // again (at least once), never loses them.
            self.status.follow = pos;
            self.write_status();
        }
        if let Some(e) = step.error {
            if self.status.errors.failed(-2, unix_ms()) {
                let notice = format!("error: {} cannot be read: {e}", self.def.command);
                self.push_notice(&notice);
            }
            self.status_dirty = true;
            return;
        }
        if self.status.errors.succeeded() {
            let notice = message::recovered(&self.def.label);
            self.push_notice(&notice);
        }
    }

    fn start_poll(&mut self) {
        let (tx, rx) = mpsc::channel();
        let command = self.def.command.clone();
        let cwd = self.def.cwd.clone();
        let timeout = poll_timeout(self.def.timing_secs);
        // Every way the worker ends goes through kill_command, which sets
        // this flag (a signal included), so the run never outlives it.
        let kill = Arc::new(AtomicBool::new(false));
        let k = kill.clone();
        std::thread::spawn(move || {
            let txs = tx.clone();
            let cap = run_capture(&command, &cwd, None, &[], timeout, &k, &|pid| {
                let _ = txs.send(PollMsg::Started(pid));
            });
            let _ = tx.send(PollMsg::Done(cap));
        });
        self.poll_rx = Some(rx);
        self.poll_kill = Some(kill);
    }

    fn pump_poll(&mut self) {
        let Some(rx) = self.poll_rx.as_ref() else {
            return;
        };
        let mut done = None;
        let mut started = None;
        while let Ok(m) = rx.try_recv() {
            match m {
                PollMsg::Started(pid) => started = Some(pid),
                PollMsg::Done(cap) => done = Some(cap),
            }
        }
        if let Some(pid) = started {
            self.record_child(pid);
        }
        if let Some(cap) = done {
            self.poll_rx = None;
            self.poll_kill = None;
            self.clear_child();
            // The next run is an interval after this one finished.
            self.next_poll = self.now() + self.gap_ms();
            self.on_poll(cap);
        }
    }

    /// `prev`: `None` on the first run (no previous output recorded yet).
    fn diff_with(
        &mut self,
        prev: Option<&str>,
        curr: &str,
    ) -> Result<Vec<String>, (Option<i32>, String)> {
        match self.def.diff.as_str() {
            "lines" => Ok(crate::feed::diff::lines(prev.unwrap_or(""), curr)),
            "unified" => {
                // No previous output is a first run: record only. An empty
                // one is not: output that comes back after going empty is new.
                Ok(crate::feed::diff::unified(prev, curr)
                    .map(|d| d.lines().map(str::to_string).collect())
                    .unwrap_or_default())
            }
            cmd => {
                let pf = format!("{}.diff-prev", self.id);
                let cf = format!("{}.diff-curr", self.id);
                let write = |n: &str, b: &str| self.store.write(n, b.as_bytes());
                let prev = prev.unwrap_or("");
                if let Err(e) = write(&pf, prev).and_then(|_| write(&cf, curr)) {
                    return Err((None, e.to_string()));
                }
                let (pp, cp) = (self.store.path(&pf), self.store.path(&cf));
                let line = format!(
                    "{cmd} {} {}",
                    shell_quote(&pp.display().to_string()),
                    shell_quote(&cp.display().to_string())
                );
                let cap = run_capture(
                    &line,
                    &self.def.cwd,
                    None,
                    &[
                        ("KLARK_FEED_PREV", pp.display().to_string()),
                        ("KLARK_FEED_CURR", cp.display().to_string()),
                        ("KLARK_FEED_ID", self.id.clone()),
                    ],
                    poll_timeout(self.def.timing_secs),
                    &self.stop,
                    &|_| {},
                );
                if cap.code != Some(0) {
                    let mut tail = cap.stderr_tail();
                    tail.insert(0, "(the --diff command failed)".into());
                    return Err((cap.code, tail.join("\n")));
                }
                Ok(String::from_utf8_lossy(&cap.stdout)
                    .lines()
                    .map(str::to_string)
                    .collect())
            }
        }
    }

    fn on_poll(&mut self, cap: Capture) {
        if cap.code != Some(0) {
            if self.stop.load(Ordering::SeqCst) {
                return; // killed by our own stop, not a failure
            }
            let code = cap.code.unwrap_or(-1);
            if self.status.errors.failed(code, unix_ms()) {
                let mut tail = cap.stderr_tail();
                if cap.timed_out {
                    tail.insert(
                        0,
                        format!(
                            "(killed after {} s without finishing)",
                            poll_timeout(self.def.timing_secs).as_secs()
                        ),
                    );
                }
                let notice = message::run_failed(&self.def.label, cap.code, &tail);
                self.push_notice(&notice);
            }
            self.status_dirty = true;
            return;
        }
        let curr = String::from_utf8_lossy(&cap.stdout).to_string();
        let prev_name = format!("{}.prev", self.id);
        let prev = self
            .store
            .read(&prev_name)
            .ok()
            .flatten()
            .map(|b| String::from_utf8_lossy(&b).to_string());
        let new = match self.diff_with(prev.as_deref(), &curr) {
            Ok(l) => l,
            Err((code, detail)) => {
                // Reported like a failed run; the previous file is kept.
                if self.status.errors.failed(code.unwrap_or(-3), unix_ms()) {
                    let notice = format!("error: --diff failed for {}.\n{detail}", self.def.label);
                    self.push_notice(&notice);
                }
                self.status_dirty = true;
                return;
            }
        };
        if self.status.errors.succeeded() {
            let notice = message::recovered(&self.def.label);
            self.push_notice(&notice);
            self.status_dirty = true;
        }
        // A joiner gets the current output once: diff(empty, current). Only
        // one this worker has picked up already; one added since the last
        // read of the definition stays fresh for the next run.
        let fresh: Vec<u32> = self
            .def
            .subs
            .iter()
            .filter(|s| s.fresh && s.stopped.is_none())
            .map(|s| s.n)
            .filter(|n| self.subs.iter().any(|r| r.n == *n))
            .collect();
        let rest: Vec<u32> = self
            .subs
            .iter()
            .map(|s| s.n)
            .filter(|n| !fresh.contains(n))
            .collect();
        let clean_lines =
            |v: Vec<String>| -> Vec<String> { v.iter().map(|l| clean(l.as_bytes())).collect() };
        let new = clean_lines(new);
        if !new.is_empty() {
            self.output_seen();
        }
        self.push_lines(new, Some(&rest));
        if !fresh.is_empty() {
            let first = match self.def.diff.as_str() {
                "unified" => crate::feed::diff::unified(Some(""), &curr)
                    .map(|d| d.lines().map(str::to_string).collect())
                    .unwrap_or_default(),
                _ => self.diff_with(Some(""), &curr).unwrap_or_default(),
            };
            let first = clean_lines(first);
            self.push_lines(first, Some(&fresh));
        }
        // Only now that the new lines are in the pending files: a crash
        // before this sends them again (at least once), never loses them.
        if let Err(e) = self.store.write(&prev_name, curr.as_bytes()) {
            eprintln!("klark feed {}: {prev_name}: {e}", self.id);
        }
        if !fresh.is_empty() {
            self.edit_def(|d| {
                for s in d.subs.iter_mut() {
                    if fresh.contains(&s.n) {
                        s.fresh = false;
                    }
                }
            });
        }
    }

    // ------------------------------------------------------------ agents

    fn check_agents(&mut self) {
        let mut lists: HashMap<String, Result<Value, String>> = HashMap::new();
        for i in 0..self.subs.len() {
            let session = self.subs[i].session.clone();
            let list = lists
                .entry(session.clone())
                .or_insert_with(|| call(&session_socket(&session), "agent.list", json!({})));
            let state = agent_state(list, &self.subs[i].agent);
            let n = self.subs[i].n;
            let entry = self.status.subs.entry(n).or_default();
            let waiting = match &state {
                AgentState::Away(why) => Some(why.clone()),
                AgentState::Down => Some(format!("klark session {session} is not running")),
                _ => None,
            };
            let busy_since = match &state {
                AgentState::Busy(_) => Some(entry.busy_since.unwrap_or_else(unix_secs)),
                _ => None,
            };
            if entry.waiting != waiting || entry.busy_since != busy_since {
                entry.waiting = waiting;
                entry.busy_since = busy_since;
                self.status_dirty = true;
            }
            if state != self.subs[i].state {
                if let AgentState::Away(why) = &state {
                    eprintln!(
                        "klark feed {}: {}/{} waits: {why}",
                        self.id, session, self.subs[i].agent
                    );
                }
            }
            self.subs[i].state = state;
        }
    }

    /// Asks the filter about a batch (once per distinct batch).
    fn filter_answer(&mut self, lines: &[String]) -> (Answer, Option<String>) {
        let Some(f) = self.def.filter.clone() else {
            return (Answer::Send, None);
        };
        let h = hash(lines);
        if let Some((_, a)) = self.filter_cache.iter().find(|(k, _)| *k == h) {
            return (*a, None);
        }
        let body = lines.join("\n") + "\n";
        let cap = run_capture(
            &f,
            &self.def.cwd,
            Some(body.as_bytes()),
            &[
                ("KLARK_FEED_ID", self.id.clone()),
                ("KLARK_FEED_LABEL", self.def.label.clone()),
            ],
            filter_timeout(),
            &self.stop,
            &|_| {},
        );
        let (answer, detail) = if cap.timed_out {
            (
                Answer::Error,
                format!(
                    "the filter ran longer than {} s",
                    filter_timeout().as_secs()
                ),
            )
        } else if let Some(e) = &cap.spawn_error {
            (Answer::Error, format!("the filter could not start: {e}"))
        } else {
            let a = filter::answer(&String::from_utf8_lossy(&cap.stdout), cap.code);
            let detail = match cap.code {
                Some(c) => format!("the filter exited with code {c}"),
                None => "the filter was killed".into(),
            };
            (a, detail)
        };
        self.filter_cache.push_back((h, answer));
        if self.filter_cache.len() > 16 {
            self.filter_cache.pop_front();
        }
        if answer == Answer::Error {
            let code = cap.code.unwrap_or(-1);
            if self.status.filter_errors.failed(code, unix_ms()) {
                self.status_dirty = true;
                return (answer, Some(message::filter_failed(&detail)));
            }
        } else {
            self.status.filter_errors.succeeded();
        }
        (answer, None)
    }

    fn deliver(&mut self, i: usize, t: u64) {
        let sub = &self.subs[i];
        let AgentState::Idle(pane) = sub.state.clone() else {
            return;
        };
        if sub.queue.is_empty() {
            return;
        }
        let urgent = self.stopping.is_some()
            || sub.stopping.is_some()
            || (sub.queue.has_notices() && !sub.queue.has_lines());
        let due = urgent
            || SendCheck {
                pending: sub.queue.has_lines(),
                agent_idle: true,
                now: t,
                last_sent: sub.last_sent,
                gap_ms: self.gap_ms(),
                last_output: sub.last_output,
                quiet_ms: QUIET_MS,
                oldest_pending: sub.oldest_pending.unwrap_or(t),
            }
            .due();
        if !due {
            return;
        }
        let mut batch = self.subs[i].queue.take(self.def.max_bytes);
        if batch.is_empty() {
            return;
        }
        if !batch.all.is_empty() {
            let (answer, notice) = self.filter_answer(&batch.all.clone());
            if let Some(n) = notice {
                batch.notices.push(n);
            }
            if answer == Answer::Drop {
                self.status.filtered += 1;
                self.status_dirty = true;
                eprintln!(
                    "klark feed {}: the filter dropped {} line(s)",
                    self.id,
                    batch.all.len()
                );
                batch = Batch {
                    notices: batch.notices,
                    shown: Vec::new(),
                    all: Vec::new(),
                    dropped: 0,
                    cut: false,
                };
                self.subs[i].oldest_pending = None;
                if batch.notices.is_empty() {
                    self.rewrite_pending(i);
                    return;
                }
            }
        }
        // The number is taken only once the message is out: a held batch
        // (someone typing, retried every few hundred ms) must not use up
        // numbers and push out the batches earlier messages point to.
        let batch_no = if batch.cut {
            let n = self.status.batch_seq + 1;
            let body = batch.all.join("\n") + "\n";
            if let Err(e) = self
                .store
                .write(&format!("{}.batch-{n}", self.id), body.as_bytes())
            {
                eprintln!("klark feed {}: saving batch {n}: {e}", self.id);
            }
            Some(n)
        } else {
            None
        };
        let text = message::render(&self.frame(&self.subs[i].instruction), &batch, batch_no);
        let session = self.subs[i].session.clone();
        // Never wait on someone typing: hold the batch and try again later.
        match crate::msg_cmd::prompt_agent(&session, &pane, &text, Duration::ZERO) {
            Ok(_) => {
                if let Some(n) = batch_no {
                    self.status.batch_seq = n;
                    if n > 5 {
                        let _ = self.store.remove(&format!("{}.batch-{}", self.id, n - 5));
                    }
                }
                let had_lines = !batch.all.is_empty();
                // Only id, agent and size are logged, never the text.
                eprintln!(
                    "klark feed {}: sent {} bytes ({} line(s), {} notice(s)) to {}/{}",
                    self.id,
                    text.len(),
                    batch.all.len(),
                    batch.notices.len(),
                    session,
                    self.subs[i].agent
                );
                self.rewrite_pending(i);
                let n = self.subs[i].n;
                let sub = &mut self.subs[i];
                sub.last_sent = Some(t);
                sub.oldest_pending = None;
                // It is working on that now; look again before the next one.
                sub.state = AgentState::Busy(pane);
                let now_s = unix_secs();
                self.status.last_sent = Some(now_s);
                let ss = self.status.subs.entry(n).or_default();
                ss.last_sent = Some(now_s);
                if had_lines {
                    self.status.idle.delivered();
                }
                self.status_dirty = true;
                // Notices count too, so error / recovered flapping is
                // bounded; the stop notice itself does not.
                if self.subs[i].stopping.is_none() {
                    let max = self.def.max_per_hour;
                    let ss: &mut SubStatus = self.status.subs.entry(n).or_default();
                    if let Rate::Tripped { count, user_only } = ss.rate.record(unix_ms(), max) {
                        let notice = message::stopped_rate(&self.id, count, user_only);
                        self.subs[i].queue.take(usize::MAX); // what is left is not sent
                        self.rewrite_pending(i);
                        self.push_notice_to(i, &notice);
                        self.subs[i].stopping = Some((
                            format!("too many updates ({count} in the last hour)"),
                            user_only,
                        ));
                        eprintln!(
                            "klark feed {}: {}/{} stopped: too many updates",
                            self.id, session, self.subs[i].agent
                        );
                    }
                }
            }
            Err(e) => {
                if !e.contains("agent_user_typing") {
                    eprintln!("klark feed {}: not delivered yet: {e}", self.id);
                }
                self.subs[i].queue.put_back(batch);
            }
        }
    }

    /// A subscription whose stop notice went out is stopped in the definition.
    fn finish_stopped_subs(&mut self) {
        let done: Vec<(u32, String, bool)> = self
            .subs
            .iter()
            .filter(|s| s.queue.is_empty())
            .filter_map(|s| s.stopping.clone().map(|(r, u)| (s.n, r, u)))
            .collect();
        if done.is_empty() {
            return;
        }
        self.edit_def(|d| {
            for s in d.subs.iter_mut() {
                if let Some((_, r, u)) = done.iter().find(|(n, _, _)| *n == s.n) {
                    s.stopped = Some(r.clone());
                    s.user_only = *u;
                }
            }
        });
        for (n, _, _) in &done {
            let _ = self.store.remove(&pending_name(&self.id, *n));
        }
        self.sync_subs();
    }

    fn rotate_log(&self) {
        if !self.background {
            return;
        }
        let path = self.store.path(&log_name(&self.id));
        if std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0) <= LOG_CAP {
            return;
        }
        let _ = std::fs::rename(&path, self.store.path(&format!("{}.log.1", self.id)));
        #[cfg(unix)]
        if let Ok(f) = open_log(&path) {
            use std::os::unix::io::AsRawFd;
            // SAFETY: dup2 onto our own stdout/stderr; `f` stays open until
            // both are replaced.
            unsafe {
                libc::dup2(f.as_raw_fd(), 1);
                libc::dup2(f.as_raw_fd(), 2);
            }
        }
    }

    fn main_loop(&mut self) -> Result<(), String> {
        let mut last_check: Option<u64> = None;
        let mut last_def: u64 = 0;
        let mut last_rotate: u64 = 0;
        loop {
            if self.stop.load(Ordering::SeqCst) {
                self.kill_command();
                self.status.state = "stopped".into();
                self.status.detail = Some("the worker was stopped".into());
                self.status.worker_pid = None;
                self.write_status();
                eprintln!("klark feed {}: stopped", self.id);
                return Ok(());
            }
            let t = self.now();

            // Once a second: the subscriber list (from the definition).
            if t.saturating_sub(last_def) >= 1_000 {
                last_def = t;
                match self.store.read_json::<FeedDef>(&def_name(&self.id)) {
                    Ok(Some(d)) => {
                        self.def = d;
                        self.sync_subs();
                    }
                    Ok(None) => {
                        // `feed stop` (or the last agent leaving) deleted it.
                        self.kill_command();
                        eprintln!("klark feed {}: removed, ending", self.id);
                        return Ok(());
                    }
                    Err(e) => eprintln!("klark feed {}: {e}", self.id),
                }
                if t.saturating_sub(last_rotate) >= 60_000 {
                    last_rotate = t;
                    self.rotate_log();
                }
            }
            if self.subs.is_empty() && self.stopping.is_none() {
                // Every subscription stopped itself (the rate limit).
                self.kill_command();
                let why = "every subscription was stopped by the rate limit".to_string();
                self.edit_def(|d| d.stopped = Some(why.clone()));
                self.status.state = "stopped".into();
                self.status.detail = Some(why);
                self.status.worker_pid = None;
                self.write_status();
                return Ok(());
            }

            let reachable_before = self.subs.iter().any(|s| s.state.reachable());
            let check_every = if last_check.is_none() {
                1_000
            } else {
                check_every_ms(reachable_before, self.down_since.is_some())
            };
            if last_check.is_none_or(|c| t.saturating_sub(c) >= check_every) {
                last_check = Some(t);
                self.check_agents();
            }
            let reachable = self.subs.iter().any(|s| s.state.reachable());
            if reachable {
                // The agents are back: the grace of a resumed worker is over.
                self.grace_ms = 0;
            }
            let all_down =
                !self.subs.is_empty() && self.subs.iter().all(|s| s.state == AgentState::Down);
            self.down_since = if all_down {
                Some(self.down_since.unwrap_or(t))
            } else {
                None
            };
            if self
                .down_since
                .is_some_and(|d| t.saturating_sub(d) >= DOWN_EXIT_MS)
                && t >= self.grace_ms
            {
                // Every subscriber's server is down: nothing to run for. The
                // server start brings the feed back (`feed resume`).
                self.kill_command();
                self.status.state = "waiting".into();
                self.status.detail = Some("no klark server is running for its agents".into());
                self.status.worker_pid = None;
                self.write_status();
                eprintln!(
                    "klark feed {}: every agent's server is down, ending",
                    self.id
                );
                return Ok(());
            }
            let state = if reachable { "running" } else { "waiting" };
            if self.status.state != state && self.stopping.is_none() {
                self.status.state = state.into();
                self.status.detail =
                    (!reachable).then(|| "no subscribed agent is reachable".into());
                self.status_dirty = true;
            }

            // The source.
            let mut waited = false;
            if self.stopping.is_none() {
                match self.def.mode {
                    Mode::Stream => {
                        if reachable && self.stream.is_none() {
                            if let Err(e) = self.start_stream() {
                                eprintln!("klark feed {}: {e}", self.id);
                            }
                        } else if !reachable && self.stream.is_some() {
                            // No command runs with nobody to hear it.
                            eprintln!("klark feed {}: nobody reachable, command stopped", self.id);
                            self.kill_command();
                        }
                        waited = self.pump_stream();
                    }
                    Mode::Follow => {
                        if reachable {
                            self.pump_follow();
                        }
                    }
                    Mode::Poll => {
                        self.pump_poll();
                        if reachable && self.poll_rx.is_none() && t >= self.next_poll {
                            self.start_poll();
                        }
                    }
                }
            }
            let t = self.now();

            // Stale: one notice after the silent period, none again until
            // output resumed.
            if let Some(stale) = self.def.stale_secs {
                if reachable
                    && !self.stale_sent
                    && self.stopping.is_none()
                    && t.saturating_sub(self.last_output) >= stale * 1000
                {
                    self.stale_sent = true;
                    let notice = message::stale(&self.def.label, stale);
                    self.push_notice(&notice);
                }
            }

            // The idle stop counts running time only.
            let elapsed = t.saturating_sub(self.last_tick);
            self.last_tick = t;
            self.status.idle.advance(elapsed, IDLE_STEP_MS);
            if self.stopping.is_none() && self.status.idle.expired(self.def.idle_stop_secs * 1000) {
                let d = format_duration(self.def.idle_stop_secs);
                let notice = message::stopped_idle(&self.id, &d);
                self.push_notice(&notice);
                self.kill_command();
                self.stopping = Some((format!("nothing new for {d}"), t));
            }

            for i in 0..self.subs.len() {
                self.deliver(i, t);
            }
            self.finish_stopped_subs();

            if let Some((why, since)) = self.stopping.clone() {
                let left = self.subs.iter().any(|s| !s.queue.is_empty());
                if !left || t.saturating_sub(since) > FINAL_DELIVERY_MS {
                    self.kill_command();
                    self.edit_def(|d| d.stopped = Some(why.clone()));
                    self.status.state = "stopped".into();
                    self.status.detail = Some(why);
                    self.status.worker_pid = None;
                    self.status.idle.delivered();
                    self.write_status();
                    if left {
                        return Err("ended, but the last message could not be delivered".into());
                    }
                    eprintln!("klark feed {}: ended", self.id);
                    return Ok(());
                }
            }
            if self.status_dirty {
                self.write_status();
            }
            if !waited {
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(agents: Value) -> Result<Value, String> {
        Ok(json!({ "agents": agents }))
    }

    /// While every server looks down the worker must look again within the
    /// 3 s it waits before ending, or a live handoff (the socket gone for a
    /// moment) ends every feed: it would end without having looked again.
    #[test]
    fn looks_again_while_every_server_is_down() {
        assert_eq!(check_every_ms(true, false), 1_000);
        assert_eq!(check_every_ms(false, true), 1_000);
        assert!(check_every_ms(false, true) < DOWN_EXIT_MS);
        assert_eq!(check_every_ms(false, false), away_check_ms());
    }

    #[test]
    fn agent_found_by_name_or_pane() {
        let l = list(json!([
            {"name": "rev", "pane_id": "p1", "agent_status": "working"},
            {"pane_id": "p2", "agent_status": "unknown"},
        ]));
        assert_eq!(agent_state(&l, "rev"), AgentState::Busy("p1".into()));
        assert_eq!(agent_state(&l, "p2"), AgentState::Idle("p2".into()));
        assert!(matches!(agent_state(&l, "gone"), AgentState::Away(_)));
        assert_eq!(agent_state(&Err("down".into()), "rev"), AgentState::Down);
    }

    #[test]
    fn ambiguous_name_waits() {
        let l = list(json!([
            {"name": "rev", "pane_id": "p1"},
            {"name": "rev", "pane_id": "p2"},
        ]));
        match agent_state(&l, "rev") {
            AgentState::Away(why) => assert!(why.contains("2 agents"), "{why}"),
            other => panic!("{other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn capture_times_out_and_kills() {
        let stop = AtomicBool::new(false);
        let t = Instant::now();
        let cap = run_capture(
            "sleep 30; echo never",
            "",
            None,
            &[],
            Duration::from_millis(300),
            &stop,
            &|_| {},
        );
        assert!(cap.timed_out);
        assert!(t.elapsed() < Duration::from_secs(5));
        assert!(cap.stdout.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn capture_feeds_stdin_and_env() {
        let stop = AtomicBool::new(false);
        let cap = run_capture(
            "cat; echo \"$X\"",
            "",
            Some(b"in\n"),
            &[("X", "ex".into())],
            Duration::from_secs(5),
            &stop,
            &|_| {},
        );
        assert_eq!(cap.code, Some(0));
        assert_eq!(String::from_utf8_lossy(&cap.stdout), "in\nex\n");
    }

    #[cfg(windows)]
    #[test]
    fn capture_times_out_and_kills_on_windows() {
        let stop = AtomicBool::new(false);
        let t = Instant::now();
        let cap = run_capture(
            "ping -n 30 127.0.0.1 >nul & echo never",
            "",
            None,
            &[],
            Duration::from_millis(300),
            &stop,
            &|_| {},
        );
        assert!(cap.timed_out);
        assert!(t.elapsed() < Duration::from_secs(5));
        assert!(cap.stdout.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn capture_feeds_stdin_and_env_on_windows() {
        let stop = AtomicBool::new(false);
        let cap = run_capture(
            "more & echo %X%",
            "",
            Some(b"in\r\n"),
            &[("X", "ex".into())],
            Duration::from_secs(5),
            &stop,
            &|_| {},
        );
        assert_eq!(cap.code, Some(0));
        // `more` adds a blank line; only the text matters.
        let out = String::from_utf8_lossy(&cap.stdout);
        let lines: Vec<&str> = out
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        assert_eq!(lines, ["in", "ex"]);
    }

    #[cfg(unix)]
    #[test]
    fn rotated_log_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("klark-feed-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("f.log");
        let _ = std::fs::remove_file(&p);
        drop(open_log(&p).unwrap());
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }
}
