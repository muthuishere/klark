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

Addresses: <agent> | <session>/<agent> | <machine>/<session>/<agent>
  agent   = an agent name (klark msg name / klark agent rename) or a pane id
  session = a klark session name; the default session is \"default\"
  machine = the name the hub knows the machine by

commands:
  whoami [--json]                        this agent's own address
  ls [--local] [--json]                  agents in every running session (+ the hub's, unless --local)
  send <to> <text...> [--from <addr>] [--json]
                                         deliver a message as a prompt to another agent
  reply <text...>                        answer the last agent that messaged this pane
  log [-n N] [--json]                    recent sent/received messages
  name <new-name>                        rename this pane's agent (its address)
  login --hub <url> --user <name> [--name <machine>] [--password-stdin]
                                         get a hub token (JWT) for cross-machine messages
  logout                                 forget the saved hub token
  status                                 hub, token expiry, running sessions
  deliver --session <s> --from <addr> <agent> <text...>
                                         deliver locally (used by klark expose)

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

/// Flags (with or without values) and positionals; `--` ends flags.
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
            } else if a.starts_with('-') && a.len() > 1 && out.positional.is_empty() {
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

/// `[machine/][session/]agent`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub machine: Option<String>,
    pub session: Option<String>,
    pub agent: String,
}

pub fn parse_address(s: &str) -> Result<Address, String> {
    let parts: Vec<&str> = s.trim().split('/').collect();
    if parts.iter().any(|p| p.trim().is_empty()) {
        return Err(format!(
            "bad address {s:?}: use <agent>, <session>/<agent> or <machine>/<session>/<agent>"
        ));
    }
    let owned = |p: &str| p.trim().to_string();
    match parts.as_slice() {
        [a] => Ok(Address {
            machine: None,
            session: None,
            agent: owned(a),
        }),
        [s, a] => Ok(Address {
            machine: None,
            session: Some(owned(s)),
            agent: owned(a),
        }),
        [m, s, a] => Ok(Address {
            machine: Some(owned(m)),
            session: Some(owned(s)),
            agent: owned(a),
        }),
        _ => Err(format!(
            "bad address {s:?}: at most <machine>/<session>/<agent>"
        )),
    }
}

/// One agent in a local session; `--local --json` prints exactly these fields
/// (the shape the link answers `list_agents` with).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalAgent {
    pub session: String,
    pub name: Option<String>,
    pub agent: Option<String>,
    pub pane_id: String,
    pub status: String,
    pub address: String,
}

impl LocalAgent {
    fn matches(&self, agent: &str) -> bool {
        self.name.as_deref() == Some(agent) || self.pane_id == agent
    }
    /// The agent part of its address: the name if it has one, else the pane id.
    fn handle(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.pane_id)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    Local { session: String, agent: String },
    Remote { to: String },
}

/// Resolve an address per ADR 0009: short forms search local agents only; a
/// full address on this machine is local; another machine goes via the hub.
pub fn resolve(addr: &Address, agents: &[LocalAgent], my_machine: &str) -> Result<Route, String> {
    if let Some(machine) = &addr.machine {
        if machine != my_machine {
            return Ok(Route::Remote {
                to: format!(
                    "{machine}/{}/{}",
                    addr.session.as_deref().unwrap_or("default"),
                    addr.agent
                ),
            });
        }
    }
    let found: Vec<&LocalAgent> = agents
        .iter()
        .filter(|a| addr.session.as_deref().is_none_or(|s| a.session == s))
        .filter(|a| a.matches(&addr.agent))
        .collect();
    match found.as_slice() {
        [one] => Ok(Route::Local {
            session: one.session.clone(),
            agent: one.pane_id.clone(),
        }),
        [] => {
            let place = match &addr.session {
                Some(s) => format!("in session {s}"),
                None => "in any running session".to_string(),
            };
            Err(format!(
                "no agent {} {place} on this machine (see: klark msg ls)",
                addr.agent
            ))
        }
        many => Err(format!(
            "{} is ambiguous; use one of: {}",
            addr.agent,
            many.iter()
                .map(|a| format!("{}/{}", a.session, a.pane_id))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// The prompt the receiving agent sees (ADR 0009, Delivery).
pub fn format_prompt(from: &str, text: &str) -> String {
    format!("[klark msg from {from}] {text}\n(reply: klark msg reply \"<text>\")")
}

// ---------------------------------------------------------------- local sessions

fn session_socket(session: &str) -> PathBuf {
    let name = (session != crate::session::DEFAULT_SESSION_NAME).then_some(session);
    crate::session::api_socket_path_for(name)
}

fn call(socket: &Path, method: &str, params: Value) -> Result<Value, String> {
    let bin = std::env::current_exe().unwrap_or_else(|_| "klark".into());
    klark_link::upstream::Upstream::new(socket.to_path_buf(), bin).call(method, params)
}

fn agents_from_list(session: &str, machine: &str, result: &Value) -> Vec<LocalAgent> {
    result
        .get("agents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let pane_id = a.get("pane_id")?.as_str()?.to_string();
            let s = |k: &str| a.get(k).and_then(Value::as_str).map(str::to_string);
            let mut agent = LocalAgent {
                session: session.to_string(),
                name: s("name"),
                agent: s("agent").or_else(|| s("display_agent")),
                pane_id,
                status: s("agent_status").unwrap_or_else(|| "unknown".into()),
                address: String::new(),
            };
            agent.address = format!("{machine}/{session}/{}", agent.handle());
            Some(agent)
        })
        .collect()
}

fn running_sessions() -> Vec<String> {
    crate::session::list_sessions()
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.running)
        .map(|s| s.name)
        .collect()
}

fn local_agents(machine: &str) -> Vec<LocalAgent> {
    let mut out = Vec::new();
    for session in running_sessions() {
        match call(&session_socket(&session), "agent.list", json!({})) {
            Ok(result) => out.extend(agents_from_list(&session, machine, &result)),
            Err(e) => eprintln!("klark msg: session {session}: {e}"),
        }
    }
    out
}

// ---------------------------------------------------------------- identity

fn my_session() -> String {
    crate::session::active_name().unwrap_or_else(|| crate::session::DEFAULT_SESSION_NAME.into())
}

fn my_machine() -> String {
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
fn hub_base() -> Option<Result<String, String>> {
    let url = std::env::var("KLARK_HUB_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
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
        HubConfig::load().and_then(|h| h.jwt),
        std::env::var("KLARK_HUB_TOKEN").ok(),
    )
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
            print_table(local.iter().map(|l| row(l, "local")).collect());
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
        print_table(
            rows.iter()
                .map(|r| {
                    let s = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                    let place = if r["local"] == json!(true) {
                        "local"
                    } else {
                        "remote"
                    };
                    [s("address"), s("status"), s("agent"), place.to_string()]
                })
                .collect(),
        );
    }
    Ok(())
}

fn row(l: &LocalAgent, place: &str) -> [String; 4] {
    [
        l.address.clone(),
        l.status.clone(),
        l.agent.clone().unwrap_or_default(),
        place.to_string(),
    ]
}

fn print_table(rows: Vec<[String; 4]>) {
    if rows.is_empty() {
        println!("no agents found (are any klark sessions running agents?)");
        return;
    }
    let head = ["ADDRESS", "STATUS", "AGENT", "WHERE"].map(String::from);
    let mut w = [0usize; 4];
    for r in std::iter::once(&head).chain(rows.iter()) {
        for (i, c) in r.iter().enumerate() {
            w[i] = w[i].max(c.len());
        }
    }
    for r in std::iter::once(&head).chain(rows.iter()) {
        println!(
            "{:<w0$}  {:<w1$}  {:<w2$}  {}",
            r[0],
            r[1],
            r[2],
            r[3],
            w0 = w[0],
            w1 = w[1],
            w2 = w[2]
        );
    }
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

fn cmd_send(args: &[String]) -> R<()> {
    let a = Args::parse(args, &["--from"], &["--json"])?;
    let (to, words) = a
        .positional
        .split_first()
        .ok_or_else(|| Fail::Usage("send needs <to> <text...>".into()))?;
    let text = text_of(words)?;
    let from = a.value("--from").unwrap_or_else(|| whoami().address);
    let (to_addr, outcome) = send(to, &from, &text);
    let json = a.has("--json");
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

/// Send and log; returns the resolved destination address and the outcome.
fn send(to: &str, from: &str, text: &str) -> (String, Result<(), String>) {
    let machine = my_machine();
    let addr = match parse_address(to) {
        Ok(a) => a,
        Err(e) => return (to.to_string(), Err(e)),
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
            (shown, deliver(&session, &agent, from, text).map(|_| ()))
        }
        Ok(Route::Remote { to }) => {
            let r = send_via_hub(&to, from, text);
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
    (to_addr, result)
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
        Err(format!("hub refused: {}", resp.error_text()))
    }
}

/// Prompt a local agent; returns its pane id. Records the sender for reply and
/// logs the receipt.
fn deliver(session: &str, agent: &str, from: &str, text: &str) -> Result<String, String> {
    let socket = session_socket(session);
    let result = call(
        &socket,
        "agent.prompt",
        json!({"target": agent, "text": format_prompt(from, text)}),
    )
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
    if result.is_ok() {
        if let Err(e) = record_sender(&data_dir(), session, &pane, from) {
            eprintln!("klark msg: could not record sender: {e}");
        }
    }
    let err = result.as_ref().err().cloned();
    log(
        "received",
        from,
        &format!("{session}/{pane}"),
        text,
        err.as_deref(),
    );
    result.map(|_| pane)
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
    let a = Args::parse(args, &[], &["--json"])?;
    let text = text_of(&a.positional)?;
    let pane = my_pane().ok_or("reply works inside a klark pane (KLARK_PANE_ID is not set)")?;
    let to = last_sender(&data_dir(), &my_session(), &pane)
        .ok_or("nobody has messaged this pane yet; use klark msg send <to> <text>")?;
    let from = whoami().address;
    let (to_addr, outcome) = send(&to, &from, &text);
    outcome?;
    println!("delivered to {to_addr}");
    Ok(())
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

    fn agent(session: &str, name: Option<&str>, pane: &str) -> LocalAgent {
        let mut a = LocalAgent {
            session: session.into(),
            name: name.map(str::to_string),
            agent: Some("claude".into()),
            pane_id: pane.into(),
            status: "idle".into(),
            address: String::new(),
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
            "[klark msg from alice-laptop/default/reviewer] please review PR 12\n(reply: klark msg reply \"<text>\")"
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
            hub: "ws://hub:8080/agent".into(),
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
    fn args_keep_message_text_verbatim() {
        let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let a = Args::parse(
            &v(&["work/rev", "hello", "--from", "x/y/z", "there"]),
            &["--from"],
            &["--json"],
        )
        .unwrap();
        assert_eq!(a.positional, ["work/rev", "hello", "there"]);
        assert_eq!(a.value("--from").as_deref(), Some("x/y/z"));
        let a = Args::parse(&v(&["rev", "--", "--json", "-n"]), &["--from"], &["--json"]).unwrap();
        assert_eq!(a.positional, ["rev", "--json", "-n"]);
        assert!(!a.has("--json"));
        assert!(Args::parse(&v(&["--bogus"]), &[], &[]).is_err());
    }
}
