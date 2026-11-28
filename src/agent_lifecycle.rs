//! Agent lifecycle helpers: how long an agent has been in its state, closed
//! (resumable) agent records, pins, and which agents a bulk close may touch.

use serde_json::Value;

pub(crate) fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `90s`, `30m`, `3h`, `1d`, or a mix such as `1h30m`.
pub(crate) fn parse_duration(text: &str) -> Result<u64, String> {
    let bad = || format!("invalid duration {text:?}: use 90s, 30m, 3h, 1d or 1h30m");
    let text = text.trim();
    if text.is_empty() {
        return Err(bad());
    }
    let mut total: u64 = 0;
    let mut digits = String::new();
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
            continue;
        }
        let unit: u64 = match ch {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            'd' => 86400,
            _ => return Err(bad()),
        };
        let n: u64 = digits.parse().map_err(|_| bad())?;
        digits.clear();
        total = n
            .checked_mul(unit)
            .and_then(|v| total.checked_add(v))
            .ok_or_else(bad)?;
    }
    if !digits.is_empty() {
        return Err(bad());
    }
    Ok(total)
}

/// `45s`, `5m`, `3h12m`, `2d4h`: the two largest non-zero units.
pub(crate) fn format_duration(secs: u64) -> String {
    let (d, h, m, s) = (secs / 86400, secs % 86400 / 3600, secs % 3600 / 60, secs % 60);
    if d > 0 {
        if h > 0 { format!("{d}d{h}h") } else { format!("{d}d") }
    } else if h > 0 {
        if m > 0 { format!("{h}h{m}m") } else { format!("{h}h") }
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{s}s")
    }
}

fn cell<'a>(agent: &'a Value, key: &str) -> &'a str {
    agent[key].as_str().unwrap_or("")
}

/// The FOR cell: `idle 3h12m`.
pub(crate) fn for_cell(agent: &Value) -> String {
    let status = cell(agent, "agent_status");
    match agent["status_seconds"].as_u64() {
        Some(secs) => format!("{status} {}", format_duration(secs)),
        None => status.to_owned(),
    }
}

/// Text table for `klark agent list` on a terminal.
pub(crate) fn agent_table(agents: &[Value]) -> String {
    let header = ["NAME", "SESSION", "AGENT", "STATE", "FOR", "FOLDER"];
    let mut rows: Vec<[String; 6]> = vec![header.map(str::to_owned)];
    for agent in agents {
        let closed = agent["agent_status"] == "closed";
        let name = agent["name"]
            .as_str()
            .or_else(|| agent["pane_id"].as_str())
            .unwrap_or("-");
        let folder = ["foreground_cwd", "cwd", "folder"]
            .into_iter()
            .map(|k| cell(agent, k))
            .find(|v| !v.is_empty())
            .unwrap_or("-");
        let for_text = if closed {
            agent["status_seconds"]
                .as_u64()
                .map(format_duration)
                .unwrap_or_default()
        } else {
            for_cell(agent)
                .split_once(' ')
                .map(|(_, rest)| rest.to_owned())
                .unwrap_or_default()
        };
        rows.push([
            name.to_owned(),
            cell(agent, "session").to_owned(),
            cell(agent, "agent").to_owned(),
            cell(agent, "agent_status").to_owned(),
            for_text,
            folder.to_owned(),
        ]);
    }
    let mut widths = [0usize; 6];
    for row in &rows {
        for (i, c) in row.iter().enumerate() {
            widths[i] = widths[i].max(c.chars().count());
        }
    }
    let mut out = String::new();
    for row in &rows {
        let mut line = String::new();
        for (i, c) in row.iter().enumerate() {
            if i == 5 {
                line.push_str(c);
            } else {
                line.push_str(&format!("{c:<w$}  ", w = widths[i]));
            }
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

pub(crate) const LIST_STATUSES: &[&str] = &["idle", "working", "blocked", "done", "unknown", "closed"];

/// Whether an agent counts as idle for `--idle-for` and bulk close: waiting
/// for input (`idle`) or finished with an unseen result (`done`).
pub(crate) fn is_idle_like(agent: &Value) -> bool {
    matches!(agent["agent_status"].as_str(), Some("idle" | "done"))
}

/// `--status` and `--idle-for` for `agent list`.
pub(crate) fn matches_filter(agent: &Value, status: Option<&str>, idle_for: Option<u64>) -> bool {
    if let Some(status) = status {
        if agent["agent_status"].as_str() != Some(status) {
            return false;
        }
    }
    if let Some(min) = idle_for {
        if !is_idle_like(agent) || agent["status_seconds"].as_u64().unwrap_or(0) < min {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn durations_parse() {
        assert_eq!(parse_duration("90s"), Ok(90));
        assert_eq!(parse_duration("30m"), Ok(1800));
        assert_eq!(parse_duration("3h"), Ok(10800));
        assert_eq!(parse_duration("1d"), Ok(86400));
        assert_eq!(parse_duration("1h30m"), Ok(5400));
        for bad in ["", "h", "3", "3x", "-3h", "1.5h", "h3"] {
            assert!(parse_duration(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn durations_format() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(45), "45s");
        assert_eq!(format_duration(300), "5m");
        assert_eq!(format_duration(3 * 3600 + 12 * 60 + 5), "3h12m");
        assert_eq!(format_duration(3600), "1h");
        assert_eq!(format_duration(2 * 86400 + 4 * 3600), "2d4h");
    }

    #[test]
    fn filters_select_by_status_and_idle_time() {
        let idle3h = json!({"agent_status":"idle","status_seconds":10800});
        let idle5m = json!({"agent_status":"idle","status_seconds":300});
        let done3h = json!({"agent_status":"done","status_seconds":10800});
        let working = json!({"agent_status":"working","status_seconds":99999});
        let blocked = json!({"agent_status":"blocked","status_seconds":99999});
        assert!(matches_filter(&idle3h, None, None));
        assert!(matches_filter(&idle3h, Some("idle"), Some(10800)));
        assert!(!matches_filter(&idle5m, Some("idle"), Some(10800)));
        assert!(matches_filter(&done3h, None, Some(3600)));
        assert!(!matches_filter(&done3h, Some("idle"), None));
        assert!(!matches_filter(&working, None, Some(60)));
        assert!(!matches_filter(&blocked, None, Some(60)));
        assert!(matches_filter(&working, Some("working"), None));
    }

    #[test]
    fn table_has_a_for_column() {
        let agents = vec![json!({"name":"a","session":"s","agent":"claude",
            "agent_status":"idle","status_seconds":11520,"cwd":"/x"})];
        let table = agent_table(&agents);
        assert!(table.lines().next().unwrap().contains("FOR"));
        assert!(table.contains("3h12m"));
        assert_eq!(for_cell(&agents[0]), "idle 3h12m");
    }
}
