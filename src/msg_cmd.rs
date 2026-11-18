//! `klark msg` — agents message agents, across sessions and machines (ADR 0009).
//!
//! Same machine: open each running session's API socket and call `agent.list`
//! / `agent.prompt` directly. Another machine: one plain-HTTP call to the hub
//! (`POST /api/messages`), which pushes the message over the target's link; the
//! link runs `klark msg deliver` there, so local and remote delivery share code.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const USAGE: &str = "usage: klark msg <command> ...

Agents message agents: in this session, in another klark session on this
machine, or on another machine linked to the same hub (ADR 0009).

Addresses: <agent> | <session>/<agent> | <machine>/<session>/<agent> | parent
  agent   = an agent name or a pane id; agents are named after their folder
            automatically (change it: klark msg name / klark agent rename)
  session = a klark session name; the default session is \"default\"
  machine = the name the hub knows the machine by

commands:
  whoami [--json]                        this agent's own address
  ls [--local] [--json]                  agents in every running session, worktrees included (+ the hub's, unless --local):
                                         address, status, kind, main checkout or worktree, parent, folder, title
  send [--from <addr>] [--json] <to> <text...>
                                         deliver a message as a prompt to another agent
  send --wait-reply <secs> <to> <text...>
                                         send, then wait for the answer and print it
                                         (works outside a klark pane: shells, scripts, other agents)
  send --wait <secs> <to> <text...>      wait until the agent is idle (not working, not on a dialog), then send
  send --all [--json] <text...>          every other agent in this session (worktrees included)
  send --all-sessions [--json] <text...> every other agent in every running session on this machine
  send --task <id|new> [--owner U] [--parent P] [--correlation C] [--wait-reply <secs>] <to> <text...>
                                         a durable task: recorded in SQLite, answered with
                                         reply --task, read with klark task get/wait (prints the id)
  reply [--to <addr>] <text...>          answer the agent that messaged this pane (--to: that
                                         sender, as the message's hint gives it; else the last one)
  reply --task <id> <result...>          answer a task sent to this pane (stores the result)
  log [-n N] [--json]                    recent sent/received messages
  name <new-name>                        rename this pane's agent (its address)
  login --hub <url> --user <name> [--name <machine>] [--password-stdin]
                                         get a hub token (JWT) for cross-machine messages
  logout                                 forget the saved hub token
  status                                 hub, token expiry, running sessions
  deliver --session <s> --from <addr> <agent> <text...>
                                         deliver locally (used by klark expose)

Options go before <to>; everything after <to> is the message, verbatim
(so the text may contain --json or --from).

env: KLARK_HUB_URL, KLARK_HUB_JWT, KLARK_HUB_TOKEN (machine token), KLARK_MACHINE_NAME";

const SOCKET_TIMEOUT_NOTE: &str = "is that klark session running?";
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

pub fn run(args: &[String]) -> ! {
    let (cmd, rest) = match args.split_first() {
        Some((c, r)) => (c.as_str(), r),
        None => usage_exit(0),
    };
    let result = match cmd {
        "whoami" => cmd_whoami(rest),
        "ls" | "list" => cmd_ls(rest),
        "send" => cmd_send(rest),
        "reply" => cmd_reply(rest),
        "deliver" => cmd_deliver(rest),
        "log" => cmd_log(rest),
        "login" => cmd_login(rest),
        "logout" => cmd_logout(),
        "status" => cmd_status(),
        "name" | "rename" => cmd_name(rest),
        "-h" | "--help" | "help" => usage_exit(0),
        other => {
            eprintln!("klark msg: unknown command {other}\n{USAGE}");
            std::process::exit(2);
        }
    };
    match result {
        Ok(()) => std::process::exit(0),
        Err(Fail::Usage(e)) => {
            eprintln!("klark msg: {e}\n\n{USAGE}");
            std::process::exit(2)
        }
        Err(Fail::Error(e)) => {
            eprintln!("klark msg: {e}");
            std::process::exit(1)
        }
    }
}

fn usage_exit(code: i32) -> ! {
    println!("{USAGE}");
    std::process::exit(code)
}

#[derive(Debug)]
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

/// Flags (with or without values) and positionals. Options come first: the
/// first positional (`<to>`, `<agent>` or the first text word) ends option
/// parsing, and everything after it is taken verbatim as text, so a message
/// may contain `--json` or `--from`. A `--` before the first positional, or
/// right after it, is consumed as the explicit end of options.
#[derive(Debug, Default)]
struct Args {
    flags: Vec<(String, Option<String>)>,
    positional: Vec<String>,
}

impl Args {
    fn parse(args: &[String], with_value: &[&str], switches: &[&str]) -> R<Self> {
        let mut out = Args::default();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if a == "--" {
                out.positional.extend(it.by_ref().cloned());
                break;
            }
            if a == "-h" || a == "--help" {
                usage_exit(0);
            }
            if let Some((k, v)) = a.split_once('=').filter(|(k, _)| with_value.contains(k)) {
                out.flags.push((k.to_string(), Some(v.to_string())));
            } else if with_value.contains(&a.as_str()) {
                let v = it
                    .next()
                    .ok_or_else(|| Fail::Usage(format!("{a} needs a value")))?;
                out.flags.push((a.clone(), Some(v.clone())));
            } else if switches.contains(&a.as_str()) {
                out.flags.push((a.clone(), None));
            } else if a.starts_with('-') && a.len() > 1 {
                return Err(Fail::Usage(format!("unknown flag {a}")));
            } else {
                out.positional.push(a.clone());
                let mut rest = it.by_ref().peekable();
                if rest.peek().is_some_and(|next| *next == "--") {
                    rest.next();
                }
                out.positional.extend(rest.cloned());
                break;
            }
        }
        Ok(out)
    }

    /// Options anywhere, positionals in between (`klark task`): an option
    /// after the id still counts, so `get <id> --owner U` is never read as
    /// text and silently unscoped. `--` ends options.
    fn parse_anywhere(args: &[String], with_value: &[&str], switches: &[&str]) -> R<Self> {
        let mut out = Args::default();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            if a == "--" {
                out.positional.extend(it.by_ref().cloned());
                break;
            }
            if let Some((k, v)) = a.split_once('=').filter(|(k, _)| with_value.contains(k)) {
                out.flags.push((k.to_string(), Some(v.to_string())));
            } else if with_value.contains(&a.as_str()) {
                let v = it
                    .next()
                    .ok_or_else(|| Fail::Usage(format!("{a} needs a value")))?;
                out.flags.push((a.clone(), Some(v.clone())));
            } else if switches.contains(&a.as_str()) {
                out.flags.push((a.clone(), None));
            } else if a.starts_with('-') && a.len() > 1 {
                return Err(Fail::Usage(format!("unknown flag {a}")));
            } else {
                out.positional.push(a.clone());
            }
        }
        Ok(out)
    }

    fn has(&self, name: &str) -> bool {
        self.flags.iter().any(|(k, _)| k == name)
    }

    fn value(&self, name: &str) -> Option<String> {
        self.flags
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .and_then(|(_, v)| v.clone())
    }
}

// ---------------------------------------------------------------- addresses

// Addresses, resolution and the per-session agent index live in
// `crate::agent_index`, shared with `klark agent` (cross-session targets).
use crate::agent_index::{
    agents_from_list, call, call_detailed, local_agents, running_sessions, session_socket,
};
pub use crate::agent_index::{parse_address, resolve, Address, LocalAgent, Route};

/// How a delivered message is framed for the receiving agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Frame<'a> {
    /// `[klark msg from <from>] <text>` plus the reply hint (ADR 0009).
    Msg,
    /// A hook notice: `[klark hook <name>] <text>`. No reply hint, and the
    /// sender is not recorded for `klark msg reply`: a hook is not an agent.
    Hook(&'a str),
    /// A durable task (klark#25): `[klark msg from <from>] [task <id>] <text>`
    /// plus the hint to answer it with `klark msg reply --task <id>`.
    Task(&'a str),
}

impl Frame<'_> {
    fn prompt(&self, from: &str, text: &str) -> String {
        match self {
            Frame::Msg => format_prompt(from, text),
            Frame::Hook(name) => format_hook_notice(name, text),
            Frame::Task(id) => format_task_prompt(from, id, text),
        }
    }
}

/// The prompt an agent sees for a task.
pub fn format_task_prompt(from: &str, id: &str, text: &str) -> String {
    format!(
        "[klark msg from {from}] [task {id}] {text}\n(when done, answer it with: klark msg reply --task {id} \"<result>\")"
    )
}

/// The prompt an agent sees for a hook notice.
pub fn format_hook_notice(hook: &str, text: &str) -> String {
    format!("[klark hook {hook}] {text}")
}

/// The sender recorded in `klark msg log` for a hook notice. It is not an
/// address: `:` is not allowed in a session or machine name and there is no
/// `/`, so it can never name or route to a session (one called `hooks`, say).
pub fn hook_sender(hook: &str) -> String {
    format!("klark-hook:{hook}")
}

/// The prompt the receiving agent sees (ADR 0009, Delivery).
pub fn format_prompt(from: &str, text: &str) -> String {
    // The hint names the sender: a pane can be messaged by several agents
    // before it answers, and "the last one" would then be the wrong one.
    format!("[klark msg from {from}] {text}\n(reply: klark msg reply --to {from} \"<text>\")")
}

// ---------------------------------------------------------------- identity

fn my_session() -> String {
    crate::session::active_name().unwrap_or_else(|| crate::session::DEFAULT_SESSION_NAME.into())
}

pub(crate) fn my_machine() -> String {
    std::env::var("KLARK_MACHINE_NAME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| HubConfig::load().and_then(|h| h.name))
        .unwrap_or_else(crate::expose_cmd::hostname)
}

fn my_pane() -> Option<String> {
    std::env::var("KLARK_PANE_ID")
        .ok()
        .filter(|p| !p.trim().is_empty())
}

#[derive(Debug, Serialize)]
struct Me {
    machine: String,
    session: String,
    agent: String,
    pane_id: Option<String>,
    address: String,
}

/// This caller's machine/session/agent address (`cli` outside a pane).
pub(crate) fn my_address() -> String {
    whoami().address
}

fn whoami() -> Me {
    let machine = my_machine();
    let session = my_session();
    let pane_id = my_pane();
    let agent = match &pane_id {
        None => "cli".to_string(),
        Some(pane) => call(
            &crate::session::active_api_socket_path(),
            "agent.list",
            json!({}),
        )
        .ok()
        .map(|r| agents_from_list(&session, &machine, &r))
        .and_then(|list| list.into_iter().find(|a| &a.pane_id == pane))
        .map(|a| a.handle().to_string())
        .unwrap_or_else(|| pane.clone()),
    };
    let address = format!("{machine}/{session}/{agent}");
    Me {
        machine,
        session,
        agent,
        pane_id,
        address,
    }
}

// ---------------------------------------------------------------- hub config

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HubConfig {
    pub hub: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jwt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

fn hub_json_path() -> PathBuf {
    crate::config::config_dir().join("hub.json")
}

impl HubConfig {
    fn load() -> Option<Self> {
        Self::load_from(&hub_json_path())
    }

    pub fn load_from(path: &Path) -> Option<Self> {
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
    }

    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let body = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let tmp = path.with_extension("json.tmp");
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts
            .open(&tmp)
            .map_err(|e| format!("{}: {e}", tmp.display()))?;
        f.write_all(body.as_bytes())
            .map_err(|e| format!("{}: {e}", tmp.display()))?;
        drop(f);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// The hub's HTTP base: KLARK_HUB_URL, else hub.json.
pub(crate) fn hub_base() -> Option<Result<String, String>> {
    let url = std::env::var("KLARK_HUB_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| crate::hub_cmd::default_hub().map(|h| h.url))
        .or_else(|| HubConfig::load().map(|h| h.hub).filter(|h| !h.is_empty()))?;
    Some(klark_link::http::hub_http_base(&url))
}

/// Bearer token precedence: KLARK_HUB_JWT, hub.json jwt, KLARK_HUB_TOKEN.
pub fn pick_token(
    env_jwt: Option<String>,
    saved_jwt: Option<String>,
    machine: Option<String>,
) -> Option<String> {
    let set = |v: Option<String>| v.filter(|s| !s.trim().is_empty());
    set(env_jwt).or(set(saved_jwt)).or(set(machine))
}

fn hub_token() -> Option<String> {
    pick_token(
        std::env::var("KLARK_HUB_JWT").ok(),
        saved_jwt_for_target(),
        std::env::var("KLARK_HUB_TOKEN")
            .ok()
            .or_else(|| crate::hub_cmd::default_hub().map(|h| h.token)),
    )
}

/// hub.json's login, only when it is for the hub we are talking to (the
/// default `klark hub` hub may be another one).
fn saved_jwt_for_target() -> Option<String> {
    let saved = HubConfig::load()?;
    let target = hub_base()?.ok()?;
    let mine = klark_link::http::hub_http_base(&saved.hub).ok()?;
    (mine == target).then_some(saved.jwt).flatten()
}

fn hub_target() -> Result<(String, String), String> {
    let base = hub_base().ok_or(
        "no hub configured: run `klark msg login --hub <url> --user <name>` or set KLARK_HUB_URL",
    )??;
    let token = hub_token()
        .ok_or("no hub token: run `klark msg login`, or set KLARK_HUB_JWT / KLARK_HUB_TOKEN")?;
    Ok((base, token))
}

/// Seconds-since-epoch `exp` claim of a JWT, if readable (no verification).
pub fn jwt_exp(jwt: &str) -> Option<i64> {
    use base64::Engine as _;
    let payload = jwt.split('.').nth(1)?;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice::<Value>(&raw)
        .ok()?
        .get("exp")?
        .as_i64()
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

// ---------------------------------------------------------------- message log

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    pub ts: String,
    /// "sent" or "received"
    pub dir: String,
    pub from: String,
    pub to: String,
    pub text: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn data_dir() -> PathBuf {
    crate::session::data_dir_for(None)
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

pub fn append_log(dir: &Path, entry: &LogEntry) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut line = serde_json::to_string(entry).map_err(|e| e.to_string())?;
    line.push('\n');
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("messages.jsonl"))
        .and_then(|mut f| f.write_all(line.as_bytes()))
        .map_err(|e| format!("messages.jsonl: {e}"))
}

pub fn tail_log(dir: &Path, n: usize) -> Vec<LogEntry> {
    let Ok(f) = std::fs::File::open(dir.join("messages.jsonl")) else {
        return Vec::new();
    };
    let all: Vec<LogEntry> = std::io::BufReader::new(f)
        .lines()
        .map_while(Result::ok)
        .filter_map(|l| serde_json::from_str(&l).ok())
        .collect();
    let skip = all.len().saturating_sub(n);
    all.into_iter().skip(skip).collect()
}

fn log(dir: &str, from: &str, to: &str, text: &str, error: Option<&str>) {
    let entry = LogEntry {
        ts: now_rfc3339(),
        dir: dir.into(),
        from: from.into(),
        to: to.into(),
        text: text.into(),
        ok: error.is_none(),
        error: error.map(str::to_string),
    };
    if let Err(e) = append_log(&data_dir(), &entry) {
        eprintln!("klark msg: {e}");
    }
}

/// Last sender per receiving pane (`<session>/<pane_id>` -> address), for reply.
fn senders_path(dir: &Path) -> PathBuf {
    dir.join("msg-senders.json")
}

pub fn record_sender(dir: &Path, session: &str, pane_id: &str, from: &str) -> Result<(), String> {
    let path = senders_path(dir);
    let mut map: serde_json::Map<String, Value> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    map.insert(format!("{session}/{pane_id}"), Value::String(from.into()));
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, Value::Object(map).to_string()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

pub fn last_sender(dir: &Path, session: &str, pane_id: &str) -> Option<String> {
    let map: Value =
        serde_json::from_str(&std::fs::read_to_string(senders_path(dir)).ok()?).ok()?;
    map.get(format!("{session}/{pane_id}"))?
        .as_str()
        .map(str::to_string)
}

// ---------------------------------------------------------------- commands

fn cmd_whoami(args: &[String]) -> R<()> {
    let a = Args::parse(args, &[], &["--json"])?;
    let me = whoami();
    if a.has("--json") {
        println!("{}", serde_json::to_string(&me).unwrap_or_default());
    } else {
        println!("{}", me.address);
    }
    Ok(())
}

fn cmd_ls(args: &[String]) -> R<()> {
    let a = Args::parse(args, &[], &["--json", "--local"])?;
    let machine = my_machine();
    let local = local_agents(&machine);
    if a.has("--local") {
        if a.has("--json") {
            println!("{}", serde_json::to_string(&local).unwrap_or_default());
        } else {
            let home = home_dir();
            print_table(
                local
                    .iter()
                    .map(|l| row(l, "local", home.as_deref()))
                    .collect(),
            );
        }
        return Ok(());
    }

    let mut rows: Vec<Value> = local
        .iter()
        .map(|l| {
            let mut v = serde_json::to_value(l).unwrap_or_default();
            v["machine"] = json!(machine);
            v["local"] = json!(true);
            v
        })
        .collect();
    if hub_base().is_some() {
        match fetch_hub_agents() {
            Ok(remote) => {
                for mut r in remote {
                    if r.get("machine").and_then(Value::as_str) == Some(machine.as_str()) {
                        continue; // ourselves, already listed from the sockets
                    }
                    r["local"] = json!(false);
                    rows.push(r);
                }
            }
            Err(e) => eprintln!("klark msg: hub: {e}"),
        }
    }
    if a.has("--json") {
        println!("{}", Value::Array(rows));
    } else {
        let home = home_dir();
        print_table(
            rows.iter()
                .map(|r| {
                    let local = r["local"] == json!(true);
                    match serde_json::from_value::<LocalAgent>(r.clone()) {
                        Ok(l) if local => row(&l, "local", home.as_deref()),
                        Ok(l) => row(&l, "remote", None),
                        Err(_) => {
                            let s = |k: &str| {
                                r.get(k).and_then(Value::as_str).unwrap_or("").to_string()
                            };
                            [
                                s("address"),
                                shown_status(&s("status")),
                                s("agent"),
                                if local { "local" } else { "remote" }.to_string(),
                                String::new(),
                                String::new(),
                                String::new(),
                            ]
                        }
                    }
                })
                .collect(),
        );
    }
    Ok(())
}

/// Table cell for a status: `-` when klark cannot classify the agent's
/// screen (`unknown`); `--json` keeps the raw value.
fn shown_status(status: &str) -> String {
    match status {
        "" | "unknown" => "-".to_string(),
        other => other.to_string(),
    }
}

/// Width `klark msg ls` keeps its table within on a terminal whose size is
/// unknown. When the table is too wide, TITLE gives way first, then FOLDER
/// (keeping its end), then WHERE (losing the branch before the worktree
/// name), each down to its minimum. Output that is not a terminal (an agent
/// reading it, a pipe) is never cut.
const TABLE_WIDTH: usize = 120;
const MIN_WHERE_WIDTH: usize = 14;
const MIN_FOLDER_WIDTH: usize = 12;
const MIN_TITLE_WIDTH: usize = 16;

fn home_dir() -> Option<String> {
    std::env::var("HOME").ok().filter(|home| home.len() > 1)
}

/// One table row: address, status, agent, where, folder, title. WHERE is
/// the checkout (`main` or `worktree <name>`, with the branch) for a local
/// agent in git, `local` for one outside git, and `remote` (plus the
/// checkout, when the other machine reports it) for an agent elsewhere.
fn row(l: &LocalAgent, place: &str, home: Option<&str>) -> [String; 7] {
    let mut checkout = match (l.checkout.as_deref(), &l.worktree) {
        (_, Some(wt)) => format!("worktree {wt}"),
        (Some(kind), None) => kind.to_string(),
        (None, None) => String::new(),
    };
    if let (false, Some(branch)) = (checkout.is_empty(), &l.branch) {
        checkout.push_str(&format!(" ({branch})"));
    }
    let place = match (place, checkout.is_empty()) {
        (place, true) => place.to_string(),
        ("local", false) => checkout,
        (place, false) => format!("{place} · {checkout}"),
    };
    [
        l.address.clone(),
        shown_status(&l.status),
        l.agent.clone().unwrap_or_default(),
        place,
        parent_cell(l),
        l.cwd
            .as_deref()
            .map(|cwd| short_folder(cwd, home))
            .unwrap_or_default(),
        l.title.clone().unwrap_or_default(),
    ]
}

/// PARENT: for an agent in a worktree, the live agent in the main checkout
/// (`session/agent`; the machine is the same), `several` when more than one
/// fits (`--json` lists them), `none` when no agent runs there.
fn parent_cell(l: &LocalAgent) -> String {
    if let Some(p) = &l.parent {
        return p
            .split_once('/')
            .map_or(p.as_str(), |(_, rest)| rest)
            .to_string();
    }
    if !l.parents.is_empty() {
        return format!("several ({})", l.parents.len());
    }
    if l.checkout.as_deref() == Some("worktree") && l.main_root.is_some() {
        return "none".into();
    }
    String::new()
}

/// `cwd` with the home folder as `~`.
fn short_folder(cwd: &str, home: Option<&str>) -> String {
    match home.and_then(|home| cwd.strip_prefix(home)) {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
        _ => cwd.to_string(),
    }
}

/// At most `width` characters, keeping the end: `…` plus the tail.
fn ellipsize_start(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len <= width {
        return text.to_string();
    }
    let tail: String = text.chars().skip(len - width.saturating_sub(1)).collect();
    format!("…{tail}")
}

/// A WHERE cell in `width`: without its `(branch)` when it does not fit, and
/// cut only if still too long.
fn fit_where(cell: &str, width: usize) -> String {
    if cell.chars().count() <= width {
        return cell.to_string();
    }
    match cell.rfind(" (").filter(|_| cell.ends_with(')')) {
        Some(at) => ellipsize(&cell[..at], width),
        None => ellipsize(cell, width),
    }
}

/// At most `width` characters, ending in `…` when cut.
fn ellipsize(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn print_table(rows: Vec<[String; 7]>) {
    if rows.is_empty() {
        println!("no agents found (are any klark sessions running agents?)");
        return;
    }
    let width = std::io::IsTerminal::is_terminal(&std::io::stdout()).then(|| {
        crossterm::terminal::size()
            .ok()
            .map(|(cols, _)| usize::from(cols))
            .filter(|cols| *cols > 0)
            .unwrap_or(TABLE_WIDTH)
    });
    print!("{}", render_table(&rows, width));
}

/// The table, fitted into `width` columns; `None` keeps every cell whole.
fn render_table(rows: &[[String; 7]], width: Option<usize>) -> String {
    let head = [
        "ADDRESS", "STATUS", "AGENT", "WHERE", "PARENT", "FOLDER", "TITLE",
    ]
    .map(String::from);
    let mut w = [0usize; 7];
    for r in std::iter::once(&head).chain(rows.iter()) {
        for (i, c) in r.iter().enumerate() {
            w[i] = w[i].max(c.chars().count());
        }
    }
    // PARENT only when some agent works in a worktree.
    let show_parent = rows.iter().any(|r| !r[4].is_empty());
    let shown = |i: usize| i != 4 || show_parent;
    let min = [
        0,
        0,
        0,
        MIN_WHERE_WIDTH,
        0,
        MIN_FOLDER_WIDTH,
        MIN_TITLE_WIDTH,
    ];
    let natural = w;
    let total = |w: &[usize; 7]| {
        let cols: Vec<usize> = (0..7).filter(|&i| shown(i)).map(|i| w[i]).collect();
        cols.iter().sum::<usize>() + 2 * (cols.len() - 1)
    };
    let table_width = width.unwrap_or(usize::MAX);
    for column in [6, 5, 3] {
        let excess = total(&w).saturating_sub(table_width);
        w[column] -= excess.min(w[column].saturating_sub(min[column]));
    }
    // Dropping branches can leave WHERE narrower than its budget: give the
    // room back to FOLDER, then TITLE.
    w[3] = std::iter::once(head[3].chars().count())
        .chain(rows.iter().map(|r| fit_where(&r[3], w[3]).chars().count()))
        .max()
        .unwrap_or(w[3]);
    for column in [5, 6] {
        let spare = table_width.saturating_sub(total(&w));
        w[column] = natural[column].min(w[column] + spare);
    }
    let mut out = String::new();
    for r in std::iter::once(&head).chain(rows.iter()) {
        let cells = [
            r[0].clone(),
            r[1].clone(),
            r[2].clone(),
            fit_where(&r[3], w[3]),
            r[4].clone(),
            ellipsize_start(&r[5], w[5]),
            ellipsize(&r[6], w[6]),
        ];
        let mut line = String::new();
        for (i, cell) in cells.iter().enumerate() {
            if !shown(i) {
                continue;
            }
            line.push_str(cell);
            if i < 6 {
                line.push_str(&" ".repeat(w[i] - cell.chars().count() + 2));
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn fetch_hub_agents() -> Result<Vec<Value>, String> {
    let (base, token) = hub_target()?;
    let resp = klark_link::http::request(
        "GET",
        &format!("{base}/api/agents"),
        Some(&token),
        None,
        HTTP_TIMEOUT,
    )?;
    if !resp.ok() {
        return Err(resp.error_text());
    }
    serde_json::from_str::<Vec<Value>>(&resp.body).map_err(|e| format!("GET /api/agents: {e}"))
}

fn text_of(words: &[String]) -> R<String> {
    let text = words.join(" ");
    if text.trim().is_empty() {
        return Err(Fail::Usage("no message text".into()));
    }
    Ok(text)
}

/// `parent`: the live agent in the main checkout of the repository this
/// pane's worktree belongs to (`klark msg ls` PARENT). Refuses rather than
/// guessing when there is none or several.
fn parent_address() -> R<String> {
    let pane = my_pane().ok_or_else(|| {
        Fail::Error("parent works inside a klark pane (KLARK_PANE_ID is not set)".into())
    })?;
    let session = my_session();
    let local = local_agents(&my_machine());
    let me = local
        .iter()
        .find(|l| l.session == session && l.pane_id == pane)
        .ok_or_else(|| {
            Fail::Error(format!(
                "parent: this pane ({session}/{pane}) is not an agent klark msg ls lists"
            ))
        })?;
    if let Some(p) = &me.parent {
        return Ok(p.clone());
    }
    let msg = if !me.parents.is_empty() {
        format!(
            "parent: several agents work in the main checkout; send to one of: {}",
            me.parents.join(", ")
        )
    } else if me.checkout.as_deref() != Some("worktree") {
        "parent: this agent is not in a git worktree, so it has no parent agent".into()
    } else if let Some(main) = &me.main_root {
        format!(
            "parent: no klark agent runs in the main checkout {} (see: klark msg ls)",
            main.display()
        )
    } else {
        "parent: could not find this worktree's main checkout".into()
    };
    Err(Fail::Error(msg))
}

fn cmd_send(args: &[String]) -> R<()> {
    let a = Args::parse(
        args,
        &[
            "--from",
            "--wait",
            "--wait-reply",
            "--task",
            "--owner",
            "--parent",
            "--correlation",
        ],
        &["--json", "--all", "--all-sessions"],
    )?;
    let wait = wait_secs(&a)?;
    if a.has("--task") && (a.has("--all") || a.has("--all-sessions")) {
        return Err(Fail::Usage("--task goes to one agent, not --all".into()));
    }
    let wait_reply = a
        .value("--wait-reply")
        .map(|v| {
            v.parse::<u64>()
                .map_err(|_| Fail::Usage(format!("--wait-reply {v}: seconds expected")))
        })
        .transpose()?;
    if a.has("--all") || a.has("--all-sessions") {
        return broadcast(&a, a.has("--all-sessions"));
    }
    let (to, words) = a
        .positional
        .split_first()
        .ok_or_else(|| Fail::Usage("send needs <to> <text...>".into()))?;
    let text = text_of(words)?;
    let parent;
    let to = if to == "parent" {
        parent = parent_address()?;
        &parent
    } else {
        to
    };
    let from = a.value("--from").unwrap_or_else(|| {
        let me = whoami();
        // Outside a pane every caller is <machine>/<session>/cli; one that
        // waits for its answer needs an address of its own.
        if wait_reply.is_some() && me.pane_id.is_none() {
            format!("{}/{}/cli-{}", me.machine, me.session, std::process::id())
        } else {
            me.address
        }
    });
    if let Some(secs) = wait {
        wait_until_idle(to, secs)?;
    }
    if let Some(id) = a.value("--task") {
        return send_task(&a, to, &from, &text, &id, wait_reply);
    }
    let seen = tail_log(&data_dir(), usize::MAX).len();
    let (to_addr, outcome) = send(to, &from, &text);
    let json = a.has("--json");
    if let (Ok(()), Some(secs)) = (&outcome, wait_reply) {
        let reply = wait_for_reply(&data_dir(), seen, &from, secs)?;
        if json {
            println!(
                "{}",
                json!({"delivered": true, "to": to_addr, "from": from, "reply": reply.text, "reply_from": reply.from})
            );
        } else {
            println!("{}", reply.text);
        }
        return Ok(());
    }
    match outcome {
        Ok(()) => {
            if json {
                println!(
                    "{}",
                    json!({"delivered": true, "to": to_addr, "from": from})
                );
            } else {
                println!("delivered to {to_addr}");
            }
            Ok(())
        }
        Err(e) => {
            if json {
                println!(
                    "{}",
                    json!({"delivered": false, "to": to_addr, "from": from, "error": e})
                );
            }
            Err(Fail::Error(e))
        }
    }
}

fn task_store() -> Result<crate::task_store::Store, String> {
    crate::task_store::Store::open(&data_dir()).map_err(String::from)
}

/// The agent's stable id (its CLI conversation id), when klark knows it.
fn agent_id_of(session: &str, pane: &str) -> Option<String> {
    let list = call(&session_socket(session), "agent.list", json!({})).ok()?;
    list.get("agents")?
        .as_array()?
        .iter()
        .find(|a| a["pane_id"] == json!(pane))?
        .pointer("/agent_session/value")?
        .as_str()
        .map(str::to_string)
}

/// A queued task for `to` (resolved locally): created, or the existing one
/// for the same id, owner, agent and prompt.
fn create_task(
    id: Option<String>,
    owner: Option<String>,
    from: &str,
    to: &str,
    prompt: &str,
    parent: Option<String>,
    correlation: Option<String>,
) -> Result<crate::task_store::Task, crate::task_store::TaskError> {
    use crate::task_store::TaskError;
    let store = crate::task_store::Store::open(&data_dir())?;
    let (shown, session, pane) = resolve_local(to).map_err(|e| TaskError {
        code: "agent_not_found",
        message: e,
    })?;
    store.create(crate::task_store::NewTask {
        id,
        owner,
        from: from.to_string(),
        to: shown,
        agent_id: agent_id_of(&session, &pane),
        to_session: session,
        to_pane: pane,
        parent_id: parent,
        correlation_id: correlation,
        prompt: prompt.to_string(),
    })
}

/// `send --task <id|new>`: create (or find) the task, deliver it framed with
/// its id unless it is already delivered, and record running / blocked /
/// failed. With `--wait-reply`, wait for its answer.
fn send_task(
    a: &Args,
    to: &str,
    from: &str,
    text: &str,
    id: &str,
    wait_reply: Option<u64>,
) -> R<()> {
    let json = a.has("--json");
    let id = (id != "new").then(|| id.to_string());
    let task = create_task(
        id,
        a.value("--owner"),
        from,
        to,
        text,
        a.value("--parent"),
        a.value("--correlation"),
    )
    .map_err(|e| Fail::Error(e.message))?;
    let store = task_store()?;
    let id = task.id.clone();
    // Idempotent: a task already delivered (or finished) is not typed again.
    let already = !matches!(task.status.as_str(), "queued" | "blocked");
    let (to_addr, outcome) = if already {
        (task.to.clone(), Ok(()))
    } else {
        let (to_addr, _, outcome) = send_routed(to, from, text, Frame::Task(&id));
        match &outcome {
            Ok(()) => store.set_status(&id, "running", None),
            Err(e) if e.contains("agent_blocked") || e.contains("blocked") => {
                store.set_status(&id, "blocked", Some(e))
            }
            Err(e) => store.set_status(&id, "failed", Some(e)),
        }
        .map_err(|e| Fail::Error(e.message))?;
        (to_addr, outcome)
    };
    if let Err(e) = outcome {
        let task = store.get(&id).ok().flatten();
        if json {
            println!(
                "{}",
                json!({"delivered": false, "task": id, "status": task.map(|t| t.status), "to": to_addr, "from": from, "error": e})
            );
        }
        return Err(Fail::Error(e));
    }
    if let Some(secs) = wait_reply {
        let task = store
            .wait(&id, None, Duration::from_secs(secs))
            .map_err(|e| Fail::Error(e.message))?;
        if json {
            println!("{}", serde_json::to_string(&task).unwrap_or_default());
        } else if task.status == "done" {
            println!("{}", task.result.unwrap_or_default());
        }
        if task.status != "done" {
            return Err(Fail::Error(format!(
                "task {id} is {}, not answered within {secs}s; it may still come: klark task wait {id}",
                task.status
            )));
        }
        return Ok(());
    }
    if json {
        println!(
            "{}",
            json!({"delivered": true, "already": already, "task": id, "to": to_addr, "from": from})
        );
    } else if already {
        println!(
            "task {id} was already delivered to {to_addr} ({})",
            task.status
        );
    } else {
        println!("task {id} delivered to {to_addr}");
    }
    Ok(())
}

/// `send --wait-reply`: the first message addressed to `me` written to the
/// shared log after entry `seen`. A reply to a sender that is not a pane
/// cannot be typed anywhere, but `reply` still logs it, so it is read here.
fn wait_for_reply(dir: &Path, seen: usize, me: &str, secs: u64) -> R<LogEntry> {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    loop {
        if let Some(e) = tail_log(dir, usize::MAX)
            .into_iter()
            .skip(seen)
            .find(|e| e.dir == "sent" && e.to == me)
        {
            return Ok(e);
        }
        if std::time::Instant::now() >= deadline {
            return Err(Fail::Error(format!(
                "delivered, but no reply within {secs}s; it may still come: klark msg log"
            )));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// `--wait <secs>` (default 600 when given without a number is not supported:
/// the value is required).
fn wait_secs(a: &Args) -> R<Option<u64>> {
    a.value("--wait")
        .map(|v| {
            v.parse::<u64>()
                .map_err(|_| Fail::Usage(format!("--wait {v}: seconds expected")))
        })
        .transpose()
}

/// Hold a local message until its target agent is idle (not working, not
/// blocked on a dialog), so it neither interrupts a turn nor answers a
/// question by accident. Remote targets are not waited on.
fn wait_until_idle(to: &str, secs: u64) -> R<()> {
    let machine = my_machine();
    let Ok(addr) = parse_address(to) else {
        return Ok(());
    };
    if addr.machine.as_deref().is_some_and(|m| m != machine) {
        return Ok(());
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    loop {
        let local = local_agents(&machine);
        let status = match resolve(&addr, &local, &machine) {
            Ok(Route::Local { session, agent }) => local
                .iter()
                .find(|l| l.session == session && l.pane_id == agent)
                .map(|l| l.status.clone()),
            _ => return Ok(()), // let send report the real error
        };
        match status.as_deref() {
            Some("idle") | Some("done") | None => return Ok(()),
            Some(s) if std::time::Instant::now() >= deadline => {
                return Err(Fail::Error(format!(
                    "{to} is still {s} after {secs}s; not sent"
                )))
            }
            _ => std::thread::sleep(Duration::from_millis(500)),
        }
    }
}

/// `send --all` / `send --all-sessions`: the same message to every other
/// agent in this session (or every running session), worktrees included.
fn broadcast(a: &Args, every_session: bool) -> R<()> {
    let text = text_of(&a.positional)?;
    let me = whoami();
    let from = a.value("--from").unwrap_or_else(|| me.address.clone());
    let targets: Vec<LocalAgent> = local_agents(&me.machine)
        .into_iter()
        .filter(|l| every_session || l.session == me.session)
        .filter(|l| !(l.session == me.session && me.pane_id.as_deref() == Some(l.pane_id.as_str())))
        .collect();
    if targets.is_empty() {
        return Err(Fail::Error(if every_session {
            "no other agents in any running session".into()
        } else {
            format!("no other agents in session {}", me.session)
        }));
    }
    let mut failed = 0;
    let wait = wait_secs(a)?;
    for t in &targets {
        if let Some(secs) = wait {
            if let Err(e) = wait_until_idle(&format!("{}/{}", t.session, t.pane_id), secs) {
                println!("skipped {}: {e:?}", t.address);
                failed += 1;
                continue;
            }
        }
        let (to, outcome) = send(&format!("{}/{}", t.session, t.pane_id), &from, &text);
        let to = if outcome.is_ok() {
            t.address.clone()
        } else {
            to
        };
        if a.has("--json") {
            println!(
                "{}",
                json!({"delivered": outcome.is_ok(), "to": to, "from": from, "error": outcome.as_ref().err()})
            );
        } else {
            match &outcome {
                Ok(()) => println!("delivered to {to}"),
                Err(e) => println!("failed {to}: {e}"),
            }
        }
        failed += usize::from(outcome.is_err());
    }
    if failed > 0 {
        return Err(Fail::Error(format!(
            "{failed} of {} not delivered",
            targets.len()
        )));
    }
    Ok(())
}

/// Send and log; returns the resolved destination address and the outcome.
fn send(to: &str, from: &str, text: &str) -> (String, Result<(), String>) {
    send_framed(to, from, text, Frame::Msg)
}

fn send_framed(to: &str, from: &str, text: &str, frame: Frame) -> (String, Result<(), String>) {
    let (shown, _, result) = send_routed(to, from, text, frame);
    (shown, result)
}

/// The local destination of an address: (shown address, session, pane id).
fn resolve_local(to: &str) -> Result<(String, String, String), String> {
    let machine = my_machine();
    let addr = parse_address(to)?;
    if addr.machine.as_deref().is_some_and(|m| m != machine) {
        return Err(format!(
            "{to}: tasks go to agents on this machine only (for now)"
        ));
    }
    let local = local_agents(&machine);
    match resolve(&addr, &local, &machine)? {
        Route::Local { session, agent } => {
            let shown = local
                .iter()
                .find(|l| l.session == session && l.pane_id == agent)
                .map(|l| l.address.clone())
                .unwrap_or_else(|| format!("{machine}/{session}/{agent}"));
            Ok((shown, session, agent))
        }
        Route::Remote { to } => Err(format!(
            "{to}: tasks go to agents on this machine only (for now)"
        )),
    }
}

fn send_routed(
    to: &str,
    from: &str,
    text: &str,
    frame: Frame,
) -> (String, Option<(String, String)>, Result<(), String>) {
    let mut route = None;
    let machine = my_machine();
    let addr = match parse_address(to) {
        Ok(a) => a,
        Err(e) => return (to.to_string(), None, Err(e)),
    };
    let local = if addr.machine.as_deref().is_some_and(|m| m != machine) {
        Vec::new()
    } else {
        local_agents(&machine)
    };
    let (to_addr, result) = match resolve(&addr, &local, &machine) {
        Err(e) => (to.to_string(), Err(e)),
        Ok(Route::Local { session, agent }) => {
            let shown = local
                .iter()
                .find(|l| l.session == session && l.pane_id == agent)
                .map(|l| l.address.clone())
                .unwrap_or_else(|| format!("{machine}/{session}/{agent}"));
            let r = deliver_framed(&session, &agent, from, text, frame).map(|_| ());
            route = Some((session, agent));
            (shown, r)
        }
        Ok(Route::Remote { to }) => {
            // The remote machine frames what the hub hands it as a `klark
            // msg`; a hook notice keeps its own prefix inside that text.
            let r = match frame {
                Frame::Msg => send_via_hub(&to, from, text),
                Frame::Hook(name) => send_via_hub(&to, from, &format_hook_notice(name, text)),
                Frame::Task(_) => Err(format!(
                    "{to}: tasks go to agents on this machine only (for now)"
                )),
            };
            (to, r)
        }
    };
    log(
        "sent",
        from,
        &to_addr,
        text,
        result.as_ref().err().map(String::as_str),
    );
    (to_addr, route, result)
}

/// A hook notice to a `notify` address: resolved, delivered and logged like
/// `klark msg send`, but framed `[klark hook <hook>]` with no reply hint, from
/// [`hook_sender`]. Returns the resolved address.
pub(crate) fn send_notice(to: &str, hook: &str, text: &str) -> Result<String, String> {
    let (addr, result) = send_framed(to, &hook_sender(hook), text, Frame::Hook(hook));
    result.map(|()| addr)
}

fn send_via_hub(to: &str, from: &str, text: &str) -> Result<(), String> {
    let (base, token) = hub_target()?;
    let body = json!({"to": to, "text": text, "from": from});
    let resp = klark_link::http::request(
        "POST",
        &format!("{base}/api/messages"),
        Some(&token),
        Some(&body),
        HTTP_TIMEOUT,
    )?;
    if resp.ok() {
        Ok(())
    } else {
        Err(hub_error(to, &resp.error_text()))
    }
}

/// A hub refusal as one readable line. The hub relays the remote `klark msg
/// deliver` stderr, which carries its own `klark msg:` and `agent.prompt:`
/// prefixes; drop those and name the agent: `m/s/a: no such agent (agent_not_found)`.
pub fn hub_error(to: &str, raw: &str) -> String {
    let mut rest = raw.trim();
    loop {
        let before = rest;
        for prefix in ["klark msg:", "hub refused:", "agent.prompt:"] {
            if let Some(stripped) = rest.strip_prefix(prefix) {
                rest = stripped.trim_start();
            }
        }
        if rest == before {
            break;
        }
    }
    let (code, detail) = match rest.split_once(' ') {
        Some((code, detail)) => (code, detail.trim()),
        None => (rest, ""),
    };
    let is_code = !code.is_empty()
        && code.contains('_')
        && code.chars().all(|c| c.is_ascii_lowercase() || c == '_');
    if !is_code {
        return format!("{to}: hub refused: {rest}");
    }
    let plain = match code {
        "agent_not_found" => "no such agent",
        "agent_blocked" => "the agent is blocked, waiting on a person",
        "agent_target_ambiguous" => "ambiguous agent name; use its pane id",
        _ if detail.is_empty() => code,
        _ => detail,
    };
    format!("{to}: {plain} ({code})")
}

/// Prompt a local agent; returns its pane id. Records the sender for reply and
/// logs the receipt.
fn deliver(session: &str, agent: &str, from: &str, text: &str) -> Result<String, String> {
    deliver_framed(session, agent, from, text, Frame::Msg)
}

fn deliver_framed(
    session: &str,
    agent: &str,
    from: &str,
    text: &str,
    frame: Frame,
) -> Result<String, String> {
    // Someone mid-edit in that agent's input: wait (up to a minute) rather
    // than mix this message into their half-written text.
    let result = prompt_agent(
        session,
        agent,
        &frame.prompt(from, text),
        Duration::from_secs(60),
    );
    let pane = result
        .as_ref()
        .map(|(_, pane)| pane.clone())
        .unwrap_or_else(|_| agent.to_string());
    let result = result.map(|(r, _)| r);
    let to = received_address(&my_machine(), session, agent, result.as_ref().ok());
    if result.is_ok() && matches!(frame, Frame::Msg | Frame::Task(_)) {
        if let Err(e) = record_sender(&data_dir(), session, &pane, from) {
            eprintln!("klark msg: could not record sender: {e}");
        }
    }
    let err = result.as_ref().err().cloned();
    log("received", from, &to, text, err.as_deref());
    result.map(|_| pane)
}

/// Prompt a local agent with `prompt` exactly as given, without losing the
/// person's draft: `agent.prompt` lifts an unsent draft out, and it is typed
/// back afterwards. While someone is mid-edit the server refuses with
/// `agent_user_typing`; that is retried for up to `typing_wait`, then
/// returned. Returns the `agent.prompt` result and the agent's pane id.
/// Used by `klark msg` (which adds its own frame and logging) and `klark feed`.
pub(crate) fn prompt_agent(
    session: &str,
    agent: &str,
    prompt: &str,
    typing_wait: Duration,
) -> Result<(Value, String), String> {
    let socket = session_socket(session);
    let typing_deadline = std::time::Instant::now() + typing_wait;
    let result = loop {
        let r = call_detailed(
            &socket,
            "agent.prompt",
            json!({"target": agent, "text": prompt}),
        )
        .map_err(prompt_error_text);
        match &r {
            Err(e)
                if e.contains("agent_user_typing")
                    && std::time::Instant::now() < typing_deadline =>
            {
                std::thread::sleep(Duration::from_secs(1));
            }
            _ => break r,
        }
    }
    .map_err(|e| {
        if e.starts_with("connect ") {
            format!("session {session}: {e} ({SOCKET_TIMEOUT_NOTE})")
        } else {
            e
        }
    });
    let pane = result.as_ref().ok().and_then(|r| {
        r.get("agent")
            .and_then(|a| a.get("pane_id"))
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    let pane = pane.unwrap_or_else(|| agent.to_string());
    if let Ok(r) = &result {
        confirm_submitted(&socket, &pane);
        // The person's unsent draft was lifted out for this message: put it back, unsent.
        if let Some(draft) = r
            .get("draft")
            .and_then(Value::as_str)
            .filter(|d| !d.is_empty())
        {
            let _ = call(
                &socket,
                "pane.send_text",
                json!({"pane_id": pane, "text": draft}),
            );
        }
    }
    result.map(|r| (r, pane))
}

/// The `agent.prompt` error as text. When the agent became blocked after
/// klark had lifted the person's draft out of its input, the draft cannot be
/// typed back (it would answer the agent's dialog), so it is returned in the
/// error instead of being dropped.
fn prompt_error_text(err: klark_link::upstream::CallError) -> String {
    let draft = err
        .error
        .as_ref()
        .and_then(|e| e.get("unsent_draft"))
        .and_then(Value::as_str)
        .filter(|d| !d.is_empty());
    match draft {
        Some(draft) => format!(
            "{}; the person's unsent draft was lifted out of the input and not retyped: {draft:?}",
            err.message
        ),
        None => err.message,
    }
}

/// A prompt counts as sent once the agent starts working. Some agents swallow
/// the Enter that follows a long paste and leave the text sitting in the input
/// (herdr #4990), so if the agent is still idle after a few seconds, press
/// Enter once more. Never when it is blocked: Enter would answer a dialog.
fn confirm_submitted(socket: &Path, pane: &str) {
    let status = || {
        call(socket, "agent.list", json!({})).ok().and_then(|r| {
            r.get("agents")?.as_array()?.iter().find_map(|a| {
                (a.get("pane_id")?.as_str()? == pane)
                    .then(|| a.get("agent_status")?.as_str().map(str::to_string))
                    .flatten()
            })
        })
    };
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        match status().as_deref() {
            Some("idle") | Some("done") => std::thread::sleep(Duration::from_millis(250)),
            _ => return, // working, blocked or gone: the prompt was taken (or is not ours to push)
        }
    }
    if matches!(status().as_deref(), Some("idle") | Some("done")) {
        let _ = call(
            socket,
            "pane.send_keys",
            json!({"pane_id": pane, "keys": ["Enter"]}),
        );
    }
}

/// The receiver's full address for the log: `machine/session/name`, or the
/// pane id when the agent has no name, or the target as given on failure.
pub fn received_address(
    machine: &str,
    session: &str,
    target: &str,
    prompt_result: Option<&Value>,
) -> String {
    let agent = prompt_result.and_then(|r| r.get("agent"));
    let field = |k: &str| agent.and_then(|a| a.get(k)).and_then(Value::as_str);
    let handle = field("name").or_else(|| field("pane_id")).unwrap_or(target);
    format!("{machine}/{session}/{handle}")
}

fn cmd_deliver(args: &[String]) -> R<()> {
    let a = Args::parse(args, &["--session", "--from"], &[])?;
    let session = a.value("--session").unwrap_or_else(my_session);
    let from = a
        .value("--from")
        .ok_or_else(|| Fail::Usage("deliver needs --from <addr>".into()))?;
    let (agent, words) = a
        .positional
        .split_first()
        .ok_or_else(|| Fail::Usage("deliver needs <agent> <text...>".into()))?;
    let text = text_of(words)?;
    // The agent may be a name or a pane id; resolve names within the session
    // so an ambiguous name fails instead of picking one.
    let machine = my_machine();
    let local = local_agents(&machine);
    let target = match resolve(
        &Address {
            machine: None,
            session: Some(session.clone()),
            agent: agent.clone(),
        },
        &local,
        &machine,
    ) {
        Ok(Route::Local { agent, .. }) => agent,
        _ => agent.clone(), // let agent.prompt report the precise error
    };
    deliver(&session, &target, &from, &text)?;
    println!("delivered");
    Ok(())
}

fn cmd_reply(args: &[String]) -> R<()> {
    let a = Args::parse(args, &["--to", "--task"], &["--json"])?;
    let text = text_of(&a.positional)?;
    if let Some(id) = a.value("--task") {
        return reply_task(&id, &text);
    }
    let to = match a.value("--to") {
        Some(to) => to,
        None => {
            let pane =
                my_pane().ok_or("reply works inside a klark pane (KLARK_PANE_ID is not set)")?;
            last_sender(&data_dir(), &my_session(), &pane)
                .ok_or("nobody has messaged this pane yet; use klark msg send <to> <text>")?
        }
    };
    let from = whoami().address;
    let (to_addr, outcome) = send(&to, &from, &text);
    match outcome {
        Ok(()) => println!("delivered to {to_addr}"),
        // The sender is not a pane (a shell, a script, a coding agent run
        // outside klark): there is nothing to type into, but the reply is in
        // the shared log, where `send --wait-reply` and `klark msg log` read it.
        Err(e) if e.starts_with("no agent ") => println!(
            "{to} is not a klark pane, so the reply was saved for it (klark msg log) instead of typed"
        ),
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

/// `reply --task <id>`: store the result (only the task's addressee may), then
/// tell the sender as an ordinary reply.
fn reply_task(id: &str, text: &str) -> R<()> {
    let pane =
        my_pane().ok_or("reply --task works inside a klark pane (KLARK_PANE_ID is not set)")?;
    let me = whoami();
    let store = task_store()?;
    let truncated = store
        .answer(id, &me.session, &pane, &me.address, text)
        .map_err(|e| Fail::Error(e.message))?;
    let task = store
        .get(id)
        .ok()
        .flatten()
        .ok_or_else(|| format!("no task {id}"))?;
    println!("task {id} answered");
    if truncated {
        println!(
            "note: the result was cut to {} KB; put the rest in a file and give its path",
            crate::task_store::RESULT_CAP / 1024
        );
    }
    let note = format!("[task {id} answered] {text}");
    let (to_addr, outcome) = send(&task.from, &me.address, &note);
    // A sender outside klark (a script, dmcp) reads the task instead.
    if outcome.is_ok() {
        println!("told {to_addr}");
    }
    Ok(())
}

const TASK_USAGE: &str = "usage: klark task <command> ...

Durable tasks (klark#25). klark's tasks.db (SQLite, next to messages.jsonl) is
the only task store; callers keep no task rows. Lifecycle:
queued -> running (delivered) | blocked (agent waits on a person; retry) | failed
-> done (the agent's `klark msg reply --task <id> \"<result>\"`).

commands:
  create --to <agent> [--owner U] [--from A] [--id X] [--parent P] [--correlation C] <prompt...>
                                          a queued task (nothing is sent); prints the task.
                                          The same --id, owner, agent and prompt return the
                                          existing task; otherwise task_conflict.
  (deliver: klark msg send --task <id|new> [--owner U] [--parent P] [--correlation C] <to> <prompt...>)
  update <id> --status queued|running|blocked|failed [--note <text>] [--error <text>]
  update <id> --status done --from <creator> --source <screen|...> (--result <text> | --result-file <f>)
                                          the creator finishes a task the agent never answered
  progress <id> <text...>                 a progress note (only the addressee's pane)
  get <id> [--owner U] [--json]           one task
  list [--owner U] [--from A] [--status s1,s2] [--parent P] [--correlation C] [-n N] [--json]
  wait <id> [--owner U] [--timeout secs] [--json]   until done or failed (default 300 s)
  events [--since <unix secs>] [--owner U]          state changes (no prompts or results)
  prune [--older-than <days>]             drop finished tasks (also automatic after 30 days)

--owner: another owner's task reads as task_not_found.
exit codes: 0 ok / done; 1 not found, failed, refused or error; 124 wait timed out.
--json errors print {\"error\": \"...\", \"code\": \"task_not_found|task_conflict|not_addressee|not_creator|task_finished|bad_request|store_error\"}.";

/// `klark task ...`.
pub fn task_run(args: &[String]) -> ! {
    let (cmd, rest) = match args.split_first() {
        Some((c, r)) => (c.as_str(), r),
        None => {
            println!("{TASK_USAGE}");
            std::process::exit(0)
        }
    };
    if matches!(cmd, "-h" | "--help" | "help") {
        println!("{TASK_USAGE}");
        std::process::exit(0)
    }
    let json = rest.iter().any(|a| a == "--json");
    let fail = |code: &str, msg: String, exit: i32| -> ! {
        if json {
            println!("{}", json!({"error": msg, "code": code}));
        } else {
            eprintln!("klark task: {msg}");
        }
        std::process::exit(exit)
    };
    let a = match Args::parse_anywhere(
        rest,
        &[
            "--to",
            "--owner",
            "--from",
            "--id",
            "--parent",
            "--correlation",
            "--status",
            "--note",
            "--error",
            "--source",
            "--result",
            "--result-file",
            "-n",
            "--timeout",
            "--since",
            "--older-than",
        ],
        &["--json"],
    ) {
        Ok(a) => a,
        Err(Fail::Usage(m)) | Err(Fail::Error(m)) => fail("bad_request", m, 2),
    };
    let store = match crate::task_store::Store::open(&data_dir()) {
        Ok(s) => s,
        Err(e) => fail(e.code, e.message, 1),
    };
    let owner = a.value("--owner");
    let print = |task: &crate::task_store::Task| {
        if json {
            println!("{}", serde_json::to_string(task).unwrap_or_default());
        } else {
            println!("task      {}", task.id);
            println!("status    {}", task.status);
            println!("owner     {}", task.owner);
            println!("from      {}", task.from);
            println!("to        {}", task.to);
            if let Some(p) = &task.progress {
                println!("progress  {p}");
            }
            if let Some(by) = &task.answered_by {
                println!("answered  by {by}");
            }
            if let Some(src) = &task.source {
                println!("source    {src}");
            }
            if let Some(e) = &task.error {
                println!("error     {e}");
            }
            println!("prompt    {}", task.prompt);
            if let Some(r) = &task.result {
                println!(
                    "result{}\n{r}",
                    if task.truncated { " (truncated)" } else { "" }
                );
            }
        }
    };
    let id_arg = || -> String {
        a.positional
            .first()
            .cloned()
            .unwrap_or_else(|| fail("bad_request", format!("{cmd} needs <id>"), 2))
    };
    let or_fail = |r: Result<crate::task_store::Task, crate::task_store::TaskError>| match r {
        Ok(t) => t,
        Err(e) => fail(e.code, e.message, 1),
    };
    match cmd {
        "create" => {
            let to = a
                .value("--to")
                .unwrap_or_else(|| fail("bad_request", "create needs --to <agent>".into(), 2));
            let prompt = a.positional.join(" ");
            if prompt.trim().is_empty() {
                fail("bad_request", "create needs a prompt".into(), 2)
            }
            let from = a.value("--from").unwrap_or_else(|| whoami().address);
            let t = or_fail(create_task(
                a.value("--id"),
                owner,
                &from,
                &to,
                &prompt,
                a.value("--parent"),
                a.value("--correlation"),
            ));
            print(&t);
        }
        "update" => {
            let id = id_arg();
            let status = a
                .value("--status")
                .unwrap_or_else(|| fail("bad_request", "update needs --status".into(), 2));
            if status == "done" {
                let from = a.value("--from").unwrap_or_else(|| {
                    fail(
                        "bad_request",
                        "--status done needs --from <creator>".into(),
                        2,
                    )
                });
                let result = match (a.value("--result"), a.value("--result-file")) {
                    (Some(r), _) => r,
                    (None, Some(f)) => std::fs::read_to_string(&f)
                        .unwrap_or_else(|e| fail("bad_request", format!("{f}: {e}"), 2)),
                    (None, None) => fail(
                        "bad_request",
                        "--status done needs --result or --result-file".into(),
                        2,
                    ),
                };
                let source = a.value("--source").unwrap_or_else(|| "creator".into());
                if let Err(e) = store.complete(&id, &from, &result, &source) {
                    fail(e.code, e.message, 1)
                }
                print(&or_fail(store.get_for(&id, None)));
            } else {
                let note = a.value("--error").or_else(|| a.value("--note"));
                print(&or_fail(store.set_status(&id, &status, note.as_deref())));
            }
        }
        "progress" => {
            let id = id_arg();
            let text = a.positional[1..].join(" ");
            let pane = my_pane().unwrap_or_else(|| {
                fail(
                    "bad_request",
                    "progress works inside a klark pane (KLARK_PANE_ID is not set)".into(),
                    2,
                )
            });
            print(&or_fail(store.progress(&id, &my_session(), &pane, &text)));
        }
        "get" | "show" => print(&or_fail(store.get_for(&id_arg(), owner.as_deref()))),
        "list" | "ls" => {
            let filter = crate::task_store::Filter {
                owner,
                from: a.value("--from"),
                statuses: a
                    .value("--status")
                    .map(|s| s.split(',').map(|x| x.trim().to_string()).collect())
                    .unwrap_or_default(),
                parent_id: a.value("--parent"),
                correlation_id: a.value("--correlation"),
                limit: a.value("-n").and_then(|v| v.parse().ok()).unwrap_or(20),
            };
            let list = match store.list(&filter) {
                Ok(l) => l,
                Err(e) => fail(e.code, e.message, 1),
            };
            if json {
                println!("{}", serde_json::to_string(&list).unwrap_or_default());
            } else if list.is_empty() {
                println!("no tasks (klark msg send --task new <to> <text>)");
            } else {
                for t in list {
                    let first: String = t.prompt.chars().take(50).collect();
                    println!("{:<38} {:<8} {:<36} {first}", t.id, t.status, t.to);
                }
            }
        }
        "wait" => {
            let secs = a
                .value("--timeout")
                .and_then(|v| v.parse().ok())
                .unwrap_or(300);
            let task = or_fail(store.wait(&id_arg(), owner.as_deref(), Duration::from_secs(secs)));
            print(&task);
            match task.status.as_str() {
                "done" => {}
                "failed" => std::process::exit(1),
                _ => std::process::exit(124),
            }
        }
        "events" => {
            let since = a.value("--since").and_then(|v| v.parse().ok()).unwrap_or(0);
            for e in store.events(since, owner.as_deref()) {
                println!("{e}");
            }
        }
        "prune" => {
            let days: u64 = a
                .value("--older-than")
                .and_then(|v| v.trim_end_matches('d').parse().ok())
                .unwrap_or(30);
            match store.prune(Duration::from_secs(days * 24 * 3600)) {
                Ok(n) => {
                    if json {
                        println!("{}", json!({"pruned": n}));
                    } else {
                        println!("pruned {n} finished tasks older than {days} days");
                    }
                }
                Err(e) => fail(e.code, e.message, 1),
            }
        }
        other => fail(
            "bad_request",
            format!("unknown command {other}\n{TASK_USAGE}"),
            2,
        ),
    }
    std::process::exit(0)
}

fn cmd_log(args: &[String]) -> R<()> {
    let a = Args::parse(args, &["-n"], &["--json"])?;
    let n = match a.value("-n") {
        Some(v) => v
            .parse()
            .map_err(|_| Fail::Usage(format!("-n {v}: not a number")))?,
        None => 20,
    };
    let entries = tail_log(&data_dir(), n);
    for e in entries {
        if a.has("--json") {
            println!("{}", serde_json::to_string(&e).unwrap_or_default());
        } else {
            let status = match &e.error {
                None => String::new(),
                Some(err) => format!("  [FAILED: {err}]"),
            };
            println!(
                "{} {:<8} {} -> {}: {}{status}",
                e.ts, e.dir, e.from, e.to, e.text
            );
        }
    }
    Ok(())
}

fn cmd_name(args: &[String]) -> R<()> {
    let a = Args::parse(args, &[], &[])?;
    let [name] = a.positional.as_slice() else {
        return Err(Fail::Usage("name needs exactly one <new-name>".into()));
    };
    if name.contains('/') || name.trim().is_empty() {
        return Err(Fail::Error(format!(
            "{name:?}: an agent name cannot be empty or contain '/'"
        )));
    }
    let pane = my_pane().ok_or("name works inside a klark pane (KLARK_PANE_ID is not set)")?;
    call(
        &crate::session::active_api_socket_path(),
        "agent.rename",
        json!({"target": pane, "name": name}),
    )?;
    println!("{}", whoami().address);
    Ok(())
}

fn read_password(from_stdin: bool) -> R<String> {
    let stdin = std::io::stdin();
    if !from_stdin && !is_tty(0) {
        return Err(Fail::Usage(
            "stdin is not a terminal: pass --password-stdin and pipe the password".into(),
        ));
    }
    let mut line = String::new();
    if from_stdin {
        stdin
            .lock()
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
    } else {
        eprint!("password: ");
        let _ = std::io::stderr().flush();
        let _echo = EchoOff::new();
        stdin
            .lock()
            .read_line(&mut line)
            .map_err(|e| e.to_string())?;
        eprintln!();
    }
    let pw = line.trim_end_matches(['\r', '\n']).to_string();
    if pw.is_empty() {
        return Err(Fail::Error("empty password".into()));
    }
    Ok(pw)
}

#[cfg(unix)]
fn is_tty(fd: i32) -> bool {
    unsafe { libc::isatty(fd) == 1 }
}
#[cfg(not(unix))]
fn is_tty(_fd: i32) -> bool {
    false
}

/// Turns terminal echo off for its lifetime.
struct EchoOff {
    #[cfg(unix)]
    saved: Option<libc::termios>,
}

impl EchoOff {
    fn new() -> Self {
        #[cfg(unix)]
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(0, &mut t) == 0 {
                let saved = t;
                t.c_lflag &= !libc::ECHO;
                t.c_lflag |= libc::ECHONL;
                libc::tcsetattr(0, libc::TCSANOW, &t);
                return Self { saved: Some(saved) };
            }
            Self { saved: None }
        }
        #[cfg(not(unix))]
        Self {}
    }
}

impl Drop for EchoOff {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(t) = self.saved {
            unsafe {
                libc::tcsetattr(0, libc::TCSANOW, &t);
            }
        }
    }
}

fn cmd_login(args: &[String]) -> R<()> {
    let a = Args::parse(args, &["--hub", "--user", "--name"], &["--password-stdin"])?;
    let hub = a
        .value("--hub")
        .or_else(|| std::env::var("KLARK_HUB_URL").ok())
        .or_else(|| HubConfig::load().map(|h| h.hub))
        .filter(|h| !h.trim().is_empty())
        .ok_or_else(|| Fail::Usage("login needs --hub <url>".into()))?;
    let user = a
        .value("--user")
        .ok_or_else(|| Fail::Usage("login needs --user <name>".into()))?;
    let base = klark_link::http::hub_http_base(&hub)?;
    let password = read_password(a.has("--password-stdin"))?;
    let resp = klark_link::http::request(
        "POST",
        &format!("{base}/api/token"),
        None,
        Some(&json!({"username": user, "password": password})),
        HTTP_TIMEOUT,
    )?;
    if !resp.ok() {
        return Err(Fail::Error(format!("login failed: {}", resp.error_text())));
    }
    let v: Value = serde_json::from_str(&resp.body).map_err(|e| format!("POST /api/token: {e}"))?;
    let jwt = v
        .get("token")
        .and_then(Value::as_str)
        .ok_or("POST /api/token: no token in the answer")?
        .to_string();
    let previous = HubConfig::load().unwrap_or_default();
    let cfg = HubConfig {
        hub,
        name: a.value("--name").or(previous.name),
        user: v
            .get("user")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or(Some(user)),
        jwt: Some(jwt),
        expires_at: v.get("expires_at").map(|e| match e {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }),
    };
    let path = hub_json_path();
    cfg.save_to(&path)?;
    println!(
        "logged in to {base} as {}; token saved to {} (expires {})",
        cfg.user.as_deref().unwrap_or("?"),
        path.display(),
        cfg.expires_at.as_deref().unwrap_or("unknown")
    );
    Ok(())
}

fn cmd_logout() -> R<()> {
    let path = hub_json_path();
    match HubConfig::load() {
        Some(mut cfg) if cfg.jwt.is_some() => {
            cfg.jwt = None;
            cfg.expires_at = None;
            cfg.save_to(&path)?;
            println!("token removed from {} (hub url kept)", path.display());
        }
        _ => println!("not logged in"),
    }
    Ok(())
}

fn cmd_status() -> R<()> {
    let me = whoami();
    println!("me:       {}", me.address);
    match hub_base() {
        None => println!("hub:      not configured (klark msg login --hub <url> --user <name>)"),
        Some(Ok(b)) => println!("hub:      {b}"),
        Some(Err(e)) => println!("hub:      invalid: {e}"),
    }
    let saved = HubConfig::load();
    let token_src = if std::env::var("KLARK_HUB_JWT").is_ok_and(|s| !s.is_empty()) {
        "KLARK_HUB_JWT"
    } else if saved.as_ref().and_then(|h| h.jwt.as_ref()).is_some() {
        "hub.json (klark msg login)"
    } else if std::env::var("KLARK_HUB_TOKEN").is_ok_and(|s| !s.is_empty()) {
        "KLARK_HUB_TOKEN (machine token)"
    } else {
        "none"
    };
    println!("token:    {token_src}");
    if let Some(jwt) = hub_token() {
        if let Some(exp) = jwt_exp(&jwt) {
            let left = exp - now_secs();
            if left <= 0 {
                println!("expiry:   EXPIRED; run klark msg login again");
            } else {
                println!("expiry:   in {}h{:02}m", left / 3600, (left % 3600) / 60);
            }
        } else if let Some(at) = saved.as_ref().and_then(|h| h.expires_at.as_ref()) {
            println!("expiry:   {at}");
        }
    }
    if let Some(user) = saved.and_then(|h| h.user) {
        println!("user:     {user}");
    }
    let sessions = running_sessions();
    println!(
        "sessions: {}",
        if sessions.is_empty() {
            "none running".to_string()
        } else {
            sessions.join(", ")
        }
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hook notice has its own prefix, no reply hint, and a sender that is
    /// not an address: it can never be routed to a session called `hooks`.
    #[test]
    fn hook_notices_are_not_messages_from_an_address() {
        let p = Frame::Hook("dmcp").prompt(&hook_sender("dmcp"), "klark hook dmcp: exited");
        assert_eq!(p, "[klark hook dmcp] klark hook dmcp: exited");
        assert!(!p.contains("reply"), "{p}");
        assert!(Frame::Msg.prompt("m/s/a", "hi").contains("klark msg reply"));

        let from = hook_sender("dmcp");
        assert!(!from.contains('/'), "{from}");
        assert!(crate::session::validate_name(&from).is_err());
        assert!(crate::session::validate_name("hooks").is_ok());
        // Even a reply typed by hand cannot reach a session or agent through it.
        let addr = parse_address(&from).unwrap();
        assert!(addr.machine.is_none() && addr.session.is_none());
        let hooks_session = [agent("hooks", Some("dmcp"), "p1")];
        assert!(resolve(&addr, &hooks_session, "m").is_err());
        // The old sender would have been routed there.
        let old = parse_address("m/hooks/dmcp").unwrap();
        assert!(matches!(
            resolve(&old, &hooks_session, "m"),
            Ok(Route::Local { .. })
        ));
    }

    fn agent(session: &str, name: Option<&str>, pane: &str) -> LocalAgent {
        let mut a = LocalAgent {
            session: session.into(),
            name: name.map(str::to_string),
            agent: Some("claude".into()),
            pane_id: pane.into(),
            status: "idle".into(),
            address: String::new(),
            worktree: None,
            cwd: None,
            checkout: None,
            branch: None,
            title: None,
            parent: None,
            parents: Vec::new(),
            root: None,
            main_root: None,
        };
        a.address = format!("m1/{session}/{}", a.handle());
        a
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "klark-msg-test-{tag}-{}-{}",
            std::process::id(),
            now_secs()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn addresses_parse() {
        assert_eq!(
            parse_address("rev").unwrap(),
            Address {
                machine: None,
                session: None,
                agent: "rev".into()
            }
        );
        assert_eq!(
            parse_address("work/rev").unwrap(),
            Address {
                machine: None,
                session: Some("work".into()),
                agent: "rev".into()
            }
        );
        assert_eq!(
            parse_address("box/work/rev").unwrap(),
            Address {
                machine: Some("box".into()),
                session: Some("work".into()),
                agent: "rev".into()
            }
        );
        for bad in ["", "a/", "/a", "a//b", "a/b/c/d"] {
            assert!(parse_address(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn resolution_follows_adr_0009() {
        let agents = vec![
            agent("default", Some("rev"), "p1"),
            agent("work", Some("rev"), "p2"),
            agent("work", Some("coder"), "p3"),
            agent("work", None, "p4"),
        ];
        let local = |s: &str, a: &str| {
            Ok(Route::Local {
                session: s.into(),
                agent: a.into(),
            })
        };
        let r = |s: &str| resolve(&parse_address(s).unwrap(), &agents, "m1");

        assert_eq!(r("coder"), local("work", "p3"));
        assert_eq!(r("p4"), local("work", "p4"));
        assert_eq!(r("work/rev"), local("work", "p2"));
        assert_eq!(r("default/rev"), local("default", "p1"));
        // A full address naming this machine stays local.
        assert_eq!(r("m1/work/coder"), local("work", "p3"));
        // Another machine goes to the hub.
        assert_eq!(
            r("m2/work/coder"),
            Ok(Route::Remote {
                to: "m2/work/coder".into()
            })
        );

        let amb = r("rev").unwrap_err();
        assert!(
            amb.contains("ambiguous") && amb.contains("default/p1") && amb.contains("work/p2"),
            "{amb}"
        );
        assert!(r("ghost").unwrap_err().contains("no agent ghost"));
        assert!(r("other/coder").unwrap_err().contains("in session other"));
    }

    #[test]
    fn prompt_matches_the_adr() {
        assert_eq!(
            format_prompt("alice-laptop/default/reviewer", "please review PR 12"),
            "[klark msg from alice-laptop/default/reviewer] please review PR 12\n(reply: klark msg reply --to alice-laptop/default/reviewer \"<text>\")"
        );
    }

    #[test]
    fn agent_list_maps_to_local_agents() {
        let result = json!({"agents": [
            {"terminal_id": "t1", "name": "rev", "agent": "claude", "agent_status": "working", "pane_id": "p1"},
            {"terminal_id": "t2", "display_agent": "codex", "agent_status": "idle", "pane_id": "p2"},
            {"terminal_id": "t3"}
        ]});
        let got = agents_from_list("work", "m1", &result);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].address, "m1/work/rev");
        assert_eq!(got[0].status, "working");
        assert_eq!(got[1].address, "m1/work/p2");
        assert_eq!(got[1].agent.as_deref(), Some("codex"));
        let v = serde_json::to_value(&got[0]).unwrap();
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            ["address", "agent", "name", "pane_id", "session", "status"]
        );
    }

    #[test]
    fn agent_list_gives_folder_checkout_branch_and_title() {
        let base = scratch("ls-fields");
        let main = base.join("klark");
        let wt = base.join("klark-agent-names");
        let gitdir = main.join(".git/worktrees/klark-agent-names");
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(main.join(".git/HEAD"), "ref: refs/heads/master\n").unwrap();
        std::fs::write(gitdir.join("HEAD"), "ref: refs/heads/agent-pane-names\n").unwrap();
        std::fs::write(
            wt.join(".git"),
            "gitdir: ../klark/.git/worktrees/klark-agent-names\n",
        )
        .unwrap();
        let result = json!({"agents": [
            {"name": "klark", "agent": "claude", "agent_status": "working", "pane_id": "w1:p1",
             "cwd": main.display().to_string(), "terminal_title_stripped": "Review PR 42"},
            {"name": "klark-agent-names", "agent": "codex", "agent_status": "idle", "pane_id": "w1:p2",
             "cwd": "/elsewhere", "foreground_cwd": wt.display().to_string(), "terminal_title_stripped": ""},
            {"agent": "claude", "agent_status": "idle", "pane_id": "w1:p3", "cwd": "/"}
        ]});
        let mut got = agents_from_list("work", "m1", &result);
        crate::agent_index::link_parents(&mut got);
        let v = serde_json::to_value(&got).unwrap();
        assert_eq!(v[0]["address"], "m1/work/klark");
        assert!(
            v[0].get("parent").is_none(),
            "the main checkout has no parent"
        );
        assert_eq!(v[1]["parent"], "m1/work/klark", "the worktree's parent");
        assert!(v[2].get("parent").is_none());
        assert_eq!(v[0]["cwd"], main.display().to_string());
        assert_eq!(v[0]["checkout"], "main");
        assert_eq!(v[0]["branch"], "master");
        assert_eq!(v[0]["title"], "Review PR 42");
        assert!(v[0].get("worktree").is_none());
        assert_eq!(
            v[1]["cwd"],
            wt.display().to_string(),
            "the agent's own folder"
        );
        assert_eq!(v[1]["checkout"], "worktree");
        assert_eq!(v[1]["worktree"], "klark-agent-names");
        assert_eq!(v[1]["branch"], "agent-pane-names");
        assert!(v[1].get("title").is_none(), "an empty title is left out");
        // Pane ids still address an unnamed agent; no git, no checkout fields.
        assert_eq!(v[2]["address"], "m1/work/w1:p3");
        assert!(v[2].get("checkout").is_none() && v[2].get("branch").is_none());

        let home = base.display().to_string();
        let rows: Vec<_> = got.iter().map(|l| row(l, "local", Some(&home))).collect();
        assert_eq!(rows[0][3], "main (master)");
        assert_eq!(rows[0][4], "");
        assert_eq!(rows[1][4], "work/klark");
        assert_eq!(rows[0][5], "~/klark");
        assert_eq!(rows[1][3], "worktree klark-agent-names (agent-pane-names)");
        assert_eq!(rows[2][3], "local");
        assert_eq!(rows[2][5], "/");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn ls_table_for_a_pipe_keeps_every_cell_whole() {
        let title = "A very long terminal title an agent needs to read in full, every word";
        let mut a = agent("w", Some("portfolio-plan"), "p1");
        a.title = Some(title.into());
        a.cwd = Some("/Users/someone/a/deeply/nested/workspace/portfolio-plan".into());
        let table = render_table(&[row(&a, "local", None)], None);
        assert!(table.contains(title), "{table}");
        assert!(table.contains("/Users/someone/a/deeply/nested/workspace/portfolio-plan"));
        assert!(!table.contains('…'), "{table}");
    }

    #[test]
    fn ls_table_fits_120_columns_and_cuts_long_cells() {
        let mut a = agent("deemwar-one-os", Some("portfolio-plan"), "w62:p1");
        a.address = "LocalMacbookpro/deemwar-one-os/portfolio-plan".into();
        a.status = "working".into();
        a.cwd = Some("/Users/x/deemwarworkspace/agents/portfolio-plan".into());
        a.checkout = Some("main".into());
        a.branch = Some("main".into());
        a.title = Some(
            "Portfolio status: reconcile every open position against the broker statements".into(),
        );
        let mut b = agent("deemwar-one-os", Some("wfnexus-main"), "w63:p1");
        b.address = "LocalMacbookpro/deemwar-one-os/wfnexus-main".into();
        b.status = "unknown".into();
        b.cwd = Some("/Users/x/deemwarworkspace/worktrees/very/deep/nested/wfnexus-main".into());
        b.checkout = Some("worktree".into());
        b.worktree = Some("wfnexus-main".into());
        b.branch = Some("feature/a-rather-long-branch-name".into());
        b.title = Some("Fix login".into());
        let mut c = agent("deemwar-one-os", None, "w6F:p1");
        c.address = "LocalMacbookpro/deemwar-one-os/w6F:p1".into();
        let rows: Vec<_> = [a, b, c]
            .iter()
            .map(|l| row(l, "local", Some("/Users/x")))
            .collect();
        let table = render_table(&rows, Some(TABLE_WIDTH));
        println!("{table}");
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].starts_with("ADDRESS"));
        assert!(lines[0].trim_end().ends_with("TITLE"));
        for line in &lines {
            assert!(
                line.chars().count() <= 120,
                "{} > 120: {line}",
                line.chars().count()
            );
        }
        assert!(
            lines[1].contains("…portfolio-plan  "),
            "folder is cut, keeping its end"
        );
        assert!(lines[1].contains("main (main)"));
        assert!(lines[1].contains("Portfolio") && lines[1].ends_with('…'));
        assert!(
            lines[2].contains("worktree wfnexus-main  "),
            "the worktree name survives; the branch gives way"
        );
        assert!(lines[2].ends_with("Fix login"));
        assert!(lines[2].contains("  -  "), "unknown status shows as -");
        assert!(lines[3].starts_with("LocalMacbookpro/deemwar-one-os/w6F:p1"));
        assert!(lines[3].ends_with("local"), "outside git: just local");
        let remote = row(&agent("work", Some("rev"), "p1"), "remote", None);
        assert_eq!(remote[3], "remote");
        let mut r = agent("work", Some("rev"), "p1");
        r.checkout = Some("main".into());
        assert_eq!(row(&r, "remote", None)[3], "remote · main");
        // A short table is not padded out to 120 columns.
        let short = render_table(
            &[row(&agent("w", Some("a"), "p1"), "local", None)],
            Some(TABLE_WIDTH),
        );
        assert!(short.lines().all(|l| l.chars().count() < 60), "{short}");
        // PARENT shows once an agent works in a worktree.
        let mut wt = agent("work", Some("feat"), "p2");
        wt.checkout = Some("worktree".into());
        wt.parent = Some("m1/work/klark".into());
        let with_parent = render_table(&[row(&wt, "local", None)], Some(TABLE_WIDTH));
        assert!(with_parent.contains("PARENT") && with_parent.contains("work/klark"));
        assert!(
            !table.contains("PARENT"),
            "no worktree agent: no PARENT column"
        );
        // Columns line up: every row's STATUS starts where the header's does.
        let status_col = lines[0].find("STATUS").unwrap();
        for line in &lines[1..] {
            let prefix: String = line.chars().take(status_col).collect();
            assert!(prefix.ends_with("  "), "{line}");
        }
    }

    #[test]
    fn token_precedence() {
        let s = |v: &str| Some(v.to_string());
        assert_eq!(pick_token(s("env"), s("saved"), s("machine")), s("env"));
        assert_eq!(pick_token(None, s("saved"), s("machine")), s("saved"));
        assert_eq!(pick_token(s(""), None, s("machine")), s("machine"));
        assert_eq!(pick_token(None, None, None), None);
    }

    #[test]
    fn jwt_expiry_is_read() {
        use base64::Engine as _;
        let b = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let jwt = format!(
            "{}.{}.sig",
            b.encode("{}"),
            b.encode(r#"{"name":"a","exp":1700000000}"#)
        );
        assert_eq!(jwt_exp(&jwt), Some(1700000000));
        assert_eq!(jwt_exp("garbage"), None);
    }

    #[test]
    fn log_appends_and_tails() {
        let dir = scratch("log");
        for i in 0..5 {
            append_log(
                &dir,
                &LogEntry {
                    ts: format!("t{i}"),
                    dir: "sent".into(),
                    from: "a".into(),
                    to: "b".into(),
                    text: format!("m{i}"),
                    ok: i != 3,
                    error: (i == 3).then(|| "blocked".into()),
                },
            )
            .unwrap();
        }
        let last = tail_log(&dir, 2);
        assert_eq!(
            last.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(),
            ["m3", "m4"]
        );
        assert_eq!(last[0].error.as_deref(), Some("blocked"));
        assert_eq!(tail_log(&dir, 100).len(), 5);
        assert!(tail_log(&dir.join("missing"), 3).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn senders_are_kept_per_pane() {
        let dir = scratch("senders");
        record_sender(&dir, "work", "p1", "m/default/a").unwrap();
        record_sender(&dir, "default", "p1", "m/x/b").unwrap();
        record_sender(&dir, "work", "p1", "m/default/c").unwrap();
        assert_eq!(
            last_sender(&dir, "work", "p1").as_deref(),
            Some("m/default/c")
        );
        assert_eq!(last_sender(&dir, "default", "p1").as_deref(), Some("m/x/b"));
        assert_eq!(last_sender(&dir, "work", "p9"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hub_json_round_trips_with_mode_0600() {
        let dir = scratch("hub");
        let path = dir.join("hub.json");
        let cfg = HubConfig {
            hub: "http://hub:7470".into(),
            name: Some("box".into()),
            user: Some("alice".into()),
            jwt: Some("a.b.c".into()),
            expires_at: Some("2026-11-07T00:00:00Z".into()),
        };
        cfg.save_to(&path).unwrap();
        assert_eq!(HubConfig::load_from(&path), Some(cfg));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_draft_lifted_before_the_agent_blocked_is_returned_not_dropped() {
        let err = klark_link::upstream::CallError {
            message: "agent.prompt: agent_blocked became blocked".into(),
            error: Some(json!({
                "code": "agent_blocked",
                "message": "became blocked",
                "unsent_draft": "half a thought"
            })),
        };
        let text = prompt_error_text(err);
        assert!(text.starts_with("agent.prompt: agent_blocked "), "{text}");
        assert!(text.ends_with("\"half a thought\""), "{text}");
        let plain = klark_link::upstream::CallError {
            message: "agent.prompt: agent_blocked blocked".into(),
            error: Some(json!({"code": "agent_blocked", "message": "blocked"})),
        };
        assert_eq!(
            prompt_error_text(plain),
            "agent.prompt: agent_blocked blocked"
        );
    }

    #[test]
    fn hub_errors_are_one_clean_line() {
        assert_eq!(
            hub_error(
                "alpha/default/nobody",
                "klark msg: agent.prompt: agent_not_found agent target nobody not found"
            ),
            "alpha/default/nobody: no such agent (agent_not_found)"
        );
        assert_eq!(
            hub_error(
                "m/s/a",
                "hub refused: klark msg: klark msg: agent.prompt: agent_blocked busy"
            ),
            "m/s/a: the agent is blocked, waiting on a person (agent_blocked)"
        );
        assert_eq!(
            hub_error("m/s/a", "machine m is not linked"),
            "m/s/a: hub refused: machine m is not linked"
        );
        assert_eq!(
            hub_error("m/s/a", "HTTP 502"),
            "m/s/a: hub refused: HTTP 502"
        );
    }

    #[test]
    fn received_log_names_the_agent_by_full_address() {
        let named = json!({"agent": {"name": "rev", "pane_id": "w1:p1"}});
        assert_eq!(
            received_address("m1", "work", "w1:p1", Some(&named)),
            "m1/work/rev"
        );
        let unnamed = json!({"agent": {"pane_id": "w1:p2"}});
        assert_eq!(
            received_address("m1", "work", "x", Some(&unnamed)),
            "m1/work/w1:p2"
        );
        assert_eq!(
            received_address("m1", "work", "ghost", None),
            "m1/work/ghost"
        );
    }

    #[test]
    fn unknown_status_shows_as_dash_in_tables() {
        assert_eq!(shown_status("unknown"), "-");
        assert_eq!(shown_status(""), "-");
        assert_eq!(shown_status("working"), "working");
        let l = agent("work", Some("rev"), "p1");
        let mut l2 = l.clone();
        l2.status = "unknown".into();
        assert_eq!(row(&l2, "local", None)[1], "-");
        // JSON keeps the raw value.
        assert_eq!(serde_json::to_value(&l2).unwrap()["status"], "unknown");
    }

    #[test]
    fn args_keep_message_text_verbatim() {
        let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        // Options before <to>; after it, every word is text, flags included.
        let a = Args::parse(
            &v(&[
                "--from", "x/y/z", "work/rev", "hello", "--from", "a/b/c", "--json", "there",
            ]),
            &["--from"],
            &["--json"],
        )
        .unwrap();
        assert_eq!(
            a.positional,
            ["work/rev", "hello", "--from", "a/b/c", "--json", "there"]
        );
        assert_eq!(a.value("--from").as_deref(), Some("x/y/z"));
        assert!(!a.has("--json"));
        // `klark msg deliver ... <agent> -- <text>` (klark-link's form).
        let a = Args::parse(
            &v(&["--session", "w", "rev", "--", "-starts with a dash"]),
            &["--session"],
            &[],
        )
        .unwrap();
        assert_eq!(a.positional, ["rev", "-starts with a dash"]);
        let a = Args::parse(&v(&["--", "--json"]), &[], &["--json"]).unwrap();
        assert_eq!(a.positional, ["--json"]);
        let a = Args::parse(&v(&["rev", "--", "--json", "-n"]), &["--from"], &["--json"]).unwrap();
        assert_eq!(a.positional, ["rev", "--json", "-n"]);
        assert!(!a.has("--json"));
        assert!(Args::parse(&v(&["--bogus"]), &[], &[]).is_err());
    }

    #[test]
    fn wait_for_reply_takes_only_a_new_answer_to_me() {
        let dir = std::env::temp_dir().join(format!("klark-waitreply-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let entry = |dir: &str, from: &str, to: &str, text: &str| LogEntry {
            ts: String::new(),
            dir: dir.into(),
            from: from.into(),
            to: to.into(),
            text: text.into(),
            ok: false,
            error: Some("no agent cli-7 in any running session".into()),
        };
        let me = "m/default/cli-7";
        append_log(&dir, &entry("sent", "m/w/rev", me, "old answer")).unwrap();
        let seen = tail_log(&dir, usize::MAX).len();
        append_log(
            &dir,
            &entry("sent", "m/w/rev", "m/default/cli-8", "not mine"),
        )
        .unwrap();
        append_log(&dir, &entry("received", "m/w/rev", me, "not a reply")).unwrap();
        append_log(&dir, &entry("sent", "m/w/rev", me, "the answer")).unwrap();
        let got = wait_for_reply(&dir, seen, me, 1).unwrap();
        assert_eq!(got.text, "the answer");
        assert!(wait_for_reply(&dir, seen + 3, me, 0).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
