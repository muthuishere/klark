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
    started: Option<(Instant, u64)>,
    /// Stopped by hand: not restarted until started by hand.
    held: bool,
    next_start: Option<Instant>,
    backoff: Duration,
    detail: Option<String>,
}

impl Running {
    fn new(def: HookDef) -> Self {
        Running {
            def,
            child: None,
            restarts: 0,
            last_exit: None,
            started: None,
            held: false,
            next_start: Some(Instant::now()),
            backoff: BACKOFF_FIRST,
            detail: None,
        }
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

    fn start(&mut self) {
        self.next_start = None;
        match spawn_hook(&self.def) {
            Ok(child) => {
                self.detail = None;
                self.started = Some((Instant::now(), unix_secs()));
                self.child = Some(child);
            }
            Err(e) => {
                self.detail = Some(format!("could not start: {e}"));
                self.last_exit = None;
                self.schedule_restart(false);
            }
        }
    }

    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            crate::feed_worker::kill_tree(&mut child);
            self.last_exit = child.try_wait().ok().flatten().and_then(|s| s.code());
        }
        self.started = None;
        self.next_start = None;
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
        self.next_start = Some(Instant::now() + self.backoff);
        self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
    }

    fn poll(&mut self) -> bool {
        let Some(child) = self.child.as_mut() else {
            return false;
        };
        let Ok(Some(status)) = child.try_wait() else {
            return false;
        };
        self.child = None;
        self.last_exit = status.code();
        if self
            .started
            .take()
            .is_some_and(|(at, _)| at.elapsed() >= BACKOFF_RESET)
        {
            self.backoff = BACKOFF_FIRST;
        }
        self.schedule_restart(status.success());
        true
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
            restart: policy_name(self.def.restart).into(),
            detail: self.detail.clone(),
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
                            r.next_start = Some(Instant::now());
                        }
                    }
                }
                (Action::Stop, None) => stop_all = true,
                (Action::Restart, None) => {
                    for r in hooks.values_mut() {
                        r.stop();
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
                            r.backoff = BACKOFF_FIRST;
                            if r.child.is_none() {
                                r.next_start = Some(Instant::now());
                            }
                        }
                        Action::Stop => {
                            r.held = true;
                            r.stop();
                        }
                        Action::Restart => {
                            r.stop();
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
            if r.poll() {
                changed = true;
            }
            if r.child.is_none() && !r.held && r.next_start.is_some_and(|at| Instant::now() >= at) {
                r.start();
                changed = true;
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

  klark hooks list [--json]         every hook: state, pid, restarts, last exit
  klark hooks start [<name>]        start one hook; with no name, start them all and keep them
                                    running even with no klark server (until `klark hooks stop`)
  klark hooks stop [<name>]         stop one hook (it stays stopped); with no name, stop them all
                                    and run the stop hooks
  klark hooks restart [<name>]      restart one hook, or all
  klark hooks logs <name> [-n N] [-f]   a hook's output (last N lines, default 50; -f follows)

Start hooks run once per machine when the first klark server starts; stop hooks run once
after the last one stops. Logs: <config>/hooks/<name>.log.";

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
        "{:<20} {:<6} {:<10} {:>7} {:>8} {:>5}  UP",
        "NAME", "KIND", "STATE", "PID", "RESTARTS", "EXIT"
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
        println!(
            "{:<20} {:<6} {:<10} {:>7} {:>8} {:>5}  {}",
            d.name,
            if start { "start" } else { "stop" },
            state,
            pid,
            restarts,
            exit,
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

    #[test]
    fn short_durations() {
        assert_eq!(short_duration(5), "5s");
        assert_eq!(short_duration(125), "2m");
        assert_eq!(short_duration(7200), "2h");
        assert_eq!(short_duration(200_000), "2d");
    }
}
