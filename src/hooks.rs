//! Global start/stop hooks (issue #11): machine services klark runs across
//! every session.
//!
//! One supervisor per machine (`klark hooks _supervise`) holds
//! `<config>/hooks/supervisor.lock` for its whole life. The first session
//! server to start launches it ([`ensure_running`]); it starts every
//! `[[hooks.start]]`, restarts them by their policy, and when no session
//! server has been running for a few seconds it stops them and runs the
//! `[[hooks.stop]]` commands, then exits. `klark hooks start` with no name
//! keeps it running without a server ("pinned") until `klark hooks stop`.
//!
//! The supervisor is the only writer of `status.json`. The CLI and the jobs
//! panel ask it for changes with small files in `requests/`, which it reads
//! every 200 ms and removes once handled. Each hook's output goes to
//! `<name>.log`. Environment values are never written anywhere.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::{HookDef, HooksConfig, RestartPolicy};

/// One rotation of a hook log at this size.
const LOG_CAP: u64 = 5 * 1024 * 1024;
/// Longest a stop hook may run.
const STOP_HOOK_TIMEOUT: Duration = Duration::from_secs(120);
/// Restart backoff: doubles from the first to the last; reset after a run this long.
const BACKOFF_FIRST: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
const BACKOFF_RESET: Duration = Duration::from_secs(60);
/// A hook back up this long after a notice that it stopped counts as running
/// again: one "running again" notice goes out, and its next exit is reported.
/// Until then, exits inside an automatic restart loop are not reported again.
const STABLE_AFTER: Duration = Duration::from_secs(10);

pub fn dir() -> PathBuf {
    crate::config::config_dir().join("hooks")
}

fn status_path() -> PathBuf {
    dir().join("status.json")
}

fn requests_dir() -> PathBuf {
    dir().join("requests")
}

pub fn log_path(name: &str) -> PathBuf {
    dir().join(format!("{name}.log"))
}

/// How long no session server must be seen before the hooks stop (a live
/// handoff or a quick restart closes the socket for a moment).
fn grace() -> Duration {
    let ms = std::env::var("KLARK_HOOKS_GRACE_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5_000);
    Duration::from_millis(ms)
}

fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------- status

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HookStatus {
    pub name: String,
    /// "start" or "stop".
    pub kind: String,
    /// running | backoff | stopped | exited | failed | done
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default)]
    pub restarts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_exit: Option<i32>,
    /// Unix seconds the current run started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    /// Unix seconds the last run ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exited_at: Option<u64>,
    #[serde(default)]
    pub restart: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SupervisorStatus {
    /// The supervisor's pid while it runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub updated: u64,
    #[serde(default)]
    pub hooks: Vec<HookStatus>,
}

pub fn read_status() -> SupervisorStatus {
    std::fs::read(status_path())
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_status(st: &SupervisorStatus) {
    let path = status_path();
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    if let Ok(bytes) = serde_json::to_vec_pretty(st) {
        if std::fs::write(&tmp, bytes).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

// ---------------------------------------------------------------- the lock

fn open_lock() -> std::io::Result<File> {
    std::fs::create_dir_all(dir())?;
    OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(dir().join("supervisor.lock"))
}

/// Whether a supervisor runs now.
pub fn is_running() -> bool {
    let Ok(f) = open_lock() else {
        return false;
    };
    match f.try_lock() {
        Ok(()) => false,
        Err(TryLockError::WouldBlock) => true,
        Err(TryLockError::Error(_)) => false,
    }
}

// ---------------------------------------------------------------- requests

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    Start,
    Stop,
    Restart,
    /// Stop every hook, run the stop hooks, end the supervisor.
    StopAll,
    /// Keep running without a session server.
    Pin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Request {
    action: Action,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

/// Leaves a request for the supervisor; returns its file, removed once handled.
pub fn send(action: Action, name: Option<&str>) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(requests_dir())?;
    static SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let file = format!("{nanos:024}-{}-{seq}.json", std::process::id());
    let path = requests_dir().join(&file);
    let tmp = requests_dir().join(format!(".{file}.tmp"));
    let req = Request {
        action,
        name: name.map(str::to_owned),
    };
    std::fs::write(
        &tmp,
        serde_json::to_vec(&req).map_err(std::io::Error::other)?,
    )?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// Waits until the supervisor has handled a request (its file is gone).
pub fn wait_handled(path: &Path, within: Duration) -> bool {
    let deadline = Instant::now() + within;
    while path.exists() {
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

fn take_requests() -> Vec<Request> {
    let Ok(entries) = std::fs::read_dir(requests_dir()) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    files
        .into_iter()
        .filter_map(|p| {
            let req = std::fs::read(&p)
                .ok()
                .and_then(|b| serde_json::from_slice::<Request>(&b).ok());
            let _ = std::fs::remove_file(&p);
            req
        })
        .collect()
}

// ---------------------------------------------------------------- starting it

/// At session server start: launch the supervisor in the background when
/// hooks are configured and none runs yet. Cheap when there are no hooks.
pub(crate) fn ensure_running() {
    let hooks = crate::config::Config::load().config.hooks;
    if hooks.is_empty() || is_running() {
        return;
    }
    if let Err(e) = spawn_supervisor(false) {
        tracing::warn!(error = %e, "could not start the hooks supervisor");
    }
}

fn spawn_supervisor(pinned: bool) -> std::io::Result<()> {
    std::fs::create_dir_all(dir())?;
    let log_path = dir().join("supervisor.log");
    if std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0) > LOG_CAP {
        let _ = std::fs::rename(&log_path, dir().join("supervisor.log.1"));
    }
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.args(["hooks", "_supervise"]);
    if pinned {
        cmd.arg("--pinned");
    }
    // Hooks belong to the machine, not to the session that happened to start first.
    for var in [
        crate::session::SESSION_ENV_VAR,
        "KLARK_PANE_ID",
        "KLARK_TAB_ID",
        "KLARK_WORKSPACE_ID",
        crate::api::SOCKET_PATH_ENV_VAR,
    ] {
        cmd.env_remove(var);
    }
    if let Some(home) = home() {
        cmd.current_dir(home);
    }
    cmd.stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    #[cfg(unix)]
    let mut child = {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        cmd.spawn()?
    };
    #[cfg(windows)]
    let mut child = crate::platform::spawn_detached_feed(&mut cmd)?;
    // Reaped off the caller's thread; the supervisor lives on as a detached process.
    std::thread::Builder::new()
        .name("klark-hooks-spawn".into())
        .spawn(move || child.wait())
        .map(|_| ())
}

fn home() -> Option<PathBuf> {
    let p = crate::worktree::expand_tilde_path("~");
    (p.as_os_str() != "~").then_some(p)
}

// ---------------------------------------------------------------- the supervisor

struct Running {
    def: HookDef,
    child: Option<Child>,
    restarts: u32,
    last_exit: Option<i32>,
    /// Unix seconds the last run ended.
    exited_at: Option<u64>,
    started: Option<(Instant, u64)>,
    /// Stopped by hand: not restarted until started by hand.
    held: bool,
    next_start: Option<Instant>,
    backoff: Duration,
    detail: Option<String>,
    /// A "stopped running" notice went out and no "running again" since.
    down_noticed: bool,
    /// Inside an automatic restart loop that was already reported: further
    /// quick exits are not reported again.
    quiet: bool,
    /// The wait before the restart scheduled last.
    wait: Duration,
    /// The loop reached the backoff cap and that was reported.
    cap_noticed: bool,
}

/// How a run ended, for the notice.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Ended {
    Code(i32),
    // Only built from a Unix exit status; Windows has no signals.
    #[cfg_attr(not(unix), allow(dead_code))]
    Signal(i32),
    Unknown,
    CouldNotStart(String),
}

impl Running {
    fn new(def: HookDef) -> Self {
        Running {
            def,
            child: None,
            restarts: 0,
            last_exit: None,
            exited_at: None,
            started: None,
            held: false,
            next_start: Some(Instant::now()),
            backoff: BACKOFF_FIRST,
            detail: None,
            down_noticed: false,
            quiet: false,
            wait: Duration::ZERO,
            cap_noticed: false,
        }
    }

    /// The next exit is reported again (a new loop starts from scratch).
    fn rearm(&mut self) {
        self.quiet = false;
        self.cap_noticed = false;
    }

    fn state(&self) -> &'static str {
        if self.child.is_some() {
            "running"
        } else if self.held {
            "stopped"
        } else if self.next_start.is_some() && self.restarts > 0 {
            "backoff"
        } else if self.next_start.is_some() {
            "starting"
        } else if self.last_exit.is_some_and(|c| c != 0) {
            "failed"
        } else {
            "exited"
        }
    }

    /// Starts the run; returns a notice when it could not start.
    fn start(&mut self) -> Option<String> {
        self.next_start = None;
        match spawn_hook(&self.def) {
            Ok(child) => {
                self.detail = None;
                self.started = Some((Instant::now(), unix_secs()));
                self.child = Some(child);
                None
            }
            Err(e) => {
                self.detail = Some(format!("could not start: {e}"));
                self.last_exit = None;
                self.schedule_restart(false);
                self.exit_notice(Ended::CouldNotStart(e.to_string()), unix_secs())
            }
        }
    }

    /// Started by hand: the next exit is reported again.
    fn by_hand(&mut self) {
        self.rearm();
    }

    /// Stops the run by hand; sends nothing (whoever stopped it asked for it).
    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            crate::feed_worker::kill_tree(&mut child);
            self.last_exit = child.try_wait().ok().flatten().and_then(|s| s.code());
            self.exited_at = Some(unix_secs());
        }
        self.started = None;
        self.next_start = None;
        self.rearm();
    }

    /// `klark hooks stop <name>`: stays stopped, and a pending "running
    /// again" notice is dropped (whoever stopped it knows).
    fn hold(&mut self) {
        self.held = true;
        self.stop();
        self.down_noticed = false;
    }

    /// The run ended: restart by policy, after the backoff.
    fn schedule_restart(&mut self, success: bool) {
        let again = match self.def.restart {
            RestartPolicy::Never => false,
            RestartPolicy::OnFailure => !success,
            RestartPolicy::Always => true,
        };
        if !again || self.held {
            return;
        }
        self.restarts += 1;
        self.wait = self.backoff;
        self.next_start = Some(Instant::now() + self.backoff);
        self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
    }

    /// Whether the run ended, and the notice to send for it, if any.
    fn poll(&mut self) -> (bool, Option<String>) {
        let Some(child) = self.child.as_mut() else {
            return (false, None);
        };
        let Ok(Some(status)) = child.try_wait() else {
            return (false, None);
        };
        self.child = None;
        self.last_exit = status.code();
        let ran = self.started.take().map(|(at, _)| at.elapsed());
        (true, self.ended(ended(&status), ran, unix_secs()))
    }

    /// Bookkeeping for a run that ended after running `ran`: the restart by
    /// policy and the notice.
    fn ended(&mut self, how: Ended, ran: Option<Duration>, now: u64) -> Option<String> {
        self.exited_at = Some(now);
        if ran.is_some_and(|d| d >= BACKOFF_RESET) {
            self.backoff = BACKOFF_FIRST;
        }
        if ran.is_some_and(|d| d >= STABLE_AFTER) {
            self.rearm();
        }
        self.schedule_restart(matches!(how, Ended::Code(0)));
        self.exit_notice(how, now)
    }

    /// The notice for a run that just ended, or `None` when it is a repeat
    /// inside an automatic restart loop already reported. Every other end is
    /// reported, a clean exit 0 included: restarted or not, someone may be
    /// relying on the hook. A loop that reaches the backoff cap is reported
    /// once more, so a hook that never recovers is not left at "restarting in 1s".
    fn exit_notice(&mut self, how: Ended, at: u64) -> Option<String> {
        let restarting = self.next_start.is_some();
        if self.quiet && restarting {
            if self.wait >= BACKOFF_MAX && !self.cap_noticed {
                self.cap_noticed = true;
                return Some(still_failing_text(&self.def.name, self.restarts));
            }
            return None;
        }
        self.down_noticed = true;
        self.quiet = restarting;
        Some(exit_text(
            &self.def.name,
            &how,
            at,
            self.def.restart,
            self.next_start
                .map(|t| t.saturating_duration_since(Instant::now())),
        ))
    }

    /// The "running again" notice, once a run reported down has stayed up
    /// [`STABLE_AFTER`].
    fn up_notice(&mut self) -> Option<String> {
        let (at, _) = self.started?;
        if !self.down_noticed || at.elapsed() < STABLE_AFTER {
            return None;
        }
        self.down_noticed = false;
        self.rearm();
        Some(up_text(
            &self.def.name,
            unix_secs(),
            self.child.as_ref().map(Child::id),
            self.restarts,
        ))
    }

    fn status(&self) -> HookStatus {
        HookStatus {
            name: self.def.name.clone(),
            kind: "start".into(),
            state: self.state().into(),
            pid: self.child.as_ref().map(Child::id),
            restarts: self.restarts,
            last_exit: self.last_exit,
            started_at: self.started.map(|(_, s)| s),
            exited_at: self.exited_at,
            restart: policy_name(self.def.restart).into(),
            detail: self.detail.clone(),
        }
    }
}

fn ended(status: &std::process::ExitStatus) -> Ended {
    if let Some(code) = status.code() {
        return Ended::Code(code);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = status.signal() {
            return Ended::Signal(sig);
        }
    }
    Ended::Unknown
}

/// `2026-11-17 10:04:05 UTC`.
fn clock(secs: u64) -> String {
    time::OffsetDateTime::from_unix_timestamp(secs as i64)
        .map(|t| {
            format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
                t.year(),
                u8::from(t.month()),
                t.day(),
                t.hour(),
                t.minute(),
                t.second()
            )
        })
        .unwrap_or_else(|_| secs.to_string())
}

fn exit_text(
    name: &str,
    how: &Ended,
    at: u64,
    policy: RestartPolicy,
    restart_in: Option<Duration>,
) -> String {
    let what = match how {
        Ended::Code(c) => format!("exited (code {c})"),
        Ended::Signal(s) => format!("exited (killed by signal {s})"),
        Ended::Unknown => "exited".to_string(),
        Ended::CouldNotStart(e) => format!("could not start ({e})"),
    };
    let next = match restart_in {
        Some(d) => format!(
            "restarting in {}s (further quick exits are not reported until it stays up {}s)",
            d.as_secs_f64().ceil() as u64,
            STABLE_AFTER.as_secs()
        ),
        None => format!("not restarting (klark hooks start {name} runs it again)"),
    };
    format!(
        "klark hook {name}: {what} at {}; restart policy {}; {next}. Logs: klark hooks logs {name}",
        clock(at),
        policy_name(policy)
    )
}

fn still_failing_text(name: &str, restarts: u32) -> String {
    format!(
        "klark hook {name}: still failing after {restarts} restarts; retrying every {}s. Logs: klark hooks logs {name}",
        BACKOFF_MAX.as_secs()
    )
}

fn up_text(name: &str, at: u64, pid: Option<u32>, restarts: u32) -> String {
    let pid = pid.map(|p| format!("pid {p}, ")).unwrap_or_default();
    format!(
        "klark hook {name}: running again at {} ({pid}{restarts} restarts). Logs: klark hooks logs {name}",
        clock(at)
    )
}

// ---------------------------------------------------------------- notices

/// Sends hook notices off the supervisor loop, in order: each is written to
/// the hook's log (so `klark feed add ... --hook <name>` delivers it like any
/// other line) and sent the way `klark msg send` sends to every `notify`
/// address. Delivery may wait on an agent someone is typing into, or on the
/// hub, and the supervisor must keep answering requests meanwhile.
///
/// Notices still queued here when the supervisor exits are lost (the log line
/// is already written). That only happens at shutdown, when klark stops the
/// hooks itself and sends no notice for that.
struct Notifier {
    tx: std::sync::mpsc::Sender<(String, Vec<String>, String)>,
}

impl Notifier {
    fn spawn() -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<(String, Vec<String>, String)>();
        let _ = std::thread::Builder::new()
            .name("klark-hooks-notify".into())
            .spawn(move || {
                for (name, to, text) in rx {
                    for addr in to {
                        match crate::msg_cmd::send_notice(&addr, &name, &text) {
                            Ok(shown) => eprintln!("klark hooks: {name}: told {shown}"),
                            Err(e) => {
                                eprintln!("klark hooks: {name}: could not tell {addr}: {e}")
                            }
                        }
                    }
                }
            });
        Notifier { tx }
    }

    fn send(&self, def: &HookDef, text: String) {
        eprintln!("{text}");
        if let Ok(mut log) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path(&def.name))
        {
            let _ = writeln!(log, "{text}");
        }
        if !def.notify.is_empty() {
            let _ = self.tx.send((def.name.clone(), def.notify.clone(), text));
        }
    }
}

fn policy_name(p: RestartPolicy) -> &'static str {
    match p {
        RestartPolicy::Never => "never",
        RestartPolicy::OnFailure => "on-failure",
        RestartPolicy::Always => "always",
    }
}

/// The hook's command through the user's shell, in its own process group,
/// output appended to its log.
fn spawn_hook(def: &HookDef) -> std::io::Result<Child> {
    let log_path = log_path(&def.name);
    if std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0) > LOG_CAP {
        let _ = std::fs::rename(&log_path, dir().join(format!("{}.log.1", def.name)));
    }
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let _ = writeln!(
        log,
        "--- klark hooks: start {} at {}",
        def.name,
        unix_secs()
    );
    let mut c = crate::feed_worker::shell_command(&def.run);
    let cwd = match def.cwd.as_deref() {
        Some(cwd) if !cwd.trim().is_empty() => crate::worktree::expand_tilde_path(cwd),
        _ => home().unwrap_or_else(|| PathBuf::from(".")),
    };
    c.current_dir(cwd);
    for (k, v) in &def.env {
        c.env(k, v);
    }
    c.env("KLARK_HOOK", &def.name);
    // A hook belongs to no session but may use klark (`klark session list --json`
    // and each session's socket), like a program in a pane.
    c.env("KLARK_ENV", "1");
    if let Ok(exe) = std::env::current_exe() {
        c.env("KLARK_BIN_PATH", exe);
    }
    c.stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
        c.spawn()
    }
    #[cfg(windows)]
    {
        crate::platform::prepare_feed_command(&mut c);
        let mut child = c.spawn()?;
        crate::platform::attach_feed_job(&mut child)?;
        Ok(child)
    }
}

/// Runs one stop hook to its end (or the timeout); returns its status.
fn run_stop_hook(def: &HookDef) -> HookStatus {
    let mut st = HookStatus {
        name: def.name.clone(),
        kind: "stop".into(),
        state: "done".into(),
        restart: String::new(),
        ..HookStatus::default()
    };
    match spawn_hook(def) {
        Ok(mut child) => {
            let deadline = Instant::now() + STOP_HOOK_TIMEOUT;
            loop {
                if let Ok(Some(s)) = child.try_wait() {
                    st.last_exit = s.code();
                    if !s.success() {
                        st.state = "failed".into();
                    }
                    break;
                }
                if Instant::now() >= deadline {
                    crate::feed_worker::kill_tree(&mut child);
                    st.state = "failed".into();
                    st.detail = Some("timed out".into());
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        Err(e) => {
            st.state = "failed".into();
            st.detail = Some(format!("could not start: {e}"));
        }
    }
    st
}

/// Kills hook process groups a supervisor that died hard left behind.
fn reap_leftovers() {
    #[cfg(unix)]
    for h in read_status().hooks {
        if let Some(pid) = h.pid {
            let pgid = pid as libc::pid_t;
            if crate::feed_worker::group_alive(pgid) {
                // SAFETY: kill(2) on the process group the dead supervisor created for this hook.
                unsafe { libc::kill(-pgid, libc::SIGTERM) };
                std::thread::sleep(Duration::from_millis(500));
                if crate::feed_worker::group_alive(pgid) {
                    // SAFETY: as above.
                    unsafe { libc::kill(-pgid, libc::SIGKILL) };
                }
            }
        }
    }
}

fn any_server_running() -> bool {
    crate::session::list_sessions()
        .map(|s| s.iter().any(|s| s.running))
        .unwrap_or(true)
}

/// A restart runs the hook as config.toml says now (a running hook keeps the
/// definition it was started with until then).
fn refresh_def(r: &mut Running, config: &HooksConfig) {
    if let Some(d) = config.start.iter().find(|d| d.name == r.def.name) {
        r.def = d.clone();
    }
}

fn load_hooks() -> HooksConfig {
    crate::config::Config::load().config.hooks.valid()
}

/// `klark hooks _supervise [--pinned]`: the supervisor's whole life.
fn supervise(mut pinned: bool) -> i32 {
    let Ok(lock) = open_lock() else {
        eprintln!("klark hooks: cannot open the lock in {}", dir().display());
        return 1;
    };
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            // Another supervisor runs: hand it the pin and leave.
            if pinned {
                let _ = send(Action::Pin, None);
            }
            return 0;
        }
        Err(TryLockError::Error(e)) => {
            eprintln!("klark hooks: lock: {e}");
            return 1;
        }
    }
    let _ = std::fs::create_dir_all(requests_dir());
    reap_leftovers();
    eprintln!(
        "klark hooks: supervisor {} started (pinned: {pinned})",
        std::process::id()
    );

    let notifier = Notifier::spawn();
    let mut config = load_hooks();
    let mut hooks: BTreeMap<String, Running> = config
        .start
        .iter()
        .map(|d| (d.name.clone(), Running::new(d.clone())))
        .collect();
    let mut stop_results: Vec<HookStatus> = Vec::new();
    let mut last_seen_server = Instant::now();
    let mut last_server_check = Instant::now() - Duration::from_secs(10);
    let mut last_write = Instant::now() - Duration::from_secs(10);

    let write = |hooks: &BTreeMap<String, Running>,
                 config: &HooksConfig,
                 stop_results: &[HookStatus],
                 pinned: bool,
                 alive: bool| {
        let mut list: Vec<HookStatus> = hooks.values().map(Running::status).collect();
        for d in &config.stop {
            list.push(
                stop_results
                    .iter()
                    .find(|s| s.name == d.name)
                    .cloned()
                    .unwrap_or_else(|| HookStatus {
                        name: d.name.clone(),
                        kind: "stop".into(),
                        state: "waiting".into(),
                        ..HookStatus::default()
                    }),
            );
        }
        write_status(&SupervisorStatus {
            pid: alive.then(std::process::id),
            pinned,
            updated: unix_secs(),
            hooks: list,
        });
    };

    loop {
        let mut changed = false;
        let mut stop_all = false;
        let requests = take_requests();
        if !requests.is_empty() {
            // A request may name a hook just added to config.toml.
            config = load_hooks();
            for d in &config.start {
                if let Some(r) = hooks.get_mut(&d.name) {
                    if r.def != *d && r.child.is_none() {
                        r.def = d.clone();
                    }
                } else {
                    let mut r = Running::new(d.clone());
                    r.held = true;
                    r.next_start = None;
                    hooks.insert(d.name.clone(), r);
                }
            }
        }
        for req in requests {
            changed = true;
            match (req.action, req.name) {
                (Action::Pin, _) => pinned = true,
                (Action::StopAll, _) => stop_all = true,
                (Action::Start, None) => {
                    pinned = true;
                    for r in hooks.values_mut() {
                        if r.child.is_none() {
                            r.held = false;
                            r.by_hand();
                            r.next_start = Some(Instant::now());
                        }
                    }
                }
                (Action::Stop, None) => stop_all = true,
                (Action::Restart, None) => {
                    for r in hooks.values_mut() {
                        r.stop();
                        refresh_def(r, &config);
                        r.held = false;
                        r.backoff = BACKOFF_FIRST;
                        r.next_start = Some(Instant::now());
                    }
                }
                (action, Some(name)) => {
                    let Some(r) = hooks.get_mut(&name) else {
                        continue;
                    };
                    match action {
                        Action::Start => {
                            r.held = false;
                            r.by_hand();
                            r.backoff = BACKOFF_FIRST;
                            if r.child.is_none() {
                                r.next_start = Some(Instant::now());
                            }
                        }
                        Action::Stop => r.hold(),
                        Action::Restart => {
                            r.stop();
                            refresh_def(r, &config);
                            r.held = false;
                            r.backoff = BACKOFF_FIRST;
                            r.next_start = Some(Instant::now());
                        }
                        Action::StopAll | Action::Pin => {}
                    }
                }
            }
        }

        for r in hooks.values_mut() {
            let (ended, notice) = r.poll();
            if ended {
                changed = true;
            }
            if let Some(text) = notice {
                notifier.send(&r.def, text);
            }
            if r.child.is_none() && !r.held && r.next_start.is_some_and(|at| Instant::now() >= at) {
                if let Some(text) = r.start() {
                    notifier.send(&r.def, text);
                }
                changed = true;
            }
            if let Some(text) = r.up_notice() {
                notifier.send(&r.def, text);
            }
        }

        if !pinned && last_server_check.elapsed() >= Duration::from_secs(1) {
            last_server_check = Instant::now();
            if any_server_running() {
                last_seen_server = Instant::now();
            } else if last_seen_server.elapsed() >= grace() {
                eprintln!("klark hooks: no klark server runs; stopping");
                stop_all = true;
            }
        }

        if stop_all {
            for r in hooks.values_mut() {
                r.held = true;
                r.stop();
            }
            write(&hooks, &config, &stop_results, pinned, true);
            for d in config.stop.clone() {
                eprintln!("klark hooks: stop hook {}", d.name);
                stop_results.push(run_stop_hook(&d));
                write(&hooks, &config, &stop_results, pinned, true);
            }
            write(&hooks, &config, &stop_results, false, false);
            eprintln!("klark hooks: supervisor {} ended", std::process::id());
            // The lock goes with the process.
            drop(lock);
            return 0;
        }

        if changed || last_write.elapsed() >= Duration::from_secs(2) {
            write(&hooks, &config, &stop_results, pinned, true);
            last_write = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

// ---------------------------------------------------------------- the CLI

const USAGE: &str = "\
klark hooks: machine services klark runs across every session ([[hooks.start]] / [[hooks.stop]] in config.toml)

  klark hooks list [--json]         every hook: state, pid, restarts, last exit and when, restart policy
  klark hooks start [<name>]        start one hook; with no name, start them all and keep them
                                    running even with no klark server (until `klark hooks stop`)
  klark hooks stop [<name>]         stop one hook (it stays stopped); with no name, stop them all
                                    and run the stop hooks
  klark hooks restart [<name>]      restart one hook, or all
  klark hooks logs <name> [-n N] [-f]   a hook's output (last N lines, default 50; -f follows)

Start hooks run once per machine when the first klark server starts; stop hooks run once
after the last one stops. Logs: <config>/hooks/<name>.log. When a start hook stops running
(or runs again), a notice goes to its log and to the agents in its `notify` list.";

pub fn run(args: &[String]) -> ! {
    let code = match args.first().map(String::as_str) {
        None | Some("-h" | "--help" | "help") => {
            println!("{USAGE}");
            0
        }
        Some("_supervise") => supervise(args.iter().any(|a| a == "--pinned")),
        Some("list" | "ls") => cmd_list(&args[1..]),
        Some("start") => cmd_control(Action::Start, &args[1..]),
        Some("stop") => cmd_control(Action::Stop, &args[1..]),
        Some("restart") => cmd_control(Action::Restart, &args[1..]),
        Some("logs" | "log") => cmd_logs(&args[1..]),
        Some(other) => {
            eprintln!("klark hooks: unknown command {other:?}\n\n{USAGE}");
            2
        }
    };
    std::process::exit(code)
}

/// Every configured hook with its live status ("not running" when the
/// supervisor is not running): the CLI list and the jobs panel.
pub(crate) fn snapshot() -> (Option<SupervisorStatus>, Vec<HookStatus>) {
    let hooks = crate::config::Config::load().config.hooks;
    let status = is_running().then(read_status);
    let list = hooks
        .all()
        .map(|(d, start)| {
            status
                .as_ref()
                .and_then(|st| st.hooks.iter().find(|s| s.name == d.name).cloned())
                .unwrap_or_else(|| HookStatus {
                    name: d.name.clone(),
                    kind: if start { "start" } else { "stop" }.into(),
                    state: "not running".into(),
                    restart: if start { policy_name(d.restart) } else { "" }.into(),
                    ..HookStatus::default()
                })
        })
        .collect();
    (status, list)
}

/// Starts the supervisor kept running without a server (`klark hooks start`).
pub(crate) fn start_pinned() -> std::io::Result<()> {
    if is_running() {
        return send(Action::Start, None).map(|_| ());
    }
    spawn_supervisor(true)
}

fn cmd_list(args: &[String]) -> i32 {
    let json = args.iter().any(|a| a == "--json");
    let loaded = crate::config::Config::load();
    let hooks = loaded.config.hooks;
    let running = is_running();
    let status = if running {
        read_status()
    } else {
        SupervisorStatus::default()
    };
    if json {
        let (_, list) = snapshot();
        println!(
            "{}",
            serde_json::json!({"supervisor": running.then_some(status.pid).flatten(), "pinned": status.pinned, "hooks": list})
        );
        return 0;
    }
    if hooks.is_empty() {
        println!(
            "No hooks. Add [[hooks.start]] or [[hooks.stop]] to {}.",
            crate::config::config_path().display()
        );
        return 0;
    }
    match (running, status.pid) {
        (true, Some(pid)) => println!(
            "supervisor {pid}{}",
            if status.pinned {
                " (kept running by `klark hooks start`)"
            } else {
                ""
            }
        ),
        _ => println!(
            "supervisor not running (starts with the first klark server, or `klark hooks start`)"
        ),
    }
    println!(
        "{:<20} {:<6} {:<10} {:>7} {:>8} {:>5}  {:<10} {:<10} UP",
        "NAME", "KIND", "STATE", "PID", "RESTARTS", "EXIT", "RESTART", "EXITED"
    );
    for (d, start) in hooks.all() {
        let s = status.hooks.iter().find(|s| s.name == d.name);
        let state = s.map_or("-", |s| s.state.as_str());
        let pid = s.and_then(|s| s.pid).map_or("-".into(), |p| p.to_string());
        let restarts = s.map_or(0, |s| s.restarts);
        let exit = s
            .and_then(|s| s.last_exit)
            .map_or("-".into(), |c| c.to_string());
        let up = s
            .and_then(|s| s.started_at)
            .filter(|_| s.is_some_and(|s| s.state == "running"))
            .map_or("-".into(), |t| {
                short_duration(unix_secs().saturating_sub(t))
            });
        let policy = if start { policy_name(d.restart) } else { "-" };
        let exited = s.and_then(|s| s.exited_at).map_or("-".into(), |t| {
            format!("{} ago", short_duration(unix_secs().saturating_sub(t)))
        });
        println!(
            "{:<20} {:<6} {:<10} {:>7} {:>8} {:>5}  {:<10} {:<10} {}",
            d.name,
            if start { "start" } else { "stop" },
            state,
            pid,
            restarts,
            exit,
            policy,
            exited,
            up
        );
        if let Some(detail) = s.and_then(|s| s.detail.as_deref()) {
            println!("  {detail}");
        }
    }
    0
}

fn short_duration(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s => format!("{}d", s / 86_400),
    }
}

fn cmd_control(action: Action, args: &[String]) -> i32 {
    let name = args.first().map(String::as_str);
    if args.len() > 1 {
        eprintln!("usage: klark hooks {} [<name>]", action_word(&action));
        return 2;
    }
    let hooks = crate::config::Config::load().config.hooks;
    if let Some(name) = name {
        match hooks.find(name) {
            Some((_, true)) => {}
            Some((_, false)) => {
                eprintln!("klark hooks: {name} is a stop hook; it runs when the hooks stop");
                return 2;
            }
            None => {
                eprintln!("klark hooks: no hook named {name:?} in config.toml");
                return 2;
            }
        }
    }
    if !is_running() {
        match (&action, name) {
            (Action::Start, None) => {
                if hooks.is_empty() {
                    eprintln!("klark hooks: no hooks in config.toml");
                    return 1;
                }
                if let Err(e) = spawn_supervisor(true) {
                    eprintln!("klark hooks: could not start: {e}");
                    return 1;
                }
                let deadline = Instant::now() + Duration::from_secs(5);
                while !is_running() && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(50));
                }
                println!("hooks started");
                return 0;
            }
            (Action::Stop, _) => {
                println!("hooks are not running");
                return 0;
            }
            _ => {
                eprintln!("klark hooks: not running; `klark hooks start` starts them");
                return 1;
            }
        }
    }
    let path = match send(action.clone(), name) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("klark hooks: {e}");
            return 1;
        }
    };
    let within = if matches!(action, Action::Stop) && name.is_none() {
        STOP_HOOK_TIMEOUT + Duration::from_secs(10)
    } else {
        Duration::from_secs(10)
    };
    if !wait_handled(&path, within) {
        eprintln!("klark hooks: the supervisor did not answer");
        return 1;
    }
    if matches!(action, Action::Stop) && name.is_none() {
        let deadline = Instant::now() + within;
        while is_running() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        println!("hooks stopped");
    } else {
        std::thread::sleep(Duration::from_millis(300));
        println!("{} {}", action_word(&action), name.unwrap_or("all hooks"));
    }
    0
}

fn action_word(a: &Action) -> &'static str {
    match a {
        Action::Start => "started",
        Action::Stop => "stopped",
        Action::Restart => "restarted",
        Action::StopAll => "stopped",
        Action::Pin => "pinned",
    }
}

fn cmd_logs(args: &[String]) -> i32 {
    let mut name = None;
    let mut lines = 50usize;
    let mut follow = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-f" | "--follow" => follow = true,
            "-n" | "--lines" => {
                i += 1;
                match args.get(i).and_then(|v| v.parse().ok()) {
                    Some(n) => lines = n,
                    None => {
                        eprintln!("klark hooks logs: -n needs a number");
                        return 2;
                    }
                }
            }
            n if name.is_none() && !n.starts_with('-') => name = Some(n.to_string()),
            other => {
                eprintln!("klark hooks logs: unexpected {other:?}");
                return 2;
            }
        }
        i += 1;
    }
    let Some(name) = name else {
        eprintln!("usage: klark hooks logs <name> [-n N] [-f]");
        return 2;
    };
    if crate::config::validate_hook_name(&name).is_err() {
        eprintln!("klark hooks logs: bad name {name:?}");
        return 2;
    }
    let path = log_path(&name);
    let Ok(mut f) = File::open(&path) else {
        eprintln!(
            "klark hooks logs: no log yet for {name} ({})",
            path.display()
        );
        return 1;
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(256 * 1024);
    let _ = f.seek(SeekFrom::Start(start));
    let mut buf = Vec::new();
    let _ = f.read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let all: Vec<&str> = text.lines().collect();
    for l in &all[all.len().saturating_sub(lines)..] {
        println!("{l}");
    }
    if !follow {
        return 0;
    }
    let mut pos = len;
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let Ok(mut f) = File::open(&path) else {
            continue;
        };
        let now = f.metadata().map(|m| m.len()).unwrap_or(0);
        if now < pos {
            pos = 0; // rotated or truncated
        }
        if now > pos {
            let _ = f.seek(SeekFrom::Start(pos));
            let mut chunk = Vec::new();
            let _ = f.read_to_end(&mut chunk);
            print!("{}", String::from_utf8_lossy(&chunk));
            let _ = std::io::stdout().flush();
            pos = now;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(restart: RestartPolicy) -> HookDef {
        HookDef {
            name: "x".into(),
            run: "true".into(),
            cwd: None,
            restart,
            env: Default::default(),
            notify: Vec::new(),
        }
    }

    #[test]
    fn restart_policy_and_backoff() {
        let mut r = Running::new(def(RestartPolicy::OnFailure));
        r.next_start = None;
        r.schedule_restart(true);
        assert!(
            r.next_start.is_none(),
            "on-failure does not restart a clean exit"
        );
        r.schedule_restart(false);
        assert!(r.next_start.is_some());
        assert_eq!(r.backoff, BACKOFF_FIRST * 2);

        let mut r = Running::new(def(RestartPolicy::Always));
        r.next_start = None;
        for _ in 0..10 {
            r.schedule_restart(true);
        }
        assert_eq!(r.restarts, 10);
        assert_eq!(r.backoff, BACKOFF_MAX);

        let mut r = Running::new(def(RestartPolicy::Never));
        r.next_start = None;
        r.schedule_restart(false);
        assert!(r.next_start.is_none());

        let mut r = Running::new(def(RestartPolicy::Always));
        r.held = true;
        r.next_start = None;
        r.schedule_restart(false);
        assert!(
            r.next_start.is_none(),
            "a hook stopped by hand stays stopped"
        );
    }

    /// The 2026-11-17 incident: dmcp under on-failure exited 0, was not
    /// restarted, and nobody was told.
    #[test]
    fn on_failure_clean_exit_still_sends_a_notice() {
        let mut r = Running::new(def(RestartPolicy::OnFailure));
        r.next_start = None;
        let at = 1_763_373_845; // 2025-11-17 10:04:05 UTC
        let n = r
            .ended(Ended::Code(0), Some(Duration::from_secs(3600)), at)
            .expect("a clean exit is reported");
        assert!(
            r.next_start.is_none(),
            "on-failure still does not restart it"
        );
        assert_eq!(r.exited_at, Some(at));
        assert_eq!(r.status().exited_at, Some(at));
        assert_eq!(
            n,
            "klark hook x: exited (code 0) at 2025-11-17 10:04:05 UTC; restart policy on-failure; \
             not restarting (klark hooks start x runs it again). Logs: klark hooks logs x"
        );
    }

    #[test]
    fn restart_loops_are_reported_once() {
        let quick = Some(Duration::from_millis(100));
        let mut r = Running::new(def(RestartPolicy::Always));
        r.next_start = None;
        let first = r.ended(Ended::Code(1), quick, 1).expect("first exit");
        assert!(first.contains("exited (code 1)"), "{first}");
        assert!(
            first.contains("restart policy always; restarting in 1s"),
            "{first}"
        );
        for _ in 0..5 {
            r.next_start = None;
            assert_eq!(r.ended(Ended::Signal(9), quick, 2), None, "collapsed");
        }
        assert_eq!(r.exited_at, Some(2), "exited_at still moves");
        // A run that stayed up is a new event.
        r.next_start = None;
        assert!(r.ended(Ended::Code(1), Some(STABLE_AFTER), 3).is_some());

        // The loop ending (on-failure, then exit 0) is a state change too.
        let mut r = Running::new(def(RestartPolicy::OnFailure));
        r.next_start = None;
        assert!(r.ended(Ended::Code(2), quick, 1).is_some());
        r.next_start = None;
        let last = r.ended(Ended::Code(0), quick, 2).expect("loop ended");
        assert!(last.contains("not restarting"), "{last}");

        // Started by hand: its next exit is reported again.
        let mut r = Running::new(def(RestartPolicy::Always));
        r.next_start = None;
        assert!(r.ended(Ended::Code(1), quick, 1).is_some());
        r.next_start = None;
        r.by_hand();
        assert!(r.ended(Ended::Code(1), quick, 2).is_some());
    }

    /// A loop that never recovers: one notice at the start, one when the
    /// backoff reaches its 60 s cap, then nothing more.
    #[test]
    fn a_loop_at_the_backoff_cap_is_reported_once_more() {
        let quick = Some(Duration::from_millis(100));
        for policy in [RestartPolicy::Always, RestartPolicy::OnFailure] {
            let mut r = Running::new(def(policy));
            r.next_start = None;
            let mut sent = Vec::new();
            for i in 0..12 {
                r.next_start = None;
                if let Some(n) = r.ended(Ended::Code(1), quick, i) {
                    sent.push((r.wait, n));
                }
            }
            assert_eq!(sent.len(), 2, "{sent:#?}");
            assert!(sent[0].1.contains("restarting in 1s"), "{sent:#?}");
            assert_eq!(sent[1].0, BACKOFF_MAX);
            // Waits 1, 2, 4, 8, 16, 32, then 60: the 7th restart is the first at the cap.
            assert_eq!(
                sent[1].1,
                "klark hook x: still failing after 7 restarts; retrying every 60s. \
                 Logs: klark hooks logs x"
            );

            // Started by hand, it can reach the cap and be reported again.
            r.by_hand();
            r.backoff = BACKOFF_FIRST;
            let mut again = 0;
            for i in 0..12 {
                r.next_start = None;
                if r.ended(Ended::Code(1), quick, 100 + i).is_some() {
                    again += 1;
                }
            }
            assert_eq!(again, 2);
        }
    }

    #[test]
    fn running_again_is_reported_once_it_stays_up() {
        let mut r = Running::new(def(RestartPolicy::Always));
        r.next_start = None;
        assert_eq!(r.up_notice(), None, "not running");
        r.started = Some((Instant::now(), 0));
        assert_eq!(r.up_notice(), None, "never reported down");
        assert!(r.ended(Ended::Code(1), None, 1).is_some());
        r.started = Some((Instant::now(), 0));
        assert_eq!(r.up_notice(), None, "not up long enough");
        let long_ago = Instant::now()
            .checked_sub(STABLE_AFTER)
            .expect("an instant 10 s ago");
        r.started = Some((long_ago, 0));
        let up = r.up_notice().expect("running again");
        assert!(up.starts_with("klark hook x: running again at "), "{up}");
        assert!(up.ends_with("Logs: klark hooks logs x"), "{up}");
        assert_eq!(r.up_notice(), None, "once");

        // Brought back by `hooks restart` (stop, then start): announced.
        assert!(r.ended(Ended::Code(1), None, 2).is_some());
        r.stop();
        r.started = Some((long_ago, 0));
        assert!(r.up_notice().is_some(), "running again after a restart");

        // Stopped by hand (`hooks stop`): nothing to announce.
        assert!(r.ended(Ended::Code(1), None, 3).is_some());
        r.hold();
        r.started = Some((long_ago, 0));
        assert_eq!(r.up_notice(), None);
    }

    #[test]
    fn notice_texts() {
        assert_eq!(clock(0), "1970-01-01 00:00:00 UTC");
        let t = exit_text(
            "svc",
            &Ended::CouldNotStart("no such dir".into()),
            0,
            RestartPolicy::Never,
            None,
        );
        assert!(
            t.starts_with("klark hook svc: could not start (no such dir) at 1970-01-01"),
            "{t}"
        );
        assert!(t.contains("restart policy never; not restarting"), "{t}");
        let t = exit_text(
            "svc",
            &Ended::Signal(15),
            0,
            RestartPolicy::Always,
            Some(Duration::from_millis(1500)),
        );
        assert!(t.contains("killed by signal 15"), "{t}");
        assert!(t.contains("restarting in 2s"), "{t}");
    }

    #[test]
    fn short_durations() {
        assert_eq!(short_duration(5), "5s");
        assert_eq!(short_duration(125), "2m");
        assert_eq!(short_duration(7200), "2h");
        assert_eq!(short_duration(200_000), "2d");
    }
}
