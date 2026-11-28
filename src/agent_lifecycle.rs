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

// ---------------------------------------------------------------------------
// Closed agents
// ---------------------------------------------------------------------------

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Closed-agent records older than this are dropped.
pub(crate) const CLOSED_MAX_AGE_SECS: u64 = 30 * 86400;

pub(crate) const REASON_USER: &str = "user";
pub(crate) const REASON_IDLE_CLOSE: &str = "idle-close";
pub(crate) const REASON_EXIT: &str = "exit";

/// One line of `agents-closed.jsonl`: enough to bring an agent back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ClosedRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub session: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// Agent kind label (`claude`, `devin`, ...).
    pub agent: String,
    /// The conversation / session id from report-agent-session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    /// `id` or `path`, as reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_kind: Option<String>,
    /// The reporting source (`klark:claude`), needed to build the resume command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub closed_at: u64,
    pub reason: String,
}

pub(crate) fn closed_log_path() -> PathBuf {
    crate::config::state_dir().join("agents-closed.jsonl")
}

/// Records inside the retention window, oldest first. Unreadable lines are skipped.
pub(crate) fn read_closed(path: &Path, now: u64) -> Vec<ClosedRecord> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str::<ClosedRecord>(line).ok())
        .filter(|r| now.saturating_sub(r.closed_at) <= CLOSED_MAX_AGE_SECS)
        .collect()
}

fn write_all(path: &Path, records: &[ClosedRecord]) -> std::io::Result<()> {
    let mut text = String::new();
    for r in records {
        text.push_str(&serde_json::to_string(r).map_err(std::io::Error::other)?);
        text.push('\n');
    }
    let tmp = path.with_extension("jsonl.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// Appends a record and prunes anything past the retention window.
pub(crate) fn append_closed(path: &Path, record: &ClosedRecord) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let total = existing.lines().filter(|l| !l.trim().is_empty()).count();
    let kept = read_closed(path, record.closed_at);
    if kept.len() == total {
        let mut line = serde_json::to_string(record).map_err(std::io::Error::other)?;
        line.push('\n');
        return std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?
            .write_all(line.as_bytes());
    }
    let mut kept = kept;
    kept.push(record.clone());
    write_all(path, &kept)
}

/// Drops one record (after it was resumed).
pub(crate) fn remove_closed(path: &Path, record: &ClosedRecord, now: u64) -> std::io::Result<()> {
    let mut records = read_closed(path, now);
    if let Some(i) = records.iter().position(|r| r == record) {
        records.remove(i);
        write_all(path, &records)?;
    }
    Ok(())
}

/// The newest record for `name`; `session` narrows it when given.
pub(crate) fn find_closed<'a>(
    records: &'a [ClosedRecord],
    name: &str,
    session: Option<&str>,
) -> Option<&'a ClosedRecord> {
    records
        .iter()
        .filter(|r| r.name.as_deref() == Some(name))
        .filter(|r| session.is_none_or(|s| r.session == s))
        .max_by_key(|r| r.closed_at)
}

/// A closed record as an `agent list` row (`agent_status: "closed"`).
pub(crate) fn closed_row(record: &ClosedRecord, now: u64) -> Value {
    let mut row = serde_json::json!({
        "agent": record.agent,
        "agent_status": "closed",
        "session": record.session,
        "status_since": record.closed_at,
        "status_seconds": now.saturating_sub(record.closed_at),
        "closed_at": record.closed_at,
        "closed_reason": record.reason,
    });
    let object = row.as_object_mut().unwrap();
    for (key, value) in [
        ("name", &record.name),
        ("workspace", &record.workspace),
        ("folder", &record.folder),
        ("conversation_id", &record.conversation_id),
    ] {
        if let Some(value) = value {
            object.insert(key.into(), value.clone().into());
        }
    }
    row
}

/// The command (after the executable) that resumes the conversation, or `None`
/// when the agent kind has no resume command or no id was recorded.
pub(crate) fn resume_args(record: &ClosedRecord) -> Option<Vec<String>> {
    let value = record.conversation_id.clone()?;
    let session_ref = match record.conversation_kind.as_deref() {
        Some("path") => crate::agent_resume::AgentSessionRef::path(value),
        _ => crate::agent_resume::AgentSessionRef::id(value),
    }?;
    let source = record
        .source
        .clone()
        .unwrap_or_else(|| format!("klark:{}", record.agent));
    let plan = crate::agent_resume::plan(&source, &record.agent, &session_ref)?;
    Some(plan.argv.into_iter().skip(1).collect())
}

// ---------------------------------------------------------------------------
// Pins
// ---------------------------------------------------------------------------

pub(crate) fn pin_key(session: &str, name: &str) -> String {
    format!("{session}/{name}")
}

pub(crate) fn pins_path() -> PathBuf {
    crate::config::state_dir().join("agents-pinned.json")
}

pub(crate) fn load_pins(path: &Path) -> HashSet<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<String>>(&text).ok())
        .map(|v| v.into_iter().collect())
        .unwrap_or_default()
}

pub(crate) fn save_pins(path: &Path, pins: &HashSet<String>) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut sorted: Vec<&String> = pins.iter().collect();
    sorted.sort();
    std::fs::write(path, serde_json::to_string_pretty(&sorted).map_err(std::io::Error::other)?)
}

// ---------------------------------------------------------------------------
// Bulk close selection
// ---------------------------------------------------------------------------

/// Agents (annotated `agent list` rows) a bulk close would close, and the
/// idle-for matches it skipped, each with why.
#[derive(Debug, Default)]
pub(crate) struct CloseSelection {
    pub close: Vec<Value>,
    pub skipped: Vec<(Value, &'static str)>,
}

/// `self_pane`: `(session, pane_id)` of the agent running the command.
pub(crate) fn select_closable(
    agents: &[Value],
    idle_for: u64,
    pins: &HashSet<String>,
    self_pane: Option<(&str, &str)>,
) -> CloseSelection {
    let mut out = CloseSelection::default();
    for agent in agents {
        let status = agent["agent_status"].as_str().unwrap_or("unknown");
        if status == "closed" {
            continue;
        }
        let quiet_long_enough = agent["status_seconds"].as_u64().unwrap_or(0) >= idle_for;
        if !quiet_long_enough {
            continue;
        }
        let session = agent["session"].as_str().unwrap_or("");
        let pane = agent["pane_id"].as_str().unwrap_or("");
        let reason = if self_pane.is_some_and(|(s, p)| s == session && p == pane) {
            Some("self")
        } else if !is_idle_like(agent) {
            match status {
                "working" => Some("working"),
                "blocked" => Some("blocked"),
                _ => Some("status unknown"),
            }
        } else if agent["name"]
            .as_str()
            .is_some_and(|name| pins.contains(&pin_key(session, name)))
        {
            Some("pinned")
        } else {
            None
        };
        match reason {
            Some(reason) => {
                // Only report skips that the idle filter alone would have matched
                // or that are about the caller; working/blocked agents past the
                // time threshold are reported so the user sees why.
                out.skipped.push((agent.clone(), reason));
            }
            None => out.close.push(agent.clone()),
        }
    }
    out
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

    fn rec(name: &str, agent: &str, at: u64) -> ClosedRecord {
        ClosedRecord {
            name: Some(name.into()),
            session: "s".into(),
            workspace: Some("ws".into()),
            folder: Some("/f".into()),
            agent: agent.into(),
            conversation_id: Some("abc-123".into()),
            conversation_kind: Some("id".into()),
            source: Some(format!("klark:{agent}")),
            closed_at: at,
            reason: REASON_USER.into(),
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("klark-lifecycle-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn closed_records_round_trip_and_prune() {
        let dir = tmp("closed");
        let path = dir.join("agents-closed.jsonl");
        let now = 100 * 86400;
        append_closed(&path, &rec("old", "claude", now - 31 * 86400)).unwrap();
        append_closed(&path, &rec("a", "claude", now - 10)).unwrap();
        // The append of "a" pruned "old" from the file itself.
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(!on_disk.contains("\"old\""), "{on_disk}");
        append_closed(&path, &rec("b", "devin", now)).unwrap();
        let read = read_closed(&path, now);
        assert_eq!(read.len(), 2);
        assert_eq!(read[1].name.as_deref(), Some("b"));
        assert_eq!(find_closed(&read, "a", None).unwrap().agent, "claude");
        assert!(find_closed(&read, "a", Some("other")).is_none());
        remove_closed(&path, &read[0], now).unwrap();
        let after = read_closed(&path, now);
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].name.as_deref(), Some("b"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_skips_bad_lines_and_missing_file() {
        let dir = tmp("bad");
        let path = dir.join("agents-closed.jsonl");
        assert!(read_closed(&path, 1).is_empty());
        std::fs::create_dir_all(&dir).unwrap();
        let good = serde_json::to_string(&rec("a", "claude", 5)).unwrap();
        std::fs::write(&path, format!("not json\n{good}\n")).unwrap();
        assert_eq!(read_closed(&path, 6).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn closed_row_looks_like_a_list_row() {
        let row = closed_row(&rec("a", "claude", 100), 160);
        assert_eq!(row["agent_status"], "closed");
        assert_eq!(row["status_seconds"], 60);
        assert_eq!(row["name"], "a");
        assert_eq!(row["folder"], "/f");
        let table = agent_table(&[row]);
        assert!(table.contains("closed"));
        assert!(table.contains("1m"));
        assert!(table.contains("/f"));
    }

    #[test]
    fn resume_command_per_agent_kind() {
        let args = |agent: &str| resume_args(&rec("a", agent, 1));
        assert_eq!(args("claude"), Some(vec!["--resume".into(), "abc-123".into()]));
        assert_eq!(args("devin"), Some(vec!["--resume".into(), "abc-123".into()]));
        assert_eq!(args("codex"), Some(vec!["resume".into(), "abc-123".into()]));
        assert_eq!(args("copilot"), Some(vec!["--resume=abc-123".into()]));
        assert_eq!(args("kimi"), Some(vec!["--session".into(), "abc-123".into()]));
        // No resume command for an unknown kind, none without an id.
        assert_eq!(args("nonesuch"), None);
        let mut no_id = rec("a", "claude", 1);
        no_id.conversation_id = None;
        assert_eq!(resume_args(&no_id), None);
    }

    #[test]
    fn pins_persist_by_session_and_name() {
        let dir = tmp("pins");
        let path = dir.join("agents-pinned.json");
        assert!(load_pins(&path).is_empty());
        let mut pins = HashSet::new();
        pins.insert(pin_key("main", "personalstocks"));
        save_pins(&path, &pins).unwrap();
        assert!(load_pins(&path).contains("main/personalstocks"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn row(name: &str, status: &str, secs: u64) -> Value {
        json!({"name": name, "session": "main", "pane_id": format!("p_{name}"),
               "agent_status": status, "status_seconds": secs})
    }

    #[test]
    fn bulk_close_excludes_working_blocked_pinned_and_self() {
        let agents = vec![
            row("idle-old", "idle", 4 * 3600),
            row("done-old", "done", 4 * 3600),
            row("idle-new", "idle", 60),
            row("working", "working", 9 * 3600),
            row("blocked", "blocked", 9 * 3600),
            row("pinned", "idle", 9 * 3600),
            row("me", "idle", 9 * 3600),
        ];
        let mut pins = HashSet::new();
        pins.insert(pin_key("main", "pinned"));
        let sel = select_closable(&agents, 3 * 3600, &pins, Some(("main", "p_me")));
        let names = |v: &[Value]| v.iter().map(|a| a["name"].as_str().unwrap().to_owned()).collect::<Vec<_>>();
        assert_eq!(names(&sel.close), ["idle-old", "done-old"]);
        let skipped: Vec<(String, &str)> = sel
            .skipped
            .iter()
            .map(|(a, why)| (a["name"].as_str().unwrap().to_owned(), *why))
            .collect();
        assert_eq!(
            skipped,
            [
                ("working".to_owned(), "working"),
                ("blocked".to_owned(), "blocked"),
                ("pinned".to_owned(), "pinned"),
                ("me".to_owned(), "self"),
            ]
        );
        // Same pane id in another session is not "self".
        let other = select_closable(&agents[..1], 0, &HashSet::new(), Some(("other", "p_idle-old")));
        assert_eq!(other.close.len(), 1);
    }
}
