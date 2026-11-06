//! `klark expose` — connect this machine to a klark hub.
//!
//! The machine dials out to the hub (no inbound port) and streams panes only
//! while someone on the hub is looking at them. See crates/klark-link.
//!
//! `klark expose url` prints the hub link for a pane: what an agent hands back
//! when someone says "share this" (skills/klark-share, ADR 0008).

const USAGE: &str = "usage: klark expose --hub <ws://hub-host:8080/agent> --token <machine-token> [--name <machine-name>]
       klark expose url [--pane <pane-id>] [--hub <url>] [--name <machine-name>]
       (or set KLARK_HUB_URL / KLARK_HUB_TOKEN / KLARK_MACHINE_NAME; --pane defaults to KLARK_PANE_ID)";

struct Options {
    hub: Option<String>,
    token: Option<String>,
    name: Option<String>,
    pane: Option<String>,
}

fn parse(args: &[String], allow_pane: bool) -> Options {
    let mut o = Options {
        hub: std::env::var("KLARK_HUB_URL").ok(),
        token: std::env::var("KLARK_HUB_TOKEN").ok(),
        name: std::env::var("KLARK_MACHINE_NAME").ok(),
        pane: std::env::var("KLARK_PANE_ID").ok(),
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
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

pub fn run(args: &[String]) -> ! {
    if args.first().map(String::as_str) == Some("url") {
        print_url(&args[1..]);
    }
    let o = parse(args, false);
    let (Some(hub_url), Some(token)) = (o.hub, o.token) else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let machine_name = o.name.unwrap_or_else(hostname);
    let bin = std::env::current_exe().unwrap_or_else(|_| "klark".into());
    klark_link::run(klark_link::LinkConfig {
        hub_url,
        token,
        machine_id: machine_name.clone(),
        machine_name,
        klark_version: crate::build_info::version(),
        socket: crate::api::socket_path(),
        bin,
        uploads_dir: crate::session::data_dir_for(None).join("uploads"),
    })
}

fn print_url(args: &[String]) -> ! {
    let o = parse(args, true);
    let Some(hub_url) = o.hub else {
        eprintln!("no hub configured: pass --hub or set KLARK_HUB_URL\n{USAGE}");
        std::process::exit(2);
    };
    let Some(pane) = o.pane.filter(|p| !p.is_empty()) else {
        eprintln!("no pane: run this inside a klark pane or pass --pane <pane-id>");
        std::process::exit(2);
    };
    let machine_name = o.name.unwrap_or_else(hostname);
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
