//! `klark hub` — link this machine to klark hubs (ADR 0012).
//!
//! The hub (klarkhub, the `hub/` image) is deployed separately. `klark hub
//! connect --url <hub>` asks the hub for a device code, the owner approves the
//! machine in the browser, and the hub hands back a machine token. Saved hubs
//! live in `<config dir>/hubs.json` (0600): `{default, hubs: {name: {url,
//! token, machine}}}`. Then it runs `klark expose` against the hub. Tokens are
//! never printed.

use std::collections::BTreeMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const USAGE: &str =
    "usage: klark hub connect [--url http://host[:port]] [--as <name>] [<name>] [--agents a,b]
                         [--token <machine-token> | --token-stdin] [--no-browser] [--foreground]
       klark hub stop                   ends the connection (alias: disconnect)
       klark hub ls | use <name> | rm <name> | status [<name>]
       connect --url adds (or refreshes) a hub: approve this machine in the browser and the hub
       issues its token. With a <name>, connects to that saved hub; with nothing, the default.
       Saved in <config dir>/hubs.json (0600). --token / --token-stdin / KLARK_HUB_TOKEN skip the
       browser (scripts). The connection keeps running in the background (log: <config dir>/hub-<name>.log);
       --foreground keeps it in this terminal. `stop` ends it.";

const HTTP_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SavedHub {
    pub url: String,
    pub token: String,
    pub machine: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Hubs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default)]
    pub hubs: BTreeMap<String, SavedHub>,
}

impl Hubs {
    pub fn path() -> PathBuf {
        crate::config::config_dir().join("hubs.json")
    }

    pub fn load() -> Self {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Written 0600 via a temp file (same as hub.json).
    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        write_private(
            path,
            &serde_json::to_string_pretty(self).map_err(|e| e.to_string())?,
        )
    }

    /// Adds or replaces `name`; the first hub becomes the default.
    pub fn add(&mut self, name: &str, hub: SavedHub) {
        self.hubs.insert(name.to_string(), hub);
        if self
            .default
            .as_ref()
            .map_or(true, |d| !self.hubs.contains_key(d))
        {
            self.default = Some(name.to_string());
        }
    }

    pub fn use_default(&mut self, name: &str) -> Result<(), String> {
        if !self.hubs.contains_key(name) {
            return Err(format!("no saved hub named {name} (see `klark hub ls`)"));
        }
        self.default = Some(name.to_string());
        Ok(())
    }

    /// Removes `name`; returns a note about the default when it changed.
    pub fn remove(&mut self, name: &str) -> Result<Option<String>, String> {
        if self.hubs.remove(name).is_none() {
            return Err(format!("no saved hub named {name} (see `klark hub ls`)"));
        }
        if self.default.as_deref() != Some(name) {
            return Ok(None);
        }
        if self.hubs.len() == 1 {
            let only = self.hubs.keys().next().cloned().unwrap();
            self.default = Some(only.clone());
            Ok(Some(format!("default is now {only} (the only hub left)")))
        } else {
            self.default = None;
            Ok(Some(if self.hubs.is_empty() {
                "no hubs left".to_string()
            } else {
                "no default hub now: pick one with `klark hub use <name>`".to_string()
            }))
        }
    }

    pub fn get(&self, name: Option<&str>) -> Option<(String, SavedHub)> {
        let name = name.map(str::to_string).or_else(|| self.default.clone())?;
        self.hubs.get(&name).map(|h| (name, h.clone()))
    }
}

fn write_private(path: &Path, body: &str) -> Result<(), String> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
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

/// The default saved hub, for `klark msg` / `klark offer` when KLARK_HUB_URL
/// is not set.
pub fn default_hub() -> Option<SavedHub> {
    Hubs::load().get(None).map(|(_, h)| h)
}

/// A hub's default name: the URL's host.
pub fn name_from_url(url: &str) -> Result<String, String> {
    let rest = url
        .trim()
        .split_once("://")
        .map(|(_, r)| r)
        .ok_or(format!("hub url {url}: expected http(s)://host[:port]"))?;
    let hostport = rest.split('/').next().unwrap_or("");
    let host = match hostport.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => hostport.split(':').next().unwrap_or(""),
    };
    if host.is_empty() {
        return Err(format!("hub url {url}: no host"));
    }
    Ok(host.to_string())
}

/// The URL the machine link dials for a hub URL: its http(s) origin (plain
/// HTTP only, ADR 0013). Old `ws://host/agent` URLs map to `http://host`.
pub fn agent_url(hub: &str) -> Result<String, String> {
    let hub = hub.trim();
    if !hub.contains("://") {
        return Err(format!("hub url {hub}: expected http(s)://host[:port]"));
    }
    klark_link::http::hub_http_base(hub)
}

#[derive(Debug, Default, PartialEq)]
pub struct ConnectOptions {
    pub url: Option<String>,
    pub name: Option<String>,
    pub token: Option<String>,
    pub token_stdin: bool,
    pub no_browser: bool,
    pub agents: Vec<String>,
    /// Stay attached to this terminal instead of running in the background.
    pub foreground: bool,
}

pub fn parse_connect(args: &[String]) -> Result<ConnectOptions, String> {
    let mut o = ConnectOptions::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--token-stdin" => o.token_stdin = true,
            "--no-browser" => o.no_browser = true,
            "--foreground" | "-f" => o.foreground = true,
            "--url" | "--hub" | "--as" | "--token" | "--agents" | "--agent" => {
                let v = it.next().cloned().ok_or(format!("{arg} needs a value"))?;
                match arg.as_str() {
                    "--url" | "--hub" => o.url = Some(v),
                    "--as" => o.name = Some(v),
                    "--token" => o.token = Some(v),
                    _ => o.agents.push(v),
                }
            }
            other if other.starts_with('-') => return Err(format!("unknown argument: {other}")),
            name if o.name.is_none() => o.name = Some(name.to_string()),
            extra => return Err(format!("unexpected argument: {extra}")),
        }
    }
    if o.token.is_some() && o.token_stdin {
        return Err("pass --token or --token-stdin, not both".into());
    }
    Ok(o)
}

/// `klark expose` arguments for a saved hub.
pub fn expose_args(hub: &SavedHub, agents: &[String]) -> Result<Vec<String>, String> {
    let mut args = vec![
        "--hub".to_string(),
        agent_url(&hub.url)?,
        "--token".to_string(),
        hub.token.clone(),
        "--name".to_string(),
        hub.machine.clone(),
    ];
    for a in agents {
        args.extend(["--agents".to_string(), a.clone()]);
    }
    Ok(args)
}

/// How the device flow tells the person what to do.
pub trait Prompt {
    fn show(&mut self, url: &str, code: &str);
}

struct Terminal {
    browser: bool,
}

impl Prompt for Terminal {
    fn show(&mut self, url: &str, code: &str) {
        eprintln!("Open {url} and sign in to approve this machine (code {code})");
        if self.browser {
            let opener = if cfg!(target_os = "macos") {
                "open"
            } else {
                "xdg-open"
            };
            let _ = Command::new(opener)
                .arg(url)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
        }
    }
}

fn post(base: &str, path: &str, body: Value) -> Result<Value, String> {
    let r = klark_link::http::request(
        "POST",
        &format!("{base}{path}"),
        None,
        Some(&body),
        HTTP_TIMEOUT,
    )?;
    if !r.ok() {
        return Err(format!("POST {path}: {}", r.error_text()));
    }
    serde_json::from_str(&r.body).map_err(|e| format!("POST {path}: {e}"))
}

/// Device authorization: start, show the code, poll until approved.
/// Returns (token, machine name the hub registered).
pub fn device_flow(
    base: &str,
    machine: &str,
    prompt: &mut dyn Prompt,
) -> Result<(String, String), String> {
    let start = post(base, "/api/connect/start", json!({ "name": machine }))?;
    let field = |k: &str| start.get(k).and_then(Value::as_str).map(str::to_string);
    let poll_id = field("poll_id").ok_or("POST /api/connect/start: no poll_id")?;
    let code = field("code").unwrap_or_default();
    let url = field("url").unwrap_or_else(|| base.to_string());
    let interval = Duration::from_secs(start.get("interval").and_then(Value::as_u64).unwrap_or(5));
    let expires = Duration::from_secs(
        start
            .get("expires_in")
            .and_then(Value::as_u64)
            .unwrap_or(600),
    );
    prompt.show(&url, &code);
    let began = Instant::now();
    loop {
        if began.elapsed() > expires {
            return Err("the approval request expired; run `klark hub connect` again".into());
        }
        std::thread::sleep(interval);
        let r = post(base, "/api/connect/poll", json!({ "poll_id": poll_id }))?;
        match r.get("status").and_then(Value::as_str) {
            Some("approved") => {
                let token = r
                    .get("token")
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
                    .ok_or("approved, but the hub sent no token")?;
                let name = r.get("machine").and_then(Value::as_str).unwrap_or(machine);
                return Ok((token.to_string(), name.to_string()));
            }
            Some("pending") => continue,
            Some("denied") => return Err("the hub denied this machine".into()),
            Some("expired") => {
                return Err("the approval request expired; run `klark hub connect` again".into())
            }
            other => {
                return Err(format!(
                    "POST /api/connect/poll: unexpected status {other:?}"
                ))
            }
        }
    }
}

/// Whether the hub refuses `token` (401/403 on the machine endpoint).
fn token_rejected(base: &str, token: &str) -> bool {
    matches!(
        klark_link::http::request("GET", &format!("{base}/agent"), Some(token), None, HTTP_TIMEOUT),
        Ok(r) if r.status == 401 || r.status == 403
    )
}

pub(crate) fn machine_name() -> String {
    std::env::var("KLARK_MACHINE_NAME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(crate::expose_cmd::hostname)
}

fn fail(msg: impl std::fmt::Display) -> ! {
    eprintln!("klark hub: {msg}");
    std::process::exit(1);
}

fn one_name(rest: &[String]) -> Result<String, String> {
    match rest {
        [n] if !n.starts_with('-') => Ok(n.clone()),
        _ => Err("needs exactly one hub name".into()),
    }
}

pub fn run(args: &[String]) -> ! {
    let rest = args.get(1..).unwrap_or(&[]);
    let result = match args.first().map(String::as_str) {
        Some("connect") => parse_connect(rest).map(connect),
        Some("ls") if rest.is_empty() => Ok(ls()),
        Some("disconnect" | "stop") if rest.len() <= 1 => Ok(disconnect(rest.first().map(String::as_str))),
        Some("use") => {
            one_name(rest).map(|n| edit(|h| h.use_default(&n).map(|_| format!("default hub: {n}"))))
        }
        Some("rm") => one_name(rest).map(|n| {
            edit(|h| {
                h.remove(&n).map(|note| match note {
                    Some(note) => format!("removed {n}; {note}"),
                    None => format!("removed {n}"),
                })
            })
        }),
        Some("status") if rest.len() <= 1 => Ok(status(rest.first().map(String::as_str))),
        Some("-h" | "--help") => {
            println!("{USAGE}");
            std::process::exit(0);
        }
        _ => Err(String::new()),
    };
    if let Err(e) = result {
        if !e.is_empty() {
            eprintln!("{e}");
        }
        eprintln!("{USAGE}");
        std::process::exit(2);
    }
    std::process::exit(0)
}

fn edit(f: impl FnOnce(&mut Hubs) -> Result<String, String>) {
    let path = Hubs::path();
    let mut hubs = Hubs::load_from(&path);
    let msg = f(&mut hubs).unwrap_or_else(|e| fail(e));
    hubs.save_to(&path).unwrap_or_else(|e| fail(e));
    println!("klark hub: {msg}");
}

fn connect(o: ConnectOptions) {
    let path = Hubs::path();
    let mut hubs = Hubs::load_from(&path);
    let explicit_token = if o.token_stdin {
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
        Some(line.trim().to_string()).filter(|t| !t.is_empty())
    } else {
        o.token.clone()
    }
    .or_else(|| {
        std::env::var("KLARK_HUB_TOKEN")
            .ok()
            .filter(|t| !t.is_empty())
    });

    let (name, url, saved) = match &o.url {
        Some(url) => {
            agent_url(url).unwrap_or_else(|e| fail(e));
            let name = o
                .name
                .clone()
                .unwrap_or_else(|| name_from_url(url).unwrap_or_else(|e| fail(e)));
            let saved = hubs.hubs.get(&name).filter(|h| h.url == *url).cloned();
            (name, url.clone(), saved)
        }
        None => match hubs.get(o.name.as_deref()) {
            Some((name, h)) => (name, h.url.clone(), Some(h)),
            None => fail(match &o.name {
                Some(n) => format!("no saved hub named {n} (see `klark hub ls`)"),
                None => {
                    "no default hub: run `klark hub connect --url http://host:port`".to_string()
                }
            }),
        },
    };
    let base = klark_link::http::hub_http_base(&url).unwrap_or_else(|e| fail(e));
    let machine = saved
        .as_ref()
        .map(|h| h.machine.clone())
        .unwrap_or_else(machine_name);

    let hub = match (explicit_token, saved) {
        (Some(token), _) => SavedHub {
            url,
            token,
            machine,
        },
        (None, Some(h)) if !token_rejected(&base, &h.token) => h,
        (None, saved) => {
            if saved.is_some() {
                eprintln!(
                    "klark hub: the hub rejected the saved token; approving this machine again"
                );
            }
            let mut prompt = Terminal {
                browser: !o.no_browser,
            };
            let (token, machine) =
                device_flow(&base, &machine, &mut prompt).unwrap_or_else(|e| fail(e));
            eprintln!("klark hub: approved as {machine}");
            SavedHub {
                url,
                token,
                machine,
            }
        }
    };
    // The hub you connect to is the default from now on.
    let make_default = hubs.default.as_deref() != Some(name.as_str());
    if hubs.hubs.get(&name) != Some(&hub) || make_default {
        hubs.add(&name, hub.clone());
        hubs.default = Some(name.clone());
        hubs.save_to(&path).unwrap_or_else(|e| fail(e));
        eprintln!(
            "klark hub: saved {name} ({}) in {}",
            hub.url,
            path.display()
        );
    }
    if !o.foreground {
        background(&name, &o.agents);
        return;
    }
    match expose_args(&hub, &o.agents) {
        Ok(args) => crate::expose_cmd::run(&args),
        Err(e) => fail(e),
    }
}

fn pid_path(name: &str) -> std::path::PathBuf {
    Hubs::path().with_file_name(format!("hub-{name}.pid"))
}

/// Stop the background connection to `name`, if one is running. True when one was stopped.
fn stop_background(name: &str) -> bool {
    let path = pid_path(name);
    let Some(pid) = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
    else {
        return false;
    };
    let _ = std::fs::remove_file(&path);
    // Only a klark process: the pid may have been reused since.
    let ours = Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains("klark hub connect"))
        .unwrap_or(false);
    ours && Command::new("kill")
        .arg(pid.to_string())
        .status()
        .is_ok_and(|s| s.success())
}

/// Run the connection detached from this terminal: closing the tab or Ctrl-C no longer
/// drops the machine off the hub. The child reads the saved token, so none is on its argv.
fn background(name: &str, agents: &[String]) {
    // One hub at a time: connecting here ends any other background connection.
    for other in Hubs::load().hubs.keys() {
        if stop_background(other) && other != name {
            eprintln!("klark hub: disconnected from {other}");
        }
    }
    let log_path = Hubs::path().with_file_name(format!("hub-{name}.log"));
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .unwrap_or_else(|e| fail(format!("{}: {e}", log_path.display())));
    let exe = std::env::current_exe().unwrap_or_else(|e| fail(e.to_string()));
    let mut cmd = Command::new(exe);
    cmd.args(["hub", "connect", name, "--foreground", "--no-browser"]);
    for a in agents {
        cmd.args(["--agents", a]);
    }
    cmd.stdin(Stdio::null())
        .stdout(log.try_clone().unwrap_or_else(|e| fail(e.to_string())))
        .stderr(log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let child = cmd.spawn().unwrap_or_else(|e| fail(e.to_string()));
    let _ = std::fs::write(pid_path(name), child.id().to_string());
    eprintln!(
        "klark hub: connected in the background (pid {}, log {}); stop with `klark hub stop`",
        child.id(),
        log_path.display()
    );
}

fn disconnect(name: Option<&str>) {
    let hubs = Hubs::load();
    // No name: whatever is connected.
    let names: Vec<String> = match name {
        Some(n) if !hubs.hubs.contains_key(n) => fail(format!("no saved hub named {n} (see `klark hub ls`)")),
        Some(n) => vec![n.to_string()],
        None => hubs.hubs.keys().cloned().collect(),
    };
    let stopped: Vec<String> = names.into_iter().filter(|n| stop_background(n)).collect();
    if stopped.is_empty() {
        println!("klark hub: not connected to any hub");
    }
    for n in stopped {
        println!("klark hub: disconnected from {n}");
    }
}

fn reachable(url: &str) -> String {
    match klark_link::http::hub_http_base(url).and_then(|base| {
        klark_link::http::request(
            "GET",
            &format!("{base}/healthz"),
            None,
            None,
            Duration::from_secs(3),
        )
    }) {
        Ok(r) if r.ok() => "reachable".to_string(),
        Ok(r) => format!("unhealthy ({})", r.error_text()),
        Err(e) => format!("unreachable ({e})"),
    }
}

/// `klark hub ls` rows; never tokens.
pub fn ls_rows(hubs: &Hubs, reach: impl Fn(&str) -> String) -> Vec<String> {
    hubs.hubs
        .iter()
        .map(|(name, h)| {
            let mark = if hubs.default.as_deref() == Some(name) {
                "*"
            } else {
                " "
            };
            format!(
                "{mark} {name}\t{}\tmachine {}\t{}",
                h.url,
                h.machine,
                reach(&h.url)
            )
        })
        .collect()
}

fn ls() {
    let hubs = Hubs::load();
    if hubs.hubs.is_empty() {
        println!("no saved hubs (run `klark hub connect --url http://host:port`)");
        return;
    }
    for row in ls_rows(&hubs, reachable) {
        println!("{row}");
    }
}

fn status(name: Option<&str>) {
    let Some((name, h)) = Hubs::load().get(name) else {
        println!("hub:     none (run `klark hub connect --url http://host:port`)");
        return;
    };
    println!("hub:     {name} {} ({})", h.url, reachable(&h.url));
    println!("machine: {}", h.machine);
    let ws = agent_url(&h.url).unwrap_or_default();
    println!(
        "linked:  {}",
        if expose_running(&ws, &name) {
            "yes (klark expose is running)"
        } else {
            "no (run `klark hub connect`)"
        }
    );
}

/// Whether a `klark expose` / `klark hub connect` for this hub runs here.
fn expose_running(ws: &str, name: &str) -> bool {
    let Ok(out) = Command::new("ps")
        .args(["-ax", "-o", "pid=,command="])
        .output()
    else {
        return false;
    };
    let me = std::process::id().to_string();
    String::from_utf8_lossy(&out.stdout).lines().any(|l| {
        let l = l.trim_start();
        let (pid, cmd) = l.split_once(' ').unwrap_or((l, ""));
        pid != me
            && cmd.contains("klark")
            && (cmd.contains(" hub connect") || cmd.contains(name) || cmd.contains(ws))
            && (cmd.contains(" hub connect") || cmd.contains(" expose "))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn hub(url: &str) -> SavedHub {
        SavedHub {
            url: url.into(),
            token: "secret-token".into(),
            machine: "box".into(),
        }
    }

    #[test]
    fn parses_connect() {
        assert_eq!(parse_connect(&[]).unwrap(), ConnectOptions::default());
        let o = parse_connect(&s(&[
            "--url",
            "http://h:7470",
            "--as",
            "lab",
            "--token-stdin",
            "--no-browser",
            "--agents",
            "a,b",
        ]))
        .unwrap();
        assert_eq!(o.url.as_deref(), Some("http://h:7470"));
        assert_eq!(o.name.as_deref(), Some("lab"));
        assert!(o.token_stdin && o.no_browser);
        assert_eq!(o.agents, s(&["a,b"]));
        assert_eq!(
            parse_connect(&s(&["lab"])).unwrap().name.as_deref(),
            Some("lab")
        );
        assert!(parse_connect(&s(&["a", "b"])).is_err());
        assert!(parse_connect(&s(&["--token"])).is_err());
        assert!(parse_connect(&s(&["--token", "t", "--token-stdin"])).is_err());
        assert!(parse_connect(&s(&["--bogus"])).is_err());
    }

    #[test]
    fn urls_and_names() {
        assert_eq!(agent_url("http://h:7470").unwrap(), "http://h:7470");
        assert_eq!(
            agent_url("https://hub.example.com/").unwrap(),
            "https://hub.example.com"
        );
        assert_eq!(agent_url("ws://h:1/agent").unwrap(), "http://h:1");
        assert!(agent_url("ftp://h").is_err());
        assert!(agent_url("h:7470").is_err());
        assert_eq!(
            name_from_url("https://hub.example.com:8443/x").unwrap(),
            "hub.example.com"
        );
        assert_eq!(name_from_url("http://[::1]:7470").unwrap(), "::1");
        assert!(name_from_url("nohost").is_err());
    }

    #[test]
    fn add_ls_use_rm() {
        let mut h = Hubs::default();
        h.add("a", hub("http://a:1"));
        h.add("b", hub("http://b:1"));
        assert_eq!(h.default.as_deref(), Some("a"), "first hub is the default");
        let rows = ls_rows(&h, |_| "reachable".into());
        assert_eq!(rows[0], "* a\thttp://a:1\tmachine box\treachable");
        assert!(rows[1].starts_with("  b\t"));
        assert!(rows.iter().all(|r| !r.contains("secret-token")));
        h.use_default("b").unwrap();
        assert!(h.use_default("zzz").is_err());
        assert_eq!(h.get(None).unwrap().0, "b");
        assert_eq!(
            h.remove("b").unwrap().as_deref(),
            Some("default is now a (the only hub left)")
        );
        h.add("c", hub("http://c:1"));
        h.add("d", hub("http://d:1"));
        assert!(h.remove("a").unwrap().unwrap().contains("klark hub use"));
        assert_eq!(h.default, None);
        assert_eq!(h.remove("c").unwrap(), None);
        assert!(h.remove("nope").is_err());
        assert_eq!(
            expose_args(&hub("https://x"), &s(&["r"])).unwrap(),
            s(&[
                "--hub",
                "https://x",
                "--token",
                "secret-token",
                "--name",
                "box",
                "--agents",
                "r"
            ])
        );
    }

    #[test]
    fn file_format_and_mode() {
        let dir = std::env::temp_dir().join(format!("klark-hubs-{}", std::process::id()));
        let path = dir.join("hubs.json");
        let mut h = Hubs::default();
        h.add("lab", hub("http://lab:7470"));
        h.save_to(&path).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            v,
            json!({"default": "lab", "hubs": {"lab": {"url": "http://lab:7470", "token": "secret-token", "machine": "box"}}})
        );
        assert_eq!(Hubs::load_from(&path), h);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(Hubs::load_from(&dir.join("missing.json")), Hubs::default());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Serves one canned JSON answer per request, in order; returns the base
    /// URL and the request bodies seen.
    fn fake_hub(answers: Vec<Value>) -> (String, std::thread::JoinHandle<Vec<String>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let t = std::thread::spawn(move || {
            let mut seen = Vec::new();
            for a in answers {
                let (mut c, _) = l.accept().unwrap();
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = c.read(&mut chunk).unwrap();
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some((head, body)) = text.split_once("\r\n\r\n") {
                        let len = head
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if body.len() >= len {
                            seen.push(format!("{} {}", head.lines().next().unwrap(), body));
                            break;
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                let body = a.to_string();
                let _ = write!(c, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            }
            seen
        });
        (base, t)
    }

    #[derive(Default)]
    struct Shown(Vec<(String, String)>);
    impl Prompt for Shown {
        fn show(&mut self, url: &str, code: &str) {
            self.0.push((url.into(), code.into()));
        }
    }

    fn start() -> Value {
        json!({"code": "ABCD-EFGH", "poll_id": "p1", "url": "http://hub/approve", "expires_in": 60, "interval": 0})
    }

    #[test]
    fn device_flow_pending_then_approved() {
        let (base, t) = fake_hub(vec![
            start(),
            json!({"status": "pending"}),
            json!({"status": "approved", "token": "tok", "machine": "box-1"}),
        ]);
        let mut shown = Shown::default();
        let got = device_flow(&base, "box", &mut shown).unwrap();
        assert_eq!(got, ("tok".to_string(), "box-1".to_string()));
        assert_eq!(
            shown.0,
            vec![("http://hub/approve".into(), "ABCD-EFGH".into())]
        );
        let seen = t.join().unwrap();
        assert!(
            seen[0].starts_with("POST /api/connect/start ") && seen[0].contains("\"name\":\"box\"")
        );
        assert!(
            seen[1].starts_with("POST /api/connect/poll ")
                && seen[1].contains("\"poll_id\":\"p1\"")
        );
    }

    #[test]
    fn device_flow_denied_and_expired() {
        let (base, t) = fake_hub(vec![start(), json!({"status": "denied"})]);
        assert!(device_flow(&base, "box", &mut Shown::default())
            .unwrap_err()
            .contains("denied"));
        t.join().unwrap();
        let (base, t) = fake_hub(vec![
            start(),
            json!({"status": "pending"}),
            json!({"status": "expired"}),
        ]);
        assert!(device_flow(&base, "box", &mut Shown::default())
            .unwrap_err()
            .contains("expired"));
        t.join().unwrap();
    }
}
