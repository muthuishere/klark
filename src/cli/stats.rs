//! `klark stats [--json]`: resource use of the server, the host and each agent pane.

use serde_json::Value;

use crate::api::schema::{EmptyParams, Method, Request};

pub(super) fn run_stats_command(args: &[String]) -> std::io::Result<i32> {
    let json = match args {
        [] => false,
        [flag] if flag == "--json" => true,
        _ => {
            eprintln!("usage: klark stats [--json]");
            return Ok(2);
        }
    };

    let response = super::send_request(&Request {
        id: "cli:stats".into(),
        method: Method::SystemStats(EmptyParams::default()),
    })?;
    if response.get("error").is_some() {
        return super::print_response(&response);
    }
    let Some(stats) = response.get("result") else {
        return super::print_response(&response);
    };
    if json {
        let mut stats = stats.clone();
        if let Some(map) = stats.as_object_mut() {
            map.remove("type");
        }
        println!("{}", serde_json::to_string(&stats).unwrap());
    } else {
        print!("{}", render_text(stats));
    }
    Ok(0)
}

/// `1.5 GiB`, `312 MiB`, `40 KiB`, `12 B`.
fn bytes_text(bytes: u64) -> String {
    const UNITS: [(&str, u64); 3] = [("GiB", 1 << 30), ("MiB", 1 << 20), ("KiB", 1 << 10)];
    for (unit, size) in UNITS {
        if bytes >= size {
            let value = bytes as f64 / size as f64;
            return if value >= 100.0 {
                format!("{value:.0} {unit}")
            } else {
                format!("{value:.1} {unit}")
            };
        }
    }
    format!("{bytes} B")
}

fn duration_text(seconds: u64) -> String {
    let (d, h, m) = (
        seconds / 86_400,
        seconds % 86_400 / 3600,
        seconds % 3600 / 60,
    );
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m {}s", seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

fn bytes_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_u64)
        .map_or_else(|| "-".to_owned(), bytes_text)
}

fn percent_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_f64)
        .map_or_else(|| "-".to_owned(), |p| format!("{p:.1}%"))
}

pub(super) fn render_text(stats: &Value) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    let klark = stats.get("klark").unwrap_or(&Value::Null);
    let host = stats.get("host").unwrap_or(&Value::Null);
    let panes = stats.get("panes").unwrap_or(&Value::Null);

    let _ = writeln!(
        out,
        "klark {}  pid {}  up {}",
        klark.get("version").and_then(Value::as_str).unwrap_or("?"),
        klark.get("pid").and_then(Value::as_u64).unwrap_or(0),
        duration_text(klark.get("uptime_s").and_then(Value::as_u64).unwrap_or(0)),
    );
    let _ = writeln!(
        out,
        "  memory {} (peak {})  cpu {}",
        bytes_field(klark, "rss_bytes"),
        bytes_field(klark, "peak_rss_bytes"),
        percent_field(klark, "cpu_percent"),
    );
    let load = host
        .get("load1")
        .and_then(Value::as_f64)
        .map_or_else(|| "-".to_owned(), |l| format!("{l:.2}"));
    let cpus = host
        .get("cpus")
        .and_then(Value::as_u64)
        .map_or_else(|| "-".to_owned(), |c| c.to_string());
    let _ = writeln!(
        out,
        "host: memory {} free of {}  load {load}  cpus {cpus}",
        bytes_field(host, "mem_available_bytes"),
        bytes_field(host, "mem_total_bytes"),
    );
    let _ = writeln!(
        out,
        "panes: {}  agents: {}",
        panes.get("count").and_then(Value::as_u64).unwrap_or(0),
        panes.get("agents").and_then(Value::as_u64).unwrap_or(0),
    );
    for agent in stats
        .get("agents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let _ = writeln!(
            out,
            "  {:<10} {:<8} {:<9} {:>9}  cpu {}",
            agent.get("pane_id").and_then(Value::as_str).unwrap_or("?"),
            agent.get("name").and_then(Value::as_str).unwrap_or("?"),
            agent.get("state").and_then(Value::as_str).unwrap_or("?"),
            bytes_field(agent, "rss_bytes"),
            percent_field(agent, "cpu_percent"),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_view_shows_numbers_and_dashes_for_missing_ones() {
        let stats = serde_json::json!({
            "type": "system_stats",
            "ts_ms": 1,
            "klark": {"pid": 7, "rss_bytes": 52_428_800u64, "cpu_percent": 1.5,
                      "uptime_s": 3700, "version": "0.9.5"},
            "host": {"mem_total_bytes": 17_179_869_184u64, "cpus": 8},
            "panes": {"count": 3, "agents": 1},
            "agents": [{"pane_id": "p_1", "name": "claude", "state": "working",
                        "rss_bytes": 1_610_612_736u64, "cpu_percent": 12.0}],
        });
        let text = render_text(&stats);
        assert!(text.contains("klark 0.9.5  pid 7  up 1h 1m"), "{text}");
        assert!(
            text.contains("memory 50.0 MiB (peak -)  cpu 1.5%"),
            "{text}"
        );
        assert!(
            text.contains("host: memory - free of 16.0 GiB  load -  cpus 8"),
            "{text}"
        );
        assert!(text.contains("panes: 3  agents: 1"), "{text}");
        assert!(
            text.contains("claude") && text.contains("1.5 GiB"),
            "{text}"
        );
    }

    #[test]
    fn units_scale() {
        assert_eq!(bytes_text(12), "12 B");
        assert_eq!(bytes_text(2048), "2.0 KiB");
        assert_eq!(bytes_text(300 << 20), "300 MiB");
        assert_eq!(duration_text(59), "59s");
        assert_eq!(duration_text(90_000), "1d 1h");
    }
}
