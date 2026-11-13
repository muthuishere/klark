//! `klark feed`: a background command's new output goes to agents (ADR 0015).
//!
//! The CLI side: argument parsing, the saved definition (`<id>.json`, changed
//! only under the store lock), starting the detached worker
//! (`src/feed_worker.rs`) and the verbs that list, show, stop, start and
//! change feeds.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::agent_index::{call, local_agents, parse_address, resolve, session_socket, Route};
use crate::config::FeedsConfig;
use crate::feed::model::{format_duration, join_key, parse_duration, FeedDef, Mode, Status, Sub};
use crate::feed::store::Store;
use crate::feed::{
    DEFAULT_GAP_SECS, DEFAULT_IDLE_STOP_SECS, DEFAULT_MAX_BYTES, DEFAULT_MAX_PER_HOUR, MIN_GAP_SECS,
};
use crate::feed_worker::{
    self as worker, def_name, log_name, pending_name, status_name, DOWN_EXIT_MS,
};

const USAGE: &str = "klark feed: send a background command's new output to agents (ADR 0015)

usage:
  klark feed add <agent> \"<instruction>\" --stream \"<command>\" [--message-gap <secs>] [options]
  klark feed add <agent> \"<instruction>\" --follow <path>      [--message-gap <secs>] [options]
  klark feed add <agent> \"<instruction>\" --poll \"<command>\"   [--interval <secs>] [options]
  klark feed list [--json]                    every feed: agents, state, last output, last sent, creator
  klark feed show <id>                        everything about one feed, including its settings
  klark feed set [<setting> [<value>]]        show or change the defaults for new feeds ([feeds] in config.toml)
  klark feed agent add <id> <agent> [\"<instruction>\"]   subscribe another agent
  klark feed agent remove <id> <agent>        unsubscribe an agent
  klark feed leave <id>                       unsubscribe myself (the calling pane)
  klark feed stop <id|--all>                  end a feed for everyone, for good
  klark feed start <id>                       run a feed again after it stopped itself
  klark feed batch <id> <n>                   print a batch that was cut in a message
  klark feed logs <id>                        the worker's own log
  klark feed resume [--session <name>]        run the session's feeds again (the server does this at start)

sources (exactly one):
  --stream \"<command>\"   a command that keeps running; its new lines are sent
  --follow <path>        a file klark follows itself (rotation, truncation, restarts)
  --poll \"<command>\"     a command run again and again; only what is new is sent

options (defaults from klark feed set):
  --message-gap <secs>   least time between two messages to one agent (stream, follow; default 30, min 5)
  --interval <secs>      time between two runs (poll; default 30, min 5)
  --diff lines|unified|\"<command>\"   what is new in a poll (default lines)
  --filter \"<command>\"   judge each batch first: prints true/false or exits 0/1
  --max-bytes <n>        largest message an agent receives (default 4096)
  --max-per-hour <n>     stop a subscription above this many messages an hour (default 30; 0 = no limit)
  --idle-stop <duration> stop a feed that delivered nothing for this long (default 7d; 0 = never)
  --stale <secs>         one notice when the source printed nothing for this long
  --label <text>         name shown in messages (default: the command's first word)
  --name <id>            the feed's id (default: derived from the command)
  --cwd <dir>            where the command runs (default: here)
  --no-restore           do not run it again when klark restarts
  --no-small-subagent    leave out the \"let a small subagent judge first\" line
  --foreground           run in this terminal until the command ends or Ctrl-C

Filtering is the command's job: --stream \"tail -n0 -F app.log | grep --line-buffered ERROR\".
Output is sent as untrusted data; do not feed text an outsider can write to an agent that acts without approval.";

pub fn run(args: &[String]) -> ! {
    let (cmd, rest) = match args.split_first() {
        Some((c, r)) => (c.as_str(), r),
        None => usage_exit(0),
    };
    let result = match cmd {
        "add" => cmd_add(rest),
        "list" | "ls" => cmd_list(rest),
        "show" => cmd_show(rest),
        "set" => cmd_set(rest),
        "resume" => cmd_resume(rest),
        "stop" => cmd_stop(rest),
        "start" => cmd_start(rest),
        "leave" => cmd_leave(rest),
        "agent" => cmd_agent(rest),
        "batch" => cmd_batch(rest),
        "logs" | "log" => cmd_logs(rest),
        "_worker" => cmd_worker(rest),
        "-h" | "--help" | "help" => usage_exit(0),
        other => Err(Fail::Usage(format!("unknown command {other}"))),
    };
    match result {
        Ok(()) => std::process::exit(0),
        Err(Fail::Usage(e)) => {
            eprintln!("klark feed: {e}\n\n{USAGE}");
            std::process::exit(2)
        }
        Err(Fail::Error(e)) => {
            eprintln!("klark feed: {e}");
            std::process::exit(1)
        }
    }
}

fn usage_exit(code: i32) -> ! {
    println!("{USAGE}");
    std::process::exit(code)
}

#[derive(Debug, PartialEq)]
enum Fail {
    Usage(String),
    Error(String),
}

impl From<String> for Fail {
    fn from(e: String) -> Self {
        Fail::Error(e)
    }
}

impl From<&str> for Fail {
    fn from(e: &str) -> Self {
        Fail::Error(e.to_string())
    }
}

type R<T> = Result<T, Fail>;

// ---------------------------------------------------------------- arguments

#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Stream(String),
    Follow(String),
    Poll(String),
}

impl Source {
    fn mode(&self) -> Mode {
        match self {
            Source::Stream(_) => Mode::Stream,
            Source::Follow(_) => Mode::Follow,
            Source::Poll(_) => Mode::Poll,
        }
    }

    fn text(&self) -> &str {
        let (Source::Stream(c) | Source::Poll(c) | Source::Follow(c)) = self;
        c
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AddArgs {
    pub agent: String,
    pub instruction: String,
    pub source: Source,
    pub message_gap_secs: u64,
    pub interval_secs: u64,
    pub max_bytes: usize,
    pub max_per_hour: u32,
    pub idle_stop_secs: u64,
    pub stale_secs: Option<u64>,
    pub diff: String,
    pub filter: Option<String>,
    pub label: Option<String>,
    pub name: Option<String>,
    pub cwd: Option<String>,
    pub restore: bool,
    pub use_small_subagent: bool,
    pub foreground: bool,
    /// Per-feed settings the caller gave explicitly (ignored on a join).
    pub explicit: Vec<String>,
}

const WITH_VALUE: &[&str] = &[
    "--stream",
    "--follow",
    "--poll",
    "--message-gap",
    "--interval",
    "--max-bytes",
    "--max-per-hour",
    "--idle-stop",
    "--stale",
    "--diff",
    "--filter",
    "--label",
    "--name",
    "--cwd",
];
const SWITCHES: &[&str] = &["--no-small-subagent", "--no-restore", "--foreground"];
/// Settings that a joiner cannot change (the join itself matches on source,
/// cwd, interval and --diff).
const SETTINGS: &[&str] = &[
    "--message-gap",
    "--max-bytes",
    "--max-per-hour",
    "--idle-stop",
    "--stale",
    "--filter",
    "--label",
    "--name",
    "--no-restore",
    "--no-small-subagent",
];

fn number<T: std::str::FromStr>(flag: &str, v: &str) -> R<T> {
    v.parse()
        .map_err(|_| Fail::Usage(format!("{flag} {v}: a whole number expected")))
}

/// What a `feed add` takes for a flag it was not given: `[feeds]` in
/// `config.toml` (ADR 0015 "Settings"). The result is stored in the feed.
#[derive(Debug, Clone, PartialEq)]
pub struct Defaults {
    pub message_gap_secs: u64,
    pub interval_secs: u64,
    pub max_bytes: usize,
    pub max_per_hour: u32,
    pub idle_stop_secs: u64,
    pub restore: bool,
    pub use_small_subagent: bool,
    pub agents_may_create: bool,
}

impl Default for Defaults {
    fn default() -> Self {
        Defaults {
            message_gap_secs: DEFAULT_GAP_SECS,
            interval_secs: DEFAULT_GAP_SECS,
            max_bytes: DEFAULT_MAX_BYTES,
            max_per_hour: DEFAULT_MAX_PER_HOUR,
            idle_stop_secs: DEFAULT_IDLE_STOP_SECS,
            restore: true,
            use_small_subagent: true,
            agents_may_create: true,
        }
    }
}

impl Defaults {
    /// Values out of range in a hand-edited file are brought to the nearest
    /// allowed one; an unreadable duration is the built-in default.
    fn from_config(c: &FeedsConfig, floor: u64) -> Self {
        Defaults {
            message_gap_secs: c.message_gap_seconds.max(floor),
            interval_secs: c.poll_interval_seconds.max(floor),
            max_bytes: c.max_bytes.max(MIN_MAX_BYTES),
            max_per_hour: c.max_per_hour,
            idle_stop_secs: parse_duration(&c.idle_stop).unwrap_or(DEFAULT_IDLE_STOP_SECS),
            restore: c.restore,
            use_small_subagent: c.use_small_subagent,
            agents_may_create: c.agents_may_create,
        }
    }

    fn load(floor: u64) -> Self {
        Self::from_config(&crate::config::Config::load().config.feeds, floor)
    }
}

const MIN_MAX_BYTES: usize = 256;

/// Parses `feed add`. Flags may come before, between or after the two
/// positionals. `floor` is the least allowed timing value (5, or lower under
/// `KLARK_FEED_TEST=1`); `defaults` fill in what was not given.
fn parse_add(args: &[String], floor: u64, defaults: &Defaults) -> R<AddArgs> {
    let mut flags: Vec<(String, Option<String>)> = Vec::new();
    let mut positional = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "-h" || a == "--help" {
            usage_exit(0);
        }
        if a == "--" {
            positional.extend(it.by_ref().cloned());
            break;
        }
        if let Some((k, v)) = a.split_once('=').filter(|(k, _)| WITH_VALUE.contains(k)) {
            flags.push((k.into(), Some(v.into())));
        } else if WITH_VALUE.contains(&a.as_str()) {
            let v = it
                .next()
                .ok_or_else(|| Fail::Usage(format!("{a} needs a value")))?;
            flags.push((a.clone(), Some(v.clone())));
        } else if SWITCHES.contains(&a.as_str()) {
            flags.push((a.clone(), None));
        } else if a.starts_with("--") {
            return Err(Fail::Usage(format!("unknown flag {a}")));
        } else {
            positional.push(a.clone());
        }
    }
    let value = |k: &str| {
        flags
            .iter()
            .rev()
            .find(|(f, _)| f == k)
            .and_then(|(_, v)| v.clone())
    };
    let has = |k: &str| flags.iter().any(|(f, _)| f == k);

    let [agent, instruction] = positional.as_slice() else {
        return Err(Fail::Usage(
            "expected <agent> \"<instruction>\" (quote the instruction)".into(),
        ));
    };
    if instruction.trim().is_empty() {
        return Err(Fail::Usage("the instruction is empty".into()));
    }

    let sources: Vec<Source> = flags
        .iter()
        .filter_map(|(k, v)| {
            let v = v.clone()?;
            match k.as_str() {
                "--stream" => Some(Source::Stream(v)),
                "--follow" => Some(Source::Follow(v)),
                "--poll" => Some(Source::Poll(v)),
                _ => None,
            }
        })
        .collect();
    let source = match sources.as_slice() {
        [one] => one.clone(),
        [] => return Err(Fail::Usage(
            "say what to watch: --stream \"<command>\", --follow <path> or --poll \"<command>\""
                .into(),
        )),
        _ => {
            return Err(Fail::Usage(
                "give exactly one of --stream, --follow, --poll".into(),
            ))
        }
    };
    if source.text().trim().is_empty() {
        return Err(Fail::Usage("the command or path is empty".into()));
    }
    let is_poll = matches!(source, Source::Poll(_));
    if is_poll && has("--message-gap") {
        return Err(Fail::Usage(
            "--message-gap is for --stream and --follow; a poll feed sends at most once per run, so use --interval".into(),
        ));
    }
    if !is_poll && has("--interval") {
        return Err(Fail::Usage(
            "--interval is for --poll; for --stream and --follow use --message-gap".into(),
        ));
    }
    if !is_poll && has("--diff") {
        return Err(Fail::Usage(
            "--diff is for --poll; a stream or a followed file sends its new lines as they are"
                .into(),
        ));
    }
    let timing = |flag: &str, default: u64| -> R<u64> {
        match value(flag) {
            None => Ok(default),
            Some(v) => {
                let n: u64 = number(flag, &v)?;
                if n < floor {
                    return Err(Fail::Usage(format!("{flag} {n}: at least {floor} seconds")));
                }
                Ok(n)
            }
        }
    };
    let max_bytes = match value("--max-bytes") {
        None => defaults.max_bytes,
        Some(v) => {
            let n: usize = number("--max-bytes", &v)?;
            if n < MIN_MAX_BYTES {
                return Err(Fail::Usage(format!(
                    "--max-bytes {n}: at least {MIN_MAX_BYTES}"
                )));
            }
            n
        }
    };
    let idle_stop_secs = match value("--idle-stop") {
        None => defaults.idle_stop_secs,
        Some(v) => parse_duration(&v).ok_or_else(|| {
            Fail::Usage(format!(
                "--idle-stop {v}: a duration like 7d, 12h, 30m or 0 (never)"
            ))
        })?,
    };
    let stale_secs = match value("--stale") {
        None => None,
        Some(v) => {
            let n: u64 = number("--stale", &v)?;
            if n < floor {
                return Err(Fail::Usage(format!(
                    "--stale {n}: at least {floor} seconds"
                )));
            }
            Some(n)
        }
    };
    let diff = value("--diff").unwrap_or_else(|| "lines".into());
    if diff.trim().is_empty() {
        return Err(Fail::Usage(
            "--diff needs lines, unified or a command".into(),
        ));
    }
    let name = value("--name");
    if let Some(n) = &name {
        if n.is_empty()
            || n.len() > 40
            || !n
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(Fail::Usage(format!(
                "--name {n}: letters, digits, - and _ only"
            )));
        }
    }
    Ok(AddArgs {
        agent: agent.clone(),
        instruction: instruction.clone(),
        message_gap_secs: timing("--message-gap", defaults.message_gap_secs)?,
        interval_secs: timing("--interval", defaults.interval_secs)?,
        max_bytes,
        max_per_hour: value("--max-per-hour")
            .map(|v| number("--max-per-hour", &v))
            .transpose()?
            .unwrap_or(defaults.max_per_hour),
        idle_stop_secs,
        stale_secs,
        diff,
        filter: value("--filter").filter(|f| !f.trim().is_empty()),
        label: value("--label").filter(|l| !l.trim().is_empty()),
        name,
        cwd: value("--cwd"),
        restore: defaults.restore && !has("--no-restore"),
        use_small_subagent: defaults.use_small_subagent && !has("--no-small-subagent"),
        foreground: has("--foreground"),
        explicit: SETTINGS
            .iter()
            .filter(|s| has(s))
            .map(|s| s.to_string())
            .collect(),
        source,
    })
}

fn floor() -> u64 {
    if worker::test_mode() {
        1
    } else {
        MIN_GAP_SECS
    }
}

/// The label shown in messages: `--label`, else the command's first word
/// (a followed file's name). Leading `NAME=value` words are skipped: the
/// label goes into every message, and such a word is often a token.
fn default_label(command: &str) -> String {
    command
        .split_whitespace()
        .find(|w| !is_assignment(w))
        .map(|w| w.rsplit('/').next().unwrap_or(w).to_string())
        .filter(|w| !w.is_empty())
        .unwrap_or_else(|| "feed".into())
}

/// `NAME=value`, a shell variable assignment in front of a command.
fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(k, _)| {
        !k.is_empty()
            && !k.starts_with(|c: char| c.is_ascii_digit())
            && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

fn feed_id(key: &str) -> String {
    let mut h = Sha256::new();
    h.update(key);
    let hex: String = h.finalize()[..4]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("f{hex}")
}

fn open_store() -> R<Store> {
    Store::open(crate::session::data_dir_for(None).join("feeds"))
        .map_err(|e| Fail::Error(format!("feed store: {e}")))
}

fn load_def(store: &Store, id: &str) -> R<FeedDef> {
    store
        .read_json::<FeedDef>(&def_name(id))?
        .ok_or_else(|| Fail::Error(format!("no feed {id} (see klark feed list)")))
}

fn load_status(store: &Store, id: &str) -> Status {
    store
        .read_json(&status_name(id))
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// A worker holds the feed's lock.
fn is_running(store: &Store, id: &str) -> bool {
    matches!(store.try_lock_feed(id), Ok(None))
}

// ---------------------------------------------------------------- callers

fn my_session() -> String {
    crate::session::active_name().unwrap_or_else(|| crate::session::DEFAULT_SESSION_NAME.into())
}

/// The calling pane: (session, pane id, its agent name, whether an agent
/// runs in it). `None` outside a klark pane.
struct Caller {
    session: String,
    pane: String,
    name: Option<String>,
    is_agent: bool,
}

fn caller() -> Option<Caller> {
    let pane = std::env::var("KLARK_PANE_ID")
        .ok()
        .filter(|p| !p.trim().is_empty())?;
    let session = my_session();
    let list = call(&session_socket(&session), "agent.list", json!({})).ok();
    let me = list
        .as_ref()
        .and_then(|l| l.get("agents")?.as_array().cloned())
        .and_then(|a| {
            a.into_iter()
                .find(|a| a.get("pane_id").and_then(Value::as_str) == Some(&pane))
        });
    Some(Caller {
        session,
        name: me
            .as_ref()
            .and_then(|a| a.get("name")?.as_str().map(str::to_string)),
        is_agent: me.is_some(),
        pane,
    })
}

impl Caller {
    fn matches(&self, s: &Sub) -> bool {
        s.session == self.session
            && (s.agent == self.pane || self.name.as_deref() == Some(s.agent.as_str()))
    }

    fn label(&self) -> String {
        format!(
            "{}/{}",
            self.session,
            self.name.as_deref().unwrap_or(&self.pane)
        )
    }
}

/// A local agent, by name or pane id, as `klark msg send` resolves it:
/// (session, pane id, name). The name must be unique in its session: it is
/// how the feed finds the agent again.
fn resolve_agent(to: &str) -> R<(String, String, Option<String>)> {
    let machine = crate::msg_cmd::my_machine();
    let addr = parse_address(to).map_err(Fail::Usage)?;
    if addr.machine.as_deref().is_some_and(|m| m != machine) {
        return Err(Fail::Error(format!(
            "{to}: feeds deliver to local agents only (output would cross the hub)"
        )));
    }
    let local = local_agents(&machine);
    match resolve(&addr, &local, &machine)? {
        Route::Local { session, agent } => {
            let me = local
                .iter()
                .find(|l| l.session == session && l.pane_id == agent);
            let name = me.and_then(|l| l.name.clone());
            if let Some(name) = &name {
                let same = local
                    .iter()
                    .filter(|l| l.session == session && l.name.as_deref() == Some(name))
                    .count();
                if same > 1 {
                    return Err(Fail::Error(format!(
                        "{to}: {same} agents are named {name} in session {session}; rename one (klark msg name)"
                    )));
                }
            }
            Ok((session, agent, name))
        }
        Route::Remote { to } => Err(Fail::Error(format!(
            "{to}: feeds deliver to local agents only"
        ))),
    }
}

// ---------------------------------------------------------------- the worker process

/// Starts the detached worker of feed `id` and waits until it runs (a
/// stream's first second, a poll's first run) or fails. `Err` carries what
/// went wrong, for the terminal.
fn start_worker(store: &Store, def: &FeedDef, resumed: bool) -> Result<(), String> {
    start_worker_within(store, def, resumed, None)
}

/// `start_worker`, waiting at most `cap` for it to run: the socket API
/// answers a client that must not stay blocked for a slow poll's first run.
fn start_worker_within(
    store: &Store,
    def: &FeedDef,
    resumed: bool,
    cap: Option<Duration>,
) -> Result<(), String> {
    let id = &def.id;
    // Nobody else writes the status while no worker holds the lock: written
    // under the lock, so a worker that started meanwhile is not overwritten.
    {
        let Some(_held) = store.try_lock_feed(id).map_err(|e| e.to_string())? else {
            return Ok(()); // already running
        };
        let mut st = load_status(store, id);
        st.state = "starting".into();
        st.detail = None;
        store.write_json(&status_name(id), &st)?;
    }

    let log_path = store.path(&log_name(id));
    if std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0) > 1024 * 1024 {
        let _ = std::fs::rename(&log_path, store.path(&format!("{id}.log.1")));
    }
    store
        .append(&log_name(id), b"")
        .map_err(|e| format!("{}: {e}", log_path.display()))?;
    let log = std::fs::OpenOptions::new()
        .append(true)
        .open(&log_path)
        .map_err(|e| format!("{}: {e}", log_path.display()))?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new(exe);
    cmd.args(["feed", "_worker", id]);
    if resumed {
        cmd.arg("--resumed");
    }
    if std::path::Path::new(&def.cwd).is_dir() {
        cmd.current_dir(&def.cwd);
    }
    cmd.stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|e| e.to_string())?)
        .stderr(log);
    #[cfg(unix)]
    let mut child = {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        cmd.spawn().map_err(|e| e.to_string())?
    };
    #[cfg(windows)]
    let mut child = crate::platform::spawn_detached_feed(&mut cmd).map_err(|e| e.to_string())?;

    let wait = match def.mode {
        Mode::Poll => Duration::from_secs(5 * def.timing_secs.max(12) + 10),
        _ => Duration::from_secs(10),
    };
    // Nobody waits at a terminal for a resume: a slow poll (5 x a long
    // interval) must not keep `feed resume` alive past the server.
    let wait = if resumed {
        wait.min(Duration::from_secs(30))
    } else {
        wait
    };
    let wait = cap.map_or(wait, |cap| wait.min(cap));
    let deadline = Instant::now() + wait;
    loop {
        let st = load_status(store, id);
        if let Ok(Some(exit)) = child.try_wait() {
            let st = load_status(store, id);
            return match st.state.as_str() {
                "failed" => Err(st.detail.unwrap_or_else(|| "the worker failed".into())),
                _ if exit.success() => Ok(()),
                _ => Err(format!(
                    "the worker exited ({exit}); see klark feed logs {id}"
                )),
            };
        }
        match st.state.as_str() {
            "starting" => {}
            "failed" => return Err(st.detail.unwrap_or_else(|| "the worker failed".into())),
            _ => return Ok(()),
        }
        if Instant::now() > deadline {
            return Ok(()); // still starting; `feed list` will tell
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn cmd_worker(args: &[String]) -> R<()> {
    let (id, resumed) = match args {
        [id] => (id, false),
        [id, r] if r == "--resumed" => (id, true),
        _ => return Err(Fail::Usage("_worker <id> [--resumed]".into())),
    };
    let store = open_store()?;
    match worker::run(store, id, true, resumed) {
        Ok(worker::Outcome::Done) => Ok(()),
        Ok(worker::Outcome::AlreadyRunning) => {
            eprintln!("klark feed {id}: already running");
            Ok(())
        }
        Err(e) => Err(Fail::Error(e)),
    }
}

/// Ends feed `id` for good: removes the definition (so nothing restarts it),
/// stops the worker and the command's tree, deletes every file.
fn stop_feed(store: &Store, id: &str) -> R<()> {
    {
        let _g = store.lock_store().map_err(|e| e.to_string())?;
        if store.read_json::<FeedDef>(&def_name(id))?.is_none() {
            return Err(Fail::Error(format!("no feed {id} (see klark feed list)")));
        }
        store.remove(&def_name(id)).map_err(|e| e.to_string())?;
    }
    end_feed(store, id);
    Ok(())
}

/// The rest of a stop, once the definition is gone (removed under the store
/// lock by the caller): the worker, the command's tree, every file.
fn end_feed(store: &Store, id: &str) {
    if is_running(store, id) {
        #[cfg(unix)]
        if let Some(pid) = load_status(store, id).worker_pid {
            // SAFETY: the feed lock is held, so this pid is that feed's worker.
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        }
        // Windows has no SIGTERM for a detached process: the worker is ended
        // outright. Its definition is already gone, so nothing it would have
        // flushed is needed; its commands' jobs close with it.
        #[cfg(windows)]
        if let Some(pid) = load_status(store, id).worker_pid {
            crate::platform::terminate_feed_worker(pid, Duration::from_secs(5));
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while is_running(store, id) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        #[cfg(unix)]
        if is_running(store, id) {
            if let Some(pid) = load_status(store, id).worker_pid {
                // SAFETY: as above; it did not end on SIGTERM.
                unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
            }
        }
    }
    // A command left by a worker that was killed hard.
    worker::reap_leftover(&load_status(store, id));
    store.remove_feed(id);
    if let Ok(Some(_held)) = store.try_lock_feed(id) {
        let _ = store.remove(&format!("{id}.lock"));
    }
}

// ---------------------------------------------------------------- add

fn cmd_add(args: &[String]) -> R<()> {
    let defaults = Defaults::load(floor());
    let a = parse_add(args, floor(), &defaults)?;
    let me = caller();
    if !defaults.agents_may_create && me.as_ref().is_some_and(|c| c.is_agent) {
        return Err(Fail::Error(
            "feeds may not be added from an agent pane ([feeds] agents_may_create = false); ask the user to run klark feed add from a shell"
                .into(),
        ));
    }
    let (session, pane, name) = resolve_agent(&a.agent)?;
    let agent = name.clone().unwrap_or_else(|| pane.clone());
    let cwd = match &a.cwd {
        Some(c) => std::fs::canonicalize(c)
            .map_err(|e| format!("--cwd {c}: {e}"))?
            .display()
            .to_string(),
        None => std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    };
    let mode = a.source.mode();
    let command = match &a.source {
        Source::Follow(p) => {
            let p = std::path::Path::new(p);
            if p.is_absolute() {
                p.display().to_string()
            } else {
                std::path::Path::new(&cwd).join(p).display().to_string()
            }
        }
        other => other.text().to_string(),
    };
    let timing = if mode == Mode::Poll {
        a.interval_secs
    } else {
        a.message_gap_secs
    };
    let diff = if mode == Mode::Poll {
        a.diff.clone()
    } else {
        "lines".into()
    };
    let key = join_key(mode, &command, &cwd, timing, &diff);
    let store = open_store()?;
    let created_by = match me {
        Some(c) if c.is_agent => format!("agent {}", c.label()),
        _ => "user".to_string(),
    };

    let guard = store.lock_store().map_err(|e| e.to_string())?;
    // Joining: the same source, cwd, mode, interval and --diff.
    let existing = store
        .feed_ids()
        .into_iter()
        .filter_map(|id| store.read_json::<FeedDef>(&def_name(&id)).ok().flatten())
        .find(|d| d.join_key() == key);
    if let Some(mut def) = existing {
        let id = def.id.clone();
        if let Some(s) = def
            .subs
            .iter()
            .find(|s| s.session == session && (s.agent == agent || s.agent == pane))
        {
            drop(guard);
            match &s.stopped {
                Some(why) => println!(
                    "klark feed {id}: {session}/{agent} is already subscribed (stopped: {why}); klark feed start {id} runs it again"
                ),
                None => println!("klark feed {id}: {session}/{agent} is already subscribed"),
            }
            return Ok(());
        }
        let n = def.next_sub;
        def.next_sub += 1;
        def.subs.push(Sub {
            n,
            session: session.clone(),
            agent: agent.clone(),
            instruction: a.instruction.clone(),
            stopped: None,
            user_only: false,
            fresh: mode == Mode::Poll,
        });
        store.write_json(&def_name(&id), &def)?;
        drop(guard);
        match &def.stopped {
            // Nothing runs it: say so instead of "joined the running feed".
            Some(why) => println!(
                "klark feed {id}: joined the feed {} ({} agents now); it is stopped ({why}): klark feed start {id} runs it again",
                def.label,
                def.subs.len()
            ),
            None => println!(
                "klark feed {id}: joined the running feed {} ({} agents now)",
                def.label,
                def.subs.len()
            ),
        }
        if !a.explicit.is_empty() {
            println!(
                "note: ignored for a joined feed: {} (it keeps the settings it was added with; klark feed show {id})",
                a.explicit.join(", ")
            );
        }
        if def.stopped.is_none() && !is_running(&store, &id) {
            start_worker(&store, &def, false).map_err(Fail::Error)?;
        }
        return Ok(());
    }

    let id = match &a.name {
        Some(n) => n.clone(),
        None => feed_id(&key),
    };
    if store.read_json::<FeedDef>(&def_name(&id))?.is_some() {
        return Err(Fail::Error(format!(
            "a different feed is already named {id}; choose another --name"
        )));
    }
    let label = a.label.clone().unwrap_or_else(|| default_label(&command));
    let def = FeedDef {
        id: id.clone(),
        mode,
        command: command.clone(),
        cwd: cwd.clone(),
        timing_secs: timing,
        diff,
        filter: a.filter.clone(),
        max_bytes: a.max_bytes,
        max_per_hour: a.max_per_hour,
        idle_stop_secs: a.idle_stop_secs,
        stale_secs: a.stale_secs,
        label: label.clone(),
        use_small_subagent: a.use_small_subagent,
        restore: a.restore,
        created_by,
        created_at: worker::unix_secs(),
        subs: vec![Sub {
            n: 0,
            session: session.clone(),
            agent: agent.clone(),
            instruction: a.instruction.clone(),
            stopped: None,
            user_only: false,
            fresh: false,
        }],
        next_sub: 1,
        stopped: None,
    };
    // Leftovers of an earlier feed of this id (its status, a stale lock).
    store.remove_feed(&id);
    store.write_json(&def_name(&id), &def)?;
    store.write_json(
        &status_name(&id),
        &Status {
            state: "starting".into(),
            ..Status::default()
        },
    )?;
    drop(guard);

    if a.foreground {
        eprintln!(
            "klark feed {id}: {label} → {session}/{agent} (Ctrl-C stops it; klark feed start {id} runs it in the background)"
        );
        return match worker::run(store, &id, false, false) {
            Ok(worker::Outcome::Done) => Ok(()),
            Ok(worker::Outcome::AlreadyRunning) => {
                eprintln!("klark feed {id}: already running in the background");
                Ok(())
            }
            Err(e) => {
                let store = open_store()?;
                let st = load_status(&store, &id);
                if st.state == "failed" {
                    let _g = store.lock_store();
                    store.remove_feed(&id);
                }
                Err(Fail::Error(e))
            }
        };
    }
    match start_worker(&store, &def, false) {
        Ok(()) => {
            println!(
                "klark feed {id}: {} {label} → {session}/{agent}. Stop it: klark feed stop {id}",
                mode.as_str()
            );
            Ok(())
        }
        Err(e) => {
            let _g = store.lock_store();
            store.remove_feed(&id);
            Err(Fail::Error(e))
        }
    }
}

// ---------------------------------------------------------------- set

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Bool,
    Secs,
    Count,
    Bytes,
    Duration,
}

/// `feed set` names (as typed) and their `[feeds]` keys.
const FEED_SETTINGS: &[(&str, &str, Kind)] = &[
    ("restore", "restore", Kind::Bool),
    ("agents-may-create", "agents_may_create", Kind::Bool),
    ("message-gap", "message_gap_seconds", Kind::Secs),
    ("poll-interval", "poll_interval_seconds", Kind::Secs),
    ("max-per-hour", "max_per_hour", Kind::Count),
    ("idle-stop", "idle_stop", Kind::Duration),
    ("max-bytes", "max_bytes", Kind::Bytes),
    ("use-small-subagent", "use_small_subagent", Kind::Bool),
];

/// A setting by its `feed set` name, its `[feeds]` key, or either with `-`
/// and `_` swapped.
fn feed_setting(name: &str) -> Option<(&'static str, &'static str, Kind)> {
    let n = name.trim().replace('_', "-");
    let n = match n.as_str() {
        "message-gap-seconds" => "message-gap",
        "poll-interval-seconds" | "interval" => "poll-interval",
        "small-subagent" => "use-small-subagent",
        other => other,
    };
    FEED_SETTINGS
        .iter()
        .find(|(cli, _, _)| *cli == n)
        .map(|&(c, k, t)| (c, k, t))
}

/// A value as `[feeds]` stores it (TOML), or why it is refused.
fn setting_value(cli: &str, kind: Kind, v: &str, floor: u64) -> R<String> {
    let v = v.trim();
    let bad = |what: &str| Fail::Usage(format!("{cli} {v}: {what}"));
    Ok(match kind {
        Kind::Bool => match v.to_ascii_lowercase().as_str() {
            "on" | "true" | "yes" | "1" => "true".into(),
            "off" | "false" | "no" | "0" => "false".into(),
            _ => return Err(bad("on or off")),
        },
        Kind::Secs => {
            let n: u64 = v.parse().map_err(|_| bad("whole seconds expected"))?;
            if n < floor {
                return Err(bad(&format!("at least {floor} seconds")));
            }
            n.to_string()
        }
        Kind::Count => v
            .parse::<u32>()
            .map_err(|_| bad("a whole number expected (0 = no limit)"))?
            .to_string(),
        Kind::Bytes => {
            let n: usize = v.parse().map_err(|_| bad("a whole number expected"))?;
            if n < MIN_MAX_BYTES {
                return Err(bad(&format!("at least {MIN_MAX_BYTES}")));
            }
            n.to_string()
        }
        Kind::Duration => {
            parse_duration(v).ok_or_else(|| bad("a duration like 7d, 12h, 30m or 0 (never)"))?;
            format!("\"{v}\"")
        }
    })
}

/// The current value of a `[feeds]` key, as written in the file.
fn current_setting(c: &FeedsConfig, key: &str) -> String {
    match key {
        "restore" => c.restore.to_string(),
        "agents_may_create" => c.agents_may_create.to_string(),
        "message_gap_seconds" => c.message_gap_seconds.to_string(),
        "poll_interval_seconds" => c.poll_interval_seconds.to_string(),
        "max_per_hour" => c.max_per_hour.to_string(),
        "idle_stop" => format!("\"{}\"", c.idle_stop),
        "max_bytes" => c.max_bytes.to_string(),
        "use_small_subagent" => c.use_small_subagent.to_string(),
        _ => String::new(),
    }
}

/// `feed set [<setting> [<value>]]`: shows or changes the defaults for new
/// feeds (`[feeds]` in `config.toml`). Never touches an existing feed.
fn cmd_set(args: &[String]) -> R<()> {
    let names = || {
        FEED_SETTINGS
            .iter()
            .map(|(c, _, _)| *c)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let lookup = |n: &str| {
        feed_setting(n)
            .ok_or_else(|| Fail::Usage(format!("unknown setting {n}; one of: {}", names())))
    };
    match args {
        [] => {
            let c = crate::config::Config::load().config.feeds;
            println!("[feeds]");
            for (_, key, _) in FEED_SETTINGS {
                println!("{key} = {}", current_setting(&c, key));
            }
            println!("# klark feed set <setting> <value> changes one; existing feeds keep their own values");
            Ok(())
        }
        [n] => {
            let (_, key, _) = lookup(n)?;
            let c = crate::config::Config::load().config.feeds;
            println!("{key} = {}", current_setting(&c, key));
            Ok(())
        }
        [n, v] => {
            let (cli, key, kind) = lookup(n)?;
            let value = setting_value(cli, kind, v, floor())?;
            // An agent kept from `feed add` may not lift that itself.
            if key == "agents_may_create"
                && !crate::config::Config::load().config.feeds.agents_may_create
                && caller().is_some_and(|c| c.is_agent)
            {
                return Err(Fail::Error(
                    "agents_may_create is off; only the user can turn it on (from a shell)".into(),
                ));
            }
            crate::config::write_edit(crate::config::ConfigEdit::FeedSetting {
                key,
                value: &value,
            })?;
            let c = crate::config::Config::load().config.feeds;
            println!(
                "{key} = {} (in [feeds], {}; existing feeds keep their own values)",
                current_setting(&c, key),
                crate::config::config_path().display()
            );
            Ok(())
        }
        _ => Err(Fail::Usage("set [<setting> [<value>]]".into())),
    }
}

// ---------------------------------------------------------------- resume

/// At server start, once the session is restored: `klark feed resume
/// --session <it>` in the background, with the server's environment. Only
/// when there is a feed at all.
pub(crate) fn spawn_resume() {
    let Ok(store) = open_store() else {
        return;
    };
    if store.feed_ids().is_empty() {
        return;
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let log_path = store.path("resume.log");
    if std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0) > 256 * 1024 {
        let _ = std::fs::remove_file(&log_path);
    }
    let _ = store.append("resume.log", b"");
    let log = std::fs::OpenOptions::new().append(true).open(&log_path);
    let mut cmd = Command::new(exe);
    cmd.args(["feed", "resume", "--session", &my_session()])
        .env_remove("KLARK_PANE_ID")
        .stdin(Stdio::null());
    match log.and_then(|l| Ok((l.try_clone()?, l))) {
        Ok((out, err)) => {
            cmd.stdout(out).stderr(err);
        }
        Err(_) => {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    #[cfg(unix)]
    let spawned = {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
        cmd.spawn()
    };
    #[cfg(windows)]
    let spawned = crate::platform::spawn_detached_feed(&mut cmd);
    match spawned {
        // Reaped off the server's main thread; it ends within seconds.
        Ok(mut child) => {
            let _ = std::thread::Builder::new()
                .name("klark-feed-resume".into())
                .spawn(move || child.wait());
        }
        Err(e) => tracing::warn!(err = %e, "feed resume could not start"),
    }
}

/// `feed resume [--session <name>]`: starts the worker of every feed with an
/// active subscriber in the session, unless it runs already. A feed with
/// `restore` off is marked `stopped: klark restarted` instead.
fn cmd_resume(args: &[String]) -> R<()> {
    let session = match args {
        [] => my_session(),
        [f, s] if f == "--session" && !s.is_empty() => s.clone(),
        _ => return Err(Fail::Usage("resume [--session <name>]".into())),
    };
    let store = open_store()?;
    let (todo, running) = resume_pass(&store, &session, store.feed_ids())?;
    start_resumed(&store, &session, &todo);
    // A worker that saw the old server go (a live handoff, a quick restart)
    // may be ending just now, after the check above: once it has had time
    // to see this server up or to end, start the ones that ended.
    if !running.is_empty() {
        std::thread::sleep(Duration::from_millis(DOWN_EXIT_MS + 2_000));
        let (todo, _) = resume_pass(&store, &session, running)?;
        start_resumed(&store, &session, &todo);
    }
    Ok(())
}

/// The feeds among `ids` to start for `session`, and those skipped because a
/// worker runs. A feed with `restore` off is marked stopped here.
fn resume_pass(store: &Store, session: &str, ids: Vec<String>) -> R<(Vec<FeedDef>, Vec<String>)> {
    let (mut todo, mut running) = (Vec::new(), Vec::new());
    for id in ids {
        let Ok(Some(def)) = store.read_json::<FeedDef>(&def_name(&id)) else {
            continue;
        };
        if def.stopped.is_some() || !def.active_subs().any(|s| s.session == session) {
            continue;
        }
        if is_running(store, &id) {
            running.push(id);
            continue;
        }
        if def.restore {
            todo.push(def);
            continue;
        }
        let _g = store.lock_store().map_err(|e| e.to_string())?;
        if let Ok(mut d) = load_def(store, &id) {
            if d.stopped.is_none() && !is_running(store, &id) {
                d.stopped = Some("klark restarted".into());
                store.write_json(&def_name(&id), &d)?;
                println!("klark feed {id}: restore is off; stopped: klark restarted");
            }
        }
    }
    Ok((todo, running))
}

fn start_resumed(store: &Store, session: &str, todo: &[FeedDef]) {
    // Side by side: a poll feed's first run must not hold up the others.
    std::thread::scope(|scope| {
        for def in todo {
            scope.spawn(move || match start_worker(store, def, true) {
                Ok(()) => println!("klark feed {}: resumed for session {session}", def.id),
                Err(e) => {
                    eprintln!("klark feed {}: could not resume: {e}", def.id);
                    let _g = store.lock_store();
                    if let Ok(mut d) = load_def(store, &def.id) {
                        d.stopped = Some("the command failed when klark restarted".into());
                        let _ = store.write_json(&def_name(&def.id), &d);
                    }
                }
            });
        }
    });
}

// ---------------------------------------------------------------- list / show

fn ago(secs: Option<u64>) -> String {
    let Some(t) = secs else {
        return "-".into();
    };
    let d = worker::unix_secs().saturating_sub(t);
    match d {
        0..=59 => format!("{d}s ago"),
        60..=3599 => format!("{}m ago", d / 60),
        3600..=86_399 => format!("{}h ago", d / 3600),
        _ => format!("{}d ago", d / 86_400),
    }
}

fn clock(secs: u64) -> String {
    time::OffsetDateTime::from_unix_timestamp(secs as i64)
        .map(|t| format!("{:02}:{:02} UTC", t.hour(), t.minute()))
        .unwrap_or_else(|_| secs.to_string())
}

fn feed_state(def: &FeedDef, st: &Status, running: bool) -> String {
    if let Some(why) = &def.stopped {
        return format!("stopped: {why}");
    }
    match (running, st.state.as_str()) {
        (true, "waiting") => format!(
            "waiting: {}",
            st.detail.as_deref().unwrap_or("no agent reachable")
        ),
        (true, s) => s.to_string(),
        (false, "waiting") => format!("not running: {}", st.detail.as_deref().unwrap_or("waiting")),
        (false, _) => "not running".into(),
    }
}

fn sub_line(def: &FeedDef, st: &Status, s: &Sub) -> String {
    let mut out = format!("{}/{}", s.session, s.agent);
    let ss = st.subs.get(&s.n);
    if let Some(why) = &s.stopped {
        out.push_str(&format!(
            " (stopped: {why}{})",
            if s.user_only { "; user only" } else { "" }
        ));
    } else if let Some(w) = ss.and_then(|x| x.waiting.as_ref()) {
        out.push_str(&format!(" (waiting: {w})"));
    } else if let Some(b) = ss.and_then(|x| x.busy_since) {
        if worker::unix_secs().saturating_sub(b) >= 3600 {
            out.push_str(&format!(" (busy since {})", clock(b)));
        }
    }
    let _ = def;
    out
}

fn all_feeds(store: &Store) -> Vec<(FeedDef, Status, bool)> {
    store
        .feed_ids()
        .into_iter()
        .filter_map(|id| {
            let def = store.read_json::<FeedDef>(&def_name(&id)).ok().flatten()?;
            let st = load_status(store, &id);
            let running = is_running(store, &id);
            Some((def, st, running))
        })
        .collect()
}

fn cmd_list(args: &[String]) -> R<()> {
    let json_out = match args {
        [] => false,
        [j] if j == "--json" => true,
        _ => return Err(Fail::Usage("list [--json]".into())),
    };
    let store = open_store()?;
    let feeds = all_feeds(&store);
    if json_out {
        let v: Vec<Value> = feeds
            .iter()
            .map(|(d, s, r)| {
                json!({
                    "id": d.id,
                    "label": d.label,
                    "mode": d.mode.as_str(),
                    "state": feed_state(d, s, *r),
                    "running": r,
                    "agents": d.subs.iter().map(|x| sub_line(d, s, x)).collect::<Vec<_>>(),
                    "last_output": s.last_output,
                    "last_sent": s.last_sent,
                    "filtered": s.filtered,
                    "created_by": d.created_by,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        return Ok(());
    }
    if feeds.is_empty() {
        println!("no feeds (klark feed add --help)");
        return Ok(());
    }
    let rows: Vec<[String; 9]> = feeds
        .iter()
        .map(|(d, s, r)| {
            [
                d.id.clone(),
                d.label.clone(),
                d.mode.as_str().into(),
                d.subs
                    .iter()
                    .map(|x| sub_line(d, s, x))
                    .collect::<Vec<_>>()
                    .join(", "),
                feed_state(d, s, *r),
                ago(s.last_output),
                ago(s.last_sent),
                if s.filtered > 0 {
                    format!("filtered: {}", s.filtered)
                } else {
                    "-".into()
                },
                d.created_by.clone(),
            ]
        })
        .collect();
    let head = [
        "ID",
        "LABEL",
        "MODE",
        "AGENTS",
        "STATE",
        "LAST OUTPUT",
        "LAST SENT",
        "FILTERED",
        "CREATED BY",
    ];
    let mut w = head.map(str::len);
    for r in &rows {
        for (i, c) in r.iter().enumerate() {
            w[i] = w[i].max(c.chars().count());
        }
    }
    let line = |cells: Vec<&str>| {
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{c:<width$}", width = w[i]))
            .collect::<Vec<_>>()
            .join("  ")
            .trim_end()
            .to_string()
    };
    println!("{}", line(head.to_vec()));
    for r in &rows {
        println!("{}", line(r.iter().map(String::as_str).collect()));
    }
    Ok(())
}

fn one_id(args: &[String], verb: &str) -> R<String> {
    match args {
        [id] if !id.starts_with('-') => Ok(id.clone()),
        _ => Err(Fail::Usage(format!("{verb} <id>"))),
    }
}

fn cmd_show(args: &[String]) -> R<()> {
    let id = one_id(args, "show")?;
    let store = open_store()?;
    let d = load_def(&store, &id)?;
    let s = load_status(&store, &id);
    let running = is_running(&store, &id);
    let timing = if d.mode == Mode::Poll {
        "interval"
    } else {
        "message gap"
    };
    println!("feed         {}", d.id);
    println!("label        {}", d.label);
    println!("mode         {}", d.mode.as_str());
    println!(
        "{}  {}",
        if d.mode == Mode::Follow {
            "path       "
        } else {
            "command    "
        },
        d.command
    );
    println!("cwd          {}", d.cwd);
    println!("state        {}", feed_state(&d, &s, running));
    println!(
        "created by   {} ({} )",
        d.created_by,
        ago(Some(d.created_at))
    );
    println!("last output  {}", ago(s.last_output));
    println!("last sent    {}", ago(s.last_sent));
    println!("filtered     {}", s.filtered);
    println!("settings:");
    println!("  {timing:<18} {} s", d.timing_secs);
    if d.mode == Mode::Poll {
        println!("  diff               {}", d.diff);
    }
    println!(
        "  filter             {}",
        d.filter.as_deref().unwrap_or("-")
    );
    println!("  max bytes          {}", d.max_bytes);
    println!(
        "  max per hour       {}",
        if d.max_per_hour == 0 {
            "no limit".to_string()
        } else {
            d.max_per_hour.to_string()
        }
    );
    println!("  idle stop          {}", format_duration(d.idle_stop_secs));
    println!(
        "  stale              {}",
        d.stale_secs
            .map(|s| format!("{s} s"))
            .unwrap_or_else(|| "off".into())
    );
    println!(
        "  restore            {}",
        if d.restore { "on" } else { "off" }
    );
    println!(
        "  small subagent     {}",
        if d.use_small_subagent { "on" } else { "off" }
    );
    println!("agents:");
    for x in &d.subs {
        println!("  {}", sub_line(&d, &s, x));
        println!("    instruction: {}", x.instruction);
    }
    Ok(())
}

// ---------------------------------------------------------------- stop / start

fn cmd_stop(args: &[String]) -> R<()> {
    let store = open_store()?;
    let ids = match args {
        [a] if a == "--all" => store.feed_ids(),
        _ => vec![one_id(args, "stop <id|--all>")?],
    };
    let all = ids.len() > 1 || matches!(args, [a] if a == "--all");
    for id in &ids {
        match stop_feed(&store, id) {
            Ok(()) => println!("klark feed {id}: stopped and removed"),
            // Gone meanwhile (its last agent left): one less to stop. One
            // feed must not keep `--all` from the rest.
            Err(e) if all => eprintln!("klark feed {id}: {e:?}"),
            Err(e) => return Err(e),
        }
    }
    if ids.is_empty() {
        println!("no feeds");
    }
    Ok(())
}

fn cmd_start(args: &[String]) -> R<()> {
    let id = one_id(args, "start")?;
    let store = open_store()?;
    let me = caller().filter(|c| c.is_agent);
    if start_feed(&store, &id, me.as_ref(), None)? {
        println!("klark feed {id}: running");
    } else {
        println!("klark feed {id}: started");
    }
    Ok(())
}

/// Clears the stop state (only `me`'s own subscription for an agent pane;
/// the feed and every subscription for the user) and runs the worker.
/// `Ok(true)` when it was already running.
fn start_feed(store: &Store, id: &str, me: Option<&Caller>, cap: Option<Duration>) -> R<bool> {
    let def = {
        let _g = store.lock_store().map_err(|e| e.to_string())?;
        let mut def = load_def(store, id)?;
        match me {
            // From an agent pane: only that agent's own subscription.
            Some(c) => {
                let s = def.subs.iter_mut().find(|s| c.matches(s)).ok_or_else(|| {
                    Fail::Error(format!(
                        "{} is not subscribed to {id}; the user can start it from a shell",
                        c.label()
                    ))
                })?;
                if s.user_only {
                    return Err(Fail::Error(format!(
                        "{id} was stopped for too many updates twice within 24 hours; only the user can start it again, from a shell (klark feed start {id}). Tell the user."
                    )));
                }
                s.stopped = None;
            }
            // From a user shell: the feed and every stopped subscription.
            None => {
                for s in def.subs.iter_mut() {
                    s.stopped = None;
                    s.user_only = false;
                }
            }
        }
        def.stopped = None;
        store.write_json(&def_name(id), &def)?;
        def
    };
    if is_running(store, id) {
        return Ok(true);
    }
    match start_worker_within(store, &def, false, cap) {
        Ok(()) => Ok(false),
        Err(e) => {
            let _g = store.lock_store();
            if let Ok(mut d) = load_def(store, id) {
                d.stopped = Some("the command failed at start".into());
                let _ = store.write_json(&def_name(id), &d);
            }
            Err(Fail::Error(e))
        }
    }
}

// ---------------------------------------------------------------- socket API

// `feed.list | stop | start` (ADR 0015, M5): the app's feeds overlay asks the
// server, since the client may be on another machine. The server acts as the
// user: these are never on the hub link allow-list.

fn fail_text(f: Fail) -> String {
    match f {
        Fail::Usage(e) | Fail::Error(e) => e,
    }
}

/// Every feed, for `feed.list`.
pub(crate) fn api_list() -> Result<Vec<crate::api::schema::FeedInfo>, String> {
    let store = open_store().map_err(fail_text)?;
    Ok(all_feeds(&store)
        .iter()
        .map(|(d, s, r)| crate::api::schema::FeedInfo {
            id: d.id.clone(),
            label: d.label.clone(),
            kind: d.mode.as_str().into(),
            agents: d.subs.iter().map(|x| sub_line(d, s, x)).collect(),
            state: feed_state(d, s, *r),
            running: *r,
            last_delivery: s.last_sent,
        })
        .collect())
}

/// An id from the socket names a file in the store: only the characters a
/// feed id can have (`feed_id`, `--name`), never a path.
fn api_id(id: &str) -> Result<&str, String> {
    if id.is_empty()
        || id.starts_with('-')
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("invalid feed id {id:?}"));
    }
    Ok(id)
}

/// `feed.stop`: as `klark feed stop <id>`, the feed ends for good.
pub(crate) fn api_stop(id: &str) -> Result<(), String> {
    let id = api_id(id)?;
    let store = open_store().map_err(fail_text)?;
    stop_feed(&store, id).map_err(fail_text)
}

/// `feed.start`: as `klark feed start <id>` from a user shell; waits at
/// most a few seconds for the worker. Returns the feed's state afterwards.
pub(crate) fn api_start(id: &str) -> Result<String, String> {
    let id = api_id(id)?;
    let store = open_store().map_err(fail_text)?;
    start_feed(&store, id, None, Some(Duration::from_secs(5))).map_err(fail_text)?;
    let def = load_def(&store, id).map_err(fail_text)?;
    Ok(feed_state(
        &def,
        &load_status(&store, id),
        is_running(&store, id),
    ))
}

// ---------------------------------------------------------------- subscribers

/// Removes the subscriptions `pick` selects; the last one leaving ends the
/// feed. Returns how many were removed.
fn unsubscribe(store: &Store, id: &str, pick: impl Fn(&Sub) -> bool) -> R<(usize, bool)> {
    let (removed, empty) = {
        let _g = store.lock_store().map_err(|e| e.to_string())?;
        let mut def = load_def(store, id)?;
        let gone: Vec<u32> = def.subs.iter().filter(|s| pick(s)).map(|s| s.n).collect();
        def.subs.retain(|s| !pick(s));
        for n in &gone {
            let _ = store.remove(&pending_name(id, *n));
        }
        if !gone.is_empty() {
            if def.subs.is_empty() {
                // Under the same lock: a `feed add` waiting for it must not
                // join a feed that is about to be deleted.
                store.remove(&def_name(id)).map_err(|e| e.to_string())?;
            } else {
                store.write_json(&def_name(id), &def)?;
            }
        }
        (gone.len(), def.subs.is_empty())
    };
    if removed > 0 && empty {
        end_feed(store, id);
    }
    Ok((removed, empty))
}

fn cmd_leave(args: &[String]) -> R<()> {
    let id = one_id(args, "leave")?;
    let me = caller().ok_or("leave works inside a klark pane (KLARK_PANE_ID is not set)")?;
    let store = open_store()?;
    let (removed, ended) = unsubscribe(&store, &id, |s| me.matches(s))?;
    if removed == 0 {
        return Err(Fail::Error(format!(
            "{} is not subscribed to {id}",
            me.label()
        )));
    }
    println!(
        "klark feed {id}: {} left{}",
        me.label(),
        if ended {
            "; it was the last agent, so the feed is stopped"
        } else {
            ""
        }
    );
    Ok(())
}

fn cmd_agent(args: &[String]) -> R<()> {
    match args.split_first() {
        Some((v, rest)) if v == "add" => {
            let (id, to, instruction) = match rest {
                [id, to] => (id, to, None),
                [id, to, i] => (id, to, Some(i.clone())),
                _ => {
                    return Err(Fail::Usage(
                        "agent add <id> <agent> [\"<instruction>\"]".into(),
                    ))
                }
            };
            let (session, pane, name) = resolve_agent(to)?;
            let agent = name.unwrap_or(pane.clone());
            let store = open_store()?;
            let def = {
                let _g = store.lock_store().map_err(|e| e.to_string())?;
                let mut def = load_def(&store, id)?;
                if def
                    .subs
                    .iter()
                    .any(|s| s.session == session && (s.agent == agent || s.agent == pane))
                {
                    println!("klark feed {id}: {session}/{agent} is already subscribed");
                    return Ok(());
                }
                let instruction = instruction
                    .or_else(|| def.subs.first().map(|s| s.instruction.clone()))
                    .ok_or("give an instruction for the new agent")?;
                let n = def.next_sub;
                def.next_sub += 1;
                def.subs.push(Sub {
                    n,
                    session: session.clone(),
                    agent: agent.clone(),
                    instruction,
                    stopped: None,
                    user_only: false,
                    fresh: def.mode == Mode::Poll,
                });
                store.write_json(&def_name(id), &def)?;
                def
            };
            println!("klark feed {id}: {session}/{agent} subscribed");
            if def.stopped.is_none() && !is_running(&store, id) {
                start_worker(&store, &def, false).map_err(Fail::Error)?;
            }
            Ok(())
        }
        Some((v, rest)) if v == "remove" || v == "rm" => {
            let [id, to] = rest else {
                return Err(Fail::Usage("agent remove <id> <agent>".into()));
            };
            let addr = parse_address(to).map_err(Fail::Usage)?;
            let store = open_store()?;
            let (removed, ended) = unsubscribe(&store, id, |s| {
                s.agent == addr.agent && addr.session.as_ref().is_none_or(|x| *x == s.session)
            })?;
            if removed == 0 {
                return Err(Fail::Error(format!("{to} is not subscribed to {id}")));
            }
            println!(
                "klark feed {id}: {to} unsubscribed{}",
                if ended {
                    "; it was the last agent, so the feed is stopped"
                } else {
                    ""
                }
            );
            Ok(())
        }
        _ => Err(Fail::Usage(
            "agent add <id> <agent> [\"<instruction>\"] | agent remove <id> <agent>".into(),
        )),
    }
}

// ---------------------------------------------------------------- batch / logs

fn cmd_batch(args: &[String]) -> R<()> {
    let [id, n] = args else {
        return Err(Fail::Usage("batch <id> <n>".into()));
    };
    let n: u64 = number("batch", n)?;
    let store = open_store()?;
    match store
        .read(&format!("{id}.batch-{n}"))
        .map_err(|e| e.to_string())?
    {
        Some(b) => {
            use std::io::Write;
            let _ = std::io::stdout().write_all(&b);
            Ok(())
        }
        None => Err(Fail::Error(format!(
            "batch {n} of feed {id} is gone (only the last 5 are kept)"
        ))),
    }
}

fn cmd_logs(args: &[String]) -> R<()> {
    let id = one_id(args, "logs")?;
    let store = open_store()?;
    let mut any = false;
    for name in [format!("{id}.log.1"), log_name(&id)] {
        if let Some(b) = store.read(&name).map_err(|e| e.to_string())? {
            use std::io::Write;
            let _ = std::io::stdout().write_all(&b);
            any = true;
        }
    }
    if !any {
        return Err(Fail::Error(format!("no log for feed {id}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|a| a.to_string()).collect()
    }

    /// ADR: a subscriber whose agent has not been idle for an hour shows as
    /// `busy since <time>`; less than an hour, waiting or stopped, it does not.
    #[test]
    fn busy_since_shows_after_an_hour() {
        let sub = Sub {
            n: 0,
            session: "work".into(),
            agent: "rev".into(),
            instruction: "x".into(),
            stopped: None,
            user_only: false,
            fresh: false,
        };
        let def = FeedDef {
            id: "f".into(),
            mode: Mode::Stream,
            command: "true".into(),
            cwd: "/".into(),
            timing_secs: 30,
            diff: "lines".into(),
            filter: None,
            max_bytes: 4096,
            max_per_hour: 30,
            idle_stop_secs: 0,
            stale_secs: None,
            label: "true".into(),
            use_small_subagent: true,
            restore: true,
            created_by: "user".into(),
            created_at: 0,
            subs: vec![sub.clone()],
            next_sub: 1,
            stopped: None,
        };
        let now = worker::unix_secs();
        let with = |busy: Option<u64>, waiting: Option<&str>| {
            let mut st = Status::default();
            let ss = st.subs.entry(0).or_default();
            ss.busy_since = busy;
            ss.waiting = waiting.map(str::to_string);
            st
        };
        assert_eq!(sub_line(&def, &with(None, None), &sub), "work/rev");
        assert_eq!(
            sub_line(&def, &with(Some(now - 3599), None), &sub),
            "work/rev"
        );
        let line = sub_line(&def, &with(Some(now - 3600), None), &sub);
        assert!(line.starts_with("work/rev (busy since "), "{line}");
        assert!(line.ends_with(" UTC)"), "{line}");
        // Waiting says why instead.
        let line = sub_line(&def, &with(Some(now - 7200), Some("pane gone")), &sub);
        assert_eq!(line, "work/rev (waiting: pane gone)");
        let mut stopped = sub.clone();
        stopped.stopped = Some("too many updates".into());
        let line = sub_line(&def, &with(Some(now - 7200), None), &stopped);
        assert_eq!(line, "work/rev (stopped: too many updates)");
    }

    fn usage_err(s: &[&str]) -> String {
        match parse_add(&args(s), 5, &Defaults::default()) {
            Err(Fail::Usage(e)) => e,
            other => panic!("expected a usage error, got {other:?}"),
        }
    }

    #[test]
    fn stream_with_defaults() {
        let a = parse_add(
            &args(&["claude-1", "watch it", "--stream", "tail -F x"]),
            5,
            &Defaults::default(),
        )
        .unwrap();
        assert_eq!(a.source, Source::Stream("tail -F x".into()));
        assert_eq!(a.message_gap_secs, 30);
        assert_eq!(a.max_bytes, 4096);
        assert_eq!(a.max_per_hour, 30);
        assert!(a.use_small_subagent);
        assert!(!a.foreground);
    }

    #[test]
    fn flags_anywhere_and_equals_form() {
        let a = parse_add(
            &args(&[
                "--stream=tail x",
                "claude-1",
                "--message-gap",
                "10",
                "watch",
                "--no-small-subagent",
                "--foreground",
            ]),
            5,
            &Defaults::default(),
        )
        .unwrap();
        assert_eq!(a.message_gap_secs, 10);
        assert!(!a.use_small_subagent && a.foreground);
        assert_eq!(a.instruction, "watch");
    }

    #[test]
    fn no_source_and_two_sources() {
        assert!(usage_err(&["a", "i"]).contains("say what to watch"));
        assert!(usage_err(&["a", "i", "--stream", "x", "--poll", "y"]).contains("exactly one"));
    }

    #[test]
    fn timing_option_must_match_the_source() {
        assert!(
            usage_err(&["a", "i", "--stream", "x", "--interval", "30"]).contains("--message-gap")
        );
        assert!(usage_err(&["a", "i", "--poll", "x", "--message-gap", "30"]).contains("--interval"));
        assert!(
            usage_err(&["a", "i", "--follow", "/x", "--interval", "30"]).contains("--message-gap")
        );
    }

    #[test]
    fn floor_of_five() {
        assert!(
            usage_err(&["a", "i", "--stream", "x", "--message-gap", "4"]).contains("at least 5")
        );
        assert!(usage_err(&["a", "i", "--poll", "x", "--interval", "1"]).contains("at least 5"));
        let a = parse_add(
            &args(&["a", "i", "--stream", "x", "--message-gap", "1"]),
            1,
            &Defaults::default(),
        )
        .unwrap();
        assert_eq!(a.message_gap_secs, 1);
    }

    #[test]
    fn bad_values_and_shapes() {
        assert!(
            usage_err(&["a", "i", "--stream", "x", "--message-gap", "soon"])
                .contains("whole number")
        );
        assert!(usage_err(&["a", "--stream", "x"]).contains("<agent>"));
        assert!(usage_err(&["a", "i", "--stream"]).contains("needs a value"));
        assert!(usage_err(&["a", "i", "--stream", "x", "--match", "y"])
            .contains("unknown flag --match"));
        assert!(
            usage_err(&["a", "i", "--stream", "x", "--max-bytes", "10"]).contains("at least 256")
        );
    }

    #[test]
    fn defaults_fill_in_and_a_flag_wins() {
        let d = Defaults {
            message_gap_secs: 12,
            interval_secs: 40,
            max_bytes: 9000,
            max_per_hour: 60,
            idle_stop_secs: 3600,
            restore: false,
            use_small_subagent: false,
            agents_may_create: true,
        };
        let a = parse_add(&args(&["a", "i", "--stream", "x"]), 5, &d).unwrap();
        assert_eq!(
            (
                a.message_gap_secs,
                a.max_bytes,
                a.max_per_hour,
                a.idle_stop_secs
            ),
            (12, 9000, 60, 3600)
        );
        assert!(!a.restore && !a.use_small_subagent);
        let a = parse_add(
            &args(&[
                "a",
                "i",
                "--poll",
                "x",
                "--max-bytes",
                "300",
                "--idle-stop",
                "0",
            ]),
            5,
            &d,
        )
        .unwrap();
        assert_eq!(
            (a.interval_secs, a.max_bytes, a.idle_stop_secs),
            (40, 300, 0)
        );
    }

    #[test]
    fn config_values_are_brought_into_range() {
        let c = FeedsConfig {
            message_gap_seconds: 1,
            max_bytes: 10,
            idle_stop: "soon".into(),
            ..FeedsConfig::default()
        };
        let d = Defaults::from_config(&c, 5);
        assert_eq!(d.message_gap_secs, 5);
        assert_eq!(d.max_bytes, MIN_MAX_BYTES);
        assert_eq!(d.idle_stop_secs, DEFAULT_IDLE_STOP_SECS);
        assert_eq!(
            Defaults::from_config(&FeedsConfig::default(), 5),
            Defaults::default()
        );
    }

    #[test]
    fn set_names_and_values() {
        assert_eq!(feed_setting("max-bytes").unwrap().1, "max_bytes");
        assert_eq!(feed_setting("max_bytes").unwrap().1, "max_bytes");
        assert_eq!(
            feed_setting("message_gap_seconds").unwrap().1,
            "message_gap_seconds"
        );
        assert!(feed_setting("colour").is_none());
        let v = |n: &str, x: &str| {
            let (c, _, k) = feed_setting(n).unwrap();
            setting_value(c, k, x, 5)
        };
        assert_eq!(v("restore", "off").unwrap(), "false");
        assert_eq!(v("idle-stop", "30d").unwrap(), "\"30d\"");
        assert_eq!(v("max-per-hour", "0").unwrap(), "0");
        assert!(matches!(v("message-gap", "4"), Err(Fail::Usage(e)) if e.contains("at least 5")));
        assert!(matches!(v("max-bytes", "100"), Err(Fail::Usage(e)) if e.contains("at least 256")));
        assert!(matches!(v("idle-stop", "soon"), Err(Fail::Usage(_))));
        assert!(matches!(v("restore", "maybe"), Err(Fail::Usage(_))));
    }

    #[test]
    fn label_and_id() {
        assert_eq!(default_label("/usr/bin/tail -F x | grep y"), "tail");
        // I26: the label is in every message; a leading token is not.
        assert_eq!(
            default_label("API_TOKEN=sekret-1 X2=y curl -fsS https://e.x"),
            "curl"
        );
        assert_eq!(default_label("A=1"), "feed");
        assert_eq!(default_label("grep a=b log"), "grep");
        let a = join_key(Mode::Stream, "tail", "/a", 0, "lines");
        let b = join_key(Mode::Stream, "tail", "/b", 0, "lines");
        assert_eq!(feed_id(&a), feed_id(&a));
        assert_ne!(feed_id(&a), feed_id(&b));
    }

    /// `feed.stop | start` take any string from the socket; an id that no
    /// feed could have (a path) is refused before the store is touched.
    #[test]
    fn api_refuses_ids_that_are_not_feed_ids() {
        for bad in ["../x", "a/b", "", ".", "x\\y", "-all"] {
            assert!(
                api_stop(bad).unwrap_err().contains("invalid feed id"),
                "{bad}"
            );
            assert!(
                api_start(bad).unwrap_err().contains("invalid feed id"),
                "{bad}"
            );
        }
    }
}
