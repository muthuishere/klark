//! `klark expose` — connect this machine to a klark hub.
//!
//! The machine dials out to the hub (no inbound port) and streams panes only
//! while someone on the hub is looking at them. See crates/klark-link.
//!
//! `klark expose url` prints the hub link for a pane: what an agent hands back
//! when someone says "share this" (skills/klark, ADR 0008).

const USAGE: &str = "usage: klark expose --hub <ws://hub-host:8080/agent> --token <machine-token> [--name <machine-name>] [--agents <name>[,<name>...]]
       klark expose url [--pane <pane-id>] [--hub <url>] [--name <machine-name>] [--agents <name>[,<name>...]]
       (or set KLARK_HUB_URL / KLARK_HUB_TOKEN / KLARK_MACHINE_NAME / KLARK_EXPOSE_AGENTS; --pane defaults to KLARK_PANE_ID)
       --agents (or repeated --agent) exposes only panes running those agents: an agent name, <session>/<name>,
       or an agent type such as claude (a name beats a type); case-insensitive. Default: every pane of every session.";

struct Options {
    hub: Option<String>,
    token: Option<String>,
    name: Option<String>,
    pane: Option<String>,
    /// `--agents` / `--agent` values; `None` when neither was given.
    agents: Option<Vec<String>>,
}

fn parse(args: &[String], allow_pane: bool) -> Options {
    let mut o = Options {
        hub: std::env::var("KLARK_HUB_URL").ok(),
        token: std::env::var("KLARK_HUB_TOKEN").ok(),
        name: std::env::var("KLARK_MACHINE_NAME").ok(),
        pane: std::env::var("KLARK_PANE_ID").ok(),
        agents: None,
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if matches!(arg.as_str(), "--agents" | "--agent") {
            match it.next() {
                Some(v) => o.agents.get_or_insert_with(Vec::new).push(v.clone()),
                None => {
                    eprintln!("{arg} needs a value\n{USAGE}");
                    std::process::exit(2);
                }
            }
            continue;
        }
        let slot = match arg.as_str() {
            "--hub" => &mut o.hub,
            "--token" => &mut o.token,
            "--name" => &mut o.name,
            "--pane" if allow_pane => &mut o.pane,
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown argument: {other}\n{USAGE}");
                std::process::exit(2);
            }
        };
        match it.next() {
            Some(value) => *slot = Some(value.clone()),
            None => {
                eprintln!("{arg} needs a value\n{USAGE}");
                std::process::exit(2);
            }
        }
    }
    o
}

/// `--agents` (flags win over KLARK_EXPOSE_AGENTS); `None` exposes everything.
fn agent_filter(o: &Options) -> Option<klark_link::filter::AgentFilter> {
    let values = o
        .agents
        .clone()
        .or_else(|| std::env::var("KLARK_EXPOSE_AGENTS").ok().map(|v| vec![v]))
        .unwrap_or_default();
    match klark_link::filter::AgentFilter::parse(&values) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            std::process::exit(2);
        }
    }
}

/// What the startup line says is exposed.
fn exposing_line(filter: Option<&klark_link::filter::AgentFilter>) -> String {
    match filter {
        Some(f) => format!(
            "klark expose: exposing agents: {} (all sessions)",
            f.describe()
        ),
        None => "klark expose: exposing everything (every pane of every session)".to_string(),
    }
}

/// Every local agent as the link's filter sees it.
fn local_agent_infos(machine: &str) -> Vec<klark_link::filter::AgentInfo> {
    crate::agent_index::local_agents(machine)
        .into_iter()
        .map(|a| klark_link::filter::AgentInfo {
            session: a.session,
            pane_id: a.pane_id,
            name: a.name,
            kind: a.agent,
        })
        .collect()
}

pub fn run(args: &[String]) -> ! {
    if args.first().map(String::as_str) == Some("url") {
        print_url(&args[1..]);
    }
    let o = parse(args, false);
    let (Some(hub_url), Some(token)) = (o.hub.clone(), o.token.clone()) else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let agents = agent_filter(&o);
    eprintln!("{}", exposing_line(agents.as_ref()));
    let machine_name = o.name.unwrap_or_else(hostname);
    let bin = std::env::current_exe().unwrap_or_else(|_| "klark".into());
    klark_link::run(klark_link::LinkConfig {
        hub_url,
        token,
        machine_id: machine_name.clone(),
        machine_name,
        klark_version: crate::build_info::version(),
        socket: default_socket(),
        bin,
        uploads_dir: crate::session::data_dir_for(None).join("uploads"),
        // Every running session, not only this one (ids namespaced
        // `<session>/<id>`, see crates/klark-link/src/sessions.rs).
        sessions: Some(klark_link::sessions::SessionSource::new(|| {
            crate::agent_index::running_sessions()
                .into_iter()
                .map(|s| {
                    let socket = crate::agent_index::session_socket(&s);
                    (s, socket)
                })
                .collect()
        })),
        agents,
    })
}

/// The default session's socket: plain pane ids on the wire belong to it.
/// Run from inside a named session, that is still the default session; an
/// explicit KLARK_SOCKET_PATH (no session) is honoured as before.
fn default_socket() -> std::path::PathBuf {
    match crate::session::active_name() {
        Some(_) => crate::session::api_socket_path_for(None),
        None => crate::api::socket_path(),
    }
}

fn print_url(args: &[String]) -> ! {
    let o = parse(args, true);
    let Some(hub_url) = o.hub.clone() else {
        eprintln!("no hub configured: pass --hub or set KLARK_HUB_URL\n{USAGE}");
        std::process::exit(2);
    };
    let Some(pane) = o.pane.clone().filter(|p| !p.is_empty()) else {
        eprintln!("no pane: run this inside a klark pane or pass --pane <pane-id>");
        std::process::exit(2);
    };
    let filter = agent_filter(&o);
    let machine_name = o.name.unwrap_or_else(hostname);
    let pane = wire_pane_id(crate::session::active_name().as_deref(), &pane);
    if let Some(filter) = &filter {
        let (session, local) = klark_link::sessions::decode_id(&pane);
        let exposed = filter.exposed_panes(&local_agent_infos(&machine_name));
        if !exposed.contains(&(session.to_string(), local.to_string())) {
            eprintln!(
                "pane {pane} is not exposed: klark expose shares only agents {} (--agents / KLARK_EXPOSE_AGENTS); \
                 add this agent's name or type to --agents and restart klark expose",
                filter.describe()
            );
            std::process::exit(2);
        }
    }
    match klark_link::pane_url(&hub_url, &machine_name, &pane) {
        Ok(url) => {
            println!("{url}");
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    }
}

/// The pane id the hub knows: namespaced `<session>/<id>` outside the default
/// session (an id that already carries a session is left alone).
fn wire_pane_id(session: Option<&str>, pane: &str) -> String {
    match session {
        Some(s) if !pane.contains('/') => klark_link::sessions::encode_id(s, pane),
        _ => pane.to_string(),
    }
}

pub(crate) fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(os_hostname)
        .unwrap_or_else(|| "klark-machine".to_string())
}

/// gethostname(2): macOS has no /etc/hostname and zsh does not export
/// HOSTNAME, so without this every Mac reported "klark-machine". `klark msg`
/// needs the same name the link reports (ADR 0009 addresses).
#[cfg(unix)]
fn os_hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if rc != 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let name = String::from_utf8_lossy(&buf[..end]).trim().to_string();
    let name = name.strip_suffix(".local").unwrap_or(&name).to_string();
    (!name.is_empty()).then_some(name)
}

#[cfg(not(unix))]
fn os_hostname() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn startup_line_names_what_is_exposed() {
        let f = klark_link::filter::AgentFilter::parse(&["reviewer,builder"]).unwrap();
        assert_eq!(
            super::exposing_line(f.as_ref()),
            "klark expose: exposing agents: reviewer, builder (all sessions)"
        );
        assert_eq!(
            super::exposing_line(None),
            "klark expose: exposing everything (every pane of every session)"
        );
    }

    #[test]
    fn url_pane_id_is_namespaced_outside_the_default_session() {
        assert_eq!(super::wire_pane_id(None, "p1"), "p1");
        assert_eq!(super::wire_pane_id(Some("work"), "p1"), "work/p1");
        assert_eq!(super::wire_pane_id(Some("work"), "work/p1"), "work/p1");
    }
}
