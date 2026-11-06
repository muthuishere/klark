//! `klark link` — connect this machine to a klark hub.
//!
//! The machine dials out to the hub (no inbound port) and streams panes only
//! while someone on the hub is looking at them. See crates/klark-link.

const USAGE: &str = "usage: klark link --hub <wss://host/agent> --token <machine-token> [--name <machine-name>]\n       (or set KLARK_HUB_URL / KLARK_HUB_TOKEN / KLARK_MACHINE_NAME)";

pub fn run(args: &[String]) -> ! {
    let mut hub = std::env::var("KLARK_HUB_URL").ok();
    let mut token = std::env::var("KLARK_HUB_TOKEN").ok();
    let mut name = std::env::var("KLARK_MACHINE_NAME").ok();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let slot = match arg.as_str() {
            "--hub" => &mut hub,
            "--token" => &mut token,
            "--name" => &mut name,
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
    let (Some(hub_url), Some(token)) = (hub, token) else {
        eprintln!("{USAGE}");
        std::process::exit(2);
    };
    let machine_name = name.unwrap_or_else(hostname);
    let bin = std::env::current_exe().unwrap_or_else(|_| "klark".into());
    klark_link::run(klark_link::LinkConfig {
        hub_url,
        token,
        machine_id: machine_name.clone(),
        machine_name,
        klark_version: crate::build_info::version(),
        socket: crate::api::socket_path(),
        bin,
    })
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .unwrap_or_else(|| "klark-machine".to_string())
}
