//! Hidden `klark transcript --pane <id> [--after C] [--before C] [--limit N] --json`.
//!
//! Answers the hub's `get_transcript` (protocol v3): the pane agent's
//! conversation as structured items, read from the agent's own session log on
//! this machine. The hub cannot read local files, so `klark expose` runs this.
//!
//! Structured logs: Claude Code JSONL (`~/.claude/projects/<cwd-slug>/
//! <session-id>.jsonl`, `source:"jsonl"`) and Devin CLI's SQLite store
//! (`~/.local/share/devin/cli/sessions.db`, `source:"db"`, see `devin.rs`;
//! its cursors are node ids on the session's main chain). Every other agent,
//! or no log found, falls back to the screen (`pane.read recent_unwrapped`)
//! as one assistant text item.
//!
//! JSONL cursors are decimal byte offsets into the log: an item's `cursor` is the
//! offset of the JSONL line it came from, the result's `cursor` is the end of
//! the last complete line read. They stay valid as the log grows (it is
//! append-only). Only the tail is ever read: windows grow backwards from the
//! end until enough items are found. Polling with `after` = the last cursor on
//! an unchanged log reads zero bytes, so no extra cache is kept.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

mod devin;

pub const DEFAULT_LIMIT: usize = 200;
pub const MAX_LIMIT: usize = 1000;
const TOOL_RESULT_CAP: usize = 4 * 1024;
const FIRST_WINDOW: u64 = 256 * 1024;
/// `after` reads at most this much; beyond it the reply is the tail.
const MAX_FORWARD: u64 = 16 * 1024 * 1024;

const USAGE: &str =
    "usage: klark transcript --pane <pane-id> [--after <cursor>] [--before <cursor>] [--limit <n>] --json";

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LogFormat {
    ClaudeJsonl,
}

/// One JSONL line's items, with the line's byte offset.
type Group = (u64, Vec<Value>);

#[derive(Debug, PartialEq)]
pub struct Page {
    pub items: Vec<Value>,
    pub cursor: u64,
    pub has_more: bool,
}

pub enum Query {
    Tail,
    After(u64),
    Before(u64),
}

// ------------------------------------------------------------------ parsing

fn truncate(s: &str, cap: usize) -> String {
    if s.len() <= cap {
        return s.to_string();
    }
    let mut end = cap;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… [{} bytes truncated]", &s[..end], s.len() - end)
}

fn result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| match p.get("type").and_then(Value::as_str) {
                Some("text") => p.get("text").and_then(Value::as_str).map(str::to_string),
                Some("image") => Some("[image]".into()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Items from one Claude Code JSONL line. Ids are the line's `uuid` (with
/// `#n` for the n-th content block after the first), so they are stable.
pub fn claude_items(line: &str, offset: u64) -> Vec<Value> {
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return vec![];
    };
    let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
    if !matches!(ty, "user" | "assistant") || v.get("isMeta").and_then(Value::as_bool) == Some(true)
    {
        return vec![];
    }
    let Some(msg) = v.get("message") else {
        return vec![];
    };
    let role = msg.get("role").and_then(Value::as_str).unwrap_or(ty);
    let uuid = v
        .get("uuid")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("L{offset}"));
    let ts = v.get("timestamp").cloned();
    let cursor = offset.to_string();
    let mut out = Vec::new();
    let mut push = |mut item: Value| {
        let id = if out.is_empty() {
            uuid.clone()
        } else {
            format!("{uuid}#{}", out.len())
        };
        item["id"] = json!(id);
        item["cursor"] = json!(cursor);
        if let Some(ts) = &ts {
            item["ts"] = ts.clone();
        }
        out.push(item);
    };
    match msg.get("content") {
        Some(Value::String(s)) if !s.trim().is_empty() => {
            push(json!({"role": role, "kind": "text", "text": s}));
        }
        Some(Value::Array(blocks)) => {
            for b in blocks {
                match b.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        let t = b.get("text").and_then(Value::as_str).unwrap_or("");
                        if !t.trim().is_empty() {
                            push(json!({"role": role, "kind": "text", "text": t}));
                        }
                    }
                    Some("thinking") => {
                        let t = b.get("thinking").and_then(Value::as_str).unwrap_or("");
                        if !t.trim().is_empty() {
                            push(json!({"role": "assistant", "kind": "thinking", "text": t}));
                        }
                    }
                    Some("tool_use") => push(json!({
                        "role": "assistant",
                        "kind": "tool_use",
                        "tool": b.get("name").cloned().unwrap_or(Value::Null),
                        "tool_use_id": b.get("id").cloned().unwrap_or(Value::Null),
                        "input": b.get("input").cloned().unwrap_or(Value::Null),
                    })),
                    Some("tool_result") => push(json!({
                        "role": "tool",
                        "kind": "tool_result",
                        "tool_use_id": b.get("tool_use_id").cloned().unwrap_or(Value::Null),
                        "text": truncate(&result_text(b.get("content").unwrap_or(&Value::Null)), TOOL_RESULT_CAP),
                        "ok": b.get("is_error").and_then(Value::as_bool) != Some(true),
                    })),
                    _ => {}
                }
            }
        }
        _ => {}
    }
    out
}

fn parse_line(format: LogFormat, line: &str, offset: u64) -> Vec<Value> {
    match format {
        LogFormat::ClaudeJsonl => claude_items(line, offset),
    }
}

// ------------------------------------------------------------------ reading

fn read_range(f: &mut File, start: u64, end: u64) -> std::io::Result<Vec<u8>> {
    let mut buf = vec![0u8; (end - start) as usize];
    f.seek(SeekFrom::Start(start))?;
    f.read_exact(&mut buf)?;
    Ok(buf)
}

/// End of the last complete (newline-terminated) line: a line still being
/// written is never read.
fn complete_end(f: &mut File, len: u64) -> std::io::Result<u64> {
    let mut end = len;
    let mut window = 64 * 1024u64;
    loop {
        let start = end.saturating_sub(window);
        let buf = read_range(f, start, end)?;
        if let Some(i) = buf.iter().rposition(|&b| b == b'\n') {
            return Ok(start + i as u64 + 1);
        }
        if start == 0 {
            return Ok(0);
        }
        end = start;
        window *= 4;
    }
}

/// Groups for complete lines in `buf` (which starts at file offset `base`).
/// `skip_first` drops a leading partial line.
fn groups_in(format: LogFormat, buf: &[u8], base: u64, skip_first: bool) -> Vec<Group> {
    let mut groups = Vec::new();
    let mut pos = 0usize;
    if skip_first {
        match buf.iter().position(|&b| b == b'\n') {
            Some(i) => pos = i + 1,
            None => return groups,
        }
    }
    while pos < buf.len() {
        let end = buf[pos..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(buf.len(), |i| pos + i);
        let line = String::from_utf8_lossy(&buf[pos..end]);
        let items = parse_line(format, line.trim(), base + pos as u64);
        if !items.is_empty() {
            groups.push((base + pos as u64, items));
        }
        pos = end + 1;
    }
    groups
}

/// Newest whole groups whose item count fits `limit` (at least one group).
fn take_newest(mut groups: Vec<Group>, limit: usize, older_exist: bool) -> (Vec<Value>, bool) {
    let mut kept: Vec<Group> = Vec::new();
    let mut n = 0;
    while let Some(g) = groups.pop() {
        if !kept.is_empty() && n + g.1.len() > limit {
            groups.push(g);
            break;
        }
        n += g.1.len();
        kept.push(g);
    }
    kept.reverse();
    let has_more = older_exist || !groups.is_empty();
    (kept.into_iter().flat_map(|g| g.1).collect(), has_more)
}

/// Items before byte offset `end`, reading backwards in growing windows.
fn page_before(
    f: &mut File,
    format: LogFormat,
    end: u64,
    limit: usize,
) -> std::io::Result<(Vec<Value>, bool)> {
    let mut window = FIRST_WINDOW;
    loop {
        let start = end.saturating_sub(window);
        let buf = read_range(f, start, end)?;
        let groups = groups_in(format, &buf, start, start > 0);
        let count: usize = groups.iter().map(|g| g.1.len()).sum();
        if count > limit || start == 0 {
            return Ok(take_newest(groups, limit, start > 0));
        }
        window *= 4;
    }
}

pub fn read_page(
    path: &Path,
    format: LogFormat,
    query: Query,
    limit: usize,
) -> std::io::Result<Page> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let end = complete_end(&mut f, len)?;
    let (items, has_more) = match query {
        Query::After(c) if c <= end && end - c <= MAX_FORWARD => {
            let buf = read_range(&mut f, c, end)?;
            // A cursor is always a line start, so nothing is skipped.
            let groups = groups_in(format, &buf, c, false);
            let (items, more) = take_newest(groups, limit, false);
            (items, more)
        }
        Query::Before(c) => page_before(&mut f, format, c.min(end), limit)?,
        // Tail, or an `after` past the end (log replaced) or too far back.
        _ => page_before(&mut f, format, end, limit)?,
    };
    Ok(Page {
        items,
        cursor: end,
        has_more,
    })
}

// ------------------------------------------------------------------ locating logs

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Claude Code's projects dir (honours CLAUDE_CONFIG_DIR).
pub fn claude_projects_root() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".claude")))
        .map(|d| d.join("projects"))
}

/// Claude Code's per-cwd directory name: every char outside [A-Za-z0-9-]
/// becomes `-` (`/Users/me/x.y` -> `-Users-me-x-y`).
pub fn claude_slug(cwd: &str) -> String {
    cwd.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// The canonical path, only if it stays under `root` (no `..` or symlink
/// escape out of the agent's log root).
pub fn scoped(root: &Path, path: &Path) -> Option<PathBuf> {
    let root = std::fs::canonicalize(root).ok()?;
    let p = std::fs::canonicalize(path).ok()?;
    (p.starts_with(&root) && p.is_file()).then_some(p)
}

fn newest_jsonl(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .max()
        .map(|(_, p)| p)
}

/// The Claude Code log for a session id (or path) and cwd.
pub fn claude_log(
    root: &Path,
    session: Option<(&str, &str)>,
    cwd: Option<&str>,
) -> Option<PathBuf> {
    let slug_dir = cwd.map(|c| root.join(claude_slug(c)));
    let candidate = match session {
        Some(("path", value)) => Some(PathBuf::from(value)),
        Some((_, id)) if !id.contains(['/', '\\']) && !id.contains("..") => {
            let name = format!("{id}.jsonl");
            slug_dir
                .as_ref()
                .map(|d| d.join(&name))
                .filter(|p| p.is_file())
                .or_else(|| {
                    // cwd moved since the session started: look one level down.
                    std::fs::read_dir(root)
                        .ok()?
                        .flatten()
                        .map(|e| e.path().join(&name))
                        .find(|p| p.is_file())
                })
        }
        _ => None,
    }
    .or_else(|| slug_dir.as_deref().and_then(newest_jsonl))?;
    scoped(root, &candidate)
}

/// Where a pane's structured transcript lives.
enum Log {
    File(PathBuf, LogFormat),
    /// Devin's SQLite store and the session id in it.
    Devin(PathBuf, String),
}

/// (agent, log) for a pane, from `pane.get`.
fn structured_log(pane: &Value) -> (Option<String>, Option<Log>) {
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let session = pane.get("agent_session");
    let agent = s(pane, "agent")
        .or_else(|| session.and_then(|x| s(x, "agent")))
        .or_else(|| s(pane, "display_agent"));
    let cwd = s(pane, "foreground_cwd").or_else(|| s(pane, "cwd"));
    let log = match agent.as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("claude") | Some("claude-code") | Some("claude_code") => {
            let sref = session.and_then(|x| Some((s(x, "kind")?, s(x, "value")?)));
            claude_projects_root()
                .and_then(|root| {
                    claude_log(
                        &root,
                        sref.as_ref().map(|(k, v)| (k.as_str(), v.as_str())),
                        cwd.as_deref(),
                    )
                })
                .map(|p| Log::File(p, LogFormat::ClaudeJsonl))
        }
        Some("devin") | Some("devin-cli") => {
            let id = session.and_then(|x| s(x, "value"));
            devin::db_path().and_then(|db| {
                let sid = devin::find_session(&db, id.as_deref(), cwd.as_deref())?;
                Some(Log::Devin(db, sid))
            })
        }
        // Extension point: add other agents' log readers here.
        _ => None,
    };
    (agent, log)
}

// ------------------------------------------------------------------ command

struct Opts {
    pane: String,
    after: Option<String>,
    before: Option<String>,
    limit: usize,
}

fn parse(args: &[String]) -> Result<Opts, String> {
    let (mut pane, mut after, mut before, mut limit) = (None, None, None, DEFAULT_LIMIT);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" => {}
            "--pane" | "--after" | "--before" | "--limit" => {
                let v = it.next().ok_or(format!("{a} needs a value"))?.clone();
                match a.as_str() {
                    "--pane" => pane = Some(v),
                    "--after" => after = Some(v),
                    "--before" => before = Some(v),
                    _ => limit = v.parse().map_err(|_| format!("bad --limit {v}"))?,
                }
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Opts {
        pane: pane.ok_or("--pane is required")?,
        after,
        before,
        limit: limit.clamp(1, MAX_LIMIT),
    })
}

fn cursor(s: &Option<String>) -> Option<u64> {
    s.as_deref().and_then(|c| c.trim().parse().ok())
}

fn transcript(o: Opts) -> Result<Value, String> {
    let bin = std::env::current_exe().unwrap_or_else(|_| "klark".into());
    let up = klark_link::upstream::Upstream::new(crate::api::socket_path(), bin);
    let info = up.call("pane.get", json!({"pane_id": o.pane}))?;
    let pane = info.get("pane").cloned().unwrap_or(info);
    let (agent, log) = structured_log(&pane);
    let query = match (cursor(&o.after), cursor(&o.before)) {
        (_, Some(b)) => Query::Before(b),
        (Some(a), None) => Query::After(a),
        _ => Query::Tail,
    };
    let structured = match log {
        Some(Log::File(path, format)) => Some(
            read_page(&path, format, query, o.limit)
                .map(|p| (p, "jsonl"))
                .map_err(|e| format!("{}: {e}", path.display()))?,
        ),
        // A locked or unreadable store falls back to the screen.
        Some(Log::Devin(db, sid)) => devin::read_page(&db, &sid, query, o.limit)
            .ok()
            .filter(|p| {
                !p.items.is_empty() || cursor(&o.after).is_some() || cursor(&o.before).is_some()
            })
            .map(|p| (p, "db")),
        None => None,
    };
    if let Some((page, source)) = structured {
        return Ok(json!({
            "agent": agent,
            "cursor": page.cursor.to_string(),
            "items": page.items,
            "has_more": page.has_more,
            "source": source,
        }));
    }
    let read = up.call(
        "pane.read",
        // ANSI kept: the hub's generic transformer shows command output in its colours;
        // the Claude and Devin transformers strip it (ADR 0011, F).
        json!({"pane_id": o.pane, "source": "recent_unwrapped", "format": "ansi"}),
    )?;
    let text = read
        .get("read")
        .and_then(|r| r.get("text"))
        .and_then(Value::as_str)
        .unwrap_or("");
    Ok(json!({
        "agent": agent,
        "cursor": "",
        "items": [{"id": "screen", "role": "assistant", "kind": "text", "text": text}],
        "has_more": false,
        "source": "screen",
    }))
}

pub fn run(args: &[String]) -> ! {
    let o = match parse(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("klark transcript: {e}\n{USAGE}");
            std::process::exit(2);
        }
    };
    match transcript(o) {
        Ok(v) => {
            println!("{v}");
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("klark transcript: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{"type":"summary","summary":"x"}
{"type":"user","uuid":"u1","timestamp":"2026-11-06T10:00:00Z","message":{"role":"user","content":"fix the bug"}}
{"type":"assistant","uuid":"a1","timestamp":"2026-11-06T10:00:01Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"look at main"},{"type":"text","text":"Reading main.rs"},{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/x/main.rs"}}]}}
{"type":"user","uuid":"u2","isMeta":true,"message":{"role":"user","content":"<meta>"}}
{"type":"user","uuid":"u3","timestamp":"2026-11-06T10:00:02Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"BIGRESULT"}],"is_error":false}]}}
{"type":"assistant","uuid":"a2","timestamp":"2026-11-06T10:00:03Z","message":{"role":"assistant","content":[{"type":"text","text":"Done."}]}}
"#;

    fn fixture(tag: &str, body: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "klark-transcript-{tag}-{}.jsonl",
            std::process::id()
        ));
        std::fs::write(&p, body).unwrap();
        p
    }

    fn ids(items: &[Value]) -> Vec<&str> {
        items.iter().map(|i| i["id"].as_str().unwrap()).collect()
    }

    #[test]
    fn parses_claude_blocks() {
        let body = FIXTURE.replace("BIGRESULT", &"x".repeat(10_000));
        let p = fixture("parse", &body);
        let page = read_page(&p, LogFormat::ClaudeJsonl, Query::Tail, 200).unwrap();
        assert_eq!(ids(&page.items), ["u1", "a1", "a1#1", "a1#2", "u3", "a2"]);
        let it = &page.items;
        assert_eq!(it[0]["role"], "user");
        assert_eq!(it[0]["text"], "fix the bug");
        assert_eq!(it[1]["kind"], "thinking");
        assert_eq!(it[3]["kind"], "tool_use");
        assert_eq!(it[3]["tool"], "Read");
        assert_eq!(it[3]["input"]["file_path"], "/x/main.rs");
        assert_eq!(it[4]["role"], "tool");
        assert_eq!(it[4]["ok"], true);
        assert!(it[4]["text"].as_str().unwrap().len() < 4200);
        assert_eq!(it[5]["ts"], "2026-11-06T10:00:03Z");
        assert!(!page.has_more);
        assert_eq!(page.cursor, body.len() as u64);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn after_returns_only_newer_and_ignores_a_partial_line() {
        let p = fixture("after", FIXTURE);
        let first = read_page(&p, LogFormat::ClaudeJsonl, Query::Tail, 200).unwrap();
        // Nothing new.
        let none = read_page(&p, LogFormat::ClaudeJsonl, Query::After(first.cursor), 200).unwrap();
        assert!(none.items.is_empty());
        assert_eq!(none.cursor, first.cursor);
        // Append one complete line and one still being written.
        let mut body = FIXTURE.to_string();
        body.push_str(r#"{"type":"user","uuid":"u9","message":{"role":"user","content":"next"}}"#);
        body.push('\n');
        body.push_str(r#"{"type":"assistant","uuid":"a9","mess"#);
        std::fs::write(&p, &body).unwrap();
        let next = read_page(&p, LogFormat::ClaudeJsonl, Query::After(first.cursor), 200).unwrap();
        assert_eq!(ids(&next.items), ["u9"]);
        assert_eq!(next.items[0]["cursor"], first.cursor.to_string());
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn before_pages_back_with_stable_cursors() {
        let p = fixture("before", FIXTURE);
        // limit 2: newest whole lines that fit -> u3, a2.
        let page = read_page(&p, LogFormat::ClaudeJsonl, Query::Tail, 2).unwrap();
        assert_eq!(ids(&page.items), ["u3", "a2"]);
        assert!(page.has_more);
        let oldest: u64 = page.items[0]["cursor"].as_str().unwrap().parse().unwrap();
        // a1 has 3 items > limit 2, but a page always holds at least one line.
        let older = read_page(&p, LogFormat::ClaudeJsonl, Query::Before(oldest), 2).unwrap();
        assert_eq!(ids(&older.items), ["a1", "a1#1", "a1#2"]);
        assert!(older.has_more);
        let c: u64 = older.items[0]["cursor"].as_str().unwrap().parse().unwrap();
        let oldest_page = read_page(&p, LogFormat::ClaudeJsonl, Query::Before(c), 2).unwrap();
        assert_eq!(ids(&oldest_page.items), ["u1"]);
        assert!(!oldest_page.has_more);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn large_log_reads_only_needed_tail() {
        let line =
            r#"{"type":"assistant","uuid":"aN","message":{"role":"assistant","content":"PAD"}}"#
                .replace("PAD", &"y".repeat(1000));
        let mut body = String::new();
        for i in 0..3000 {
            body.push_str(&line.replace("aN", &format!("a{i}")));
            body.push('\n');
        }
        let p = fixture("big", &body);
        let page = read_page(&p, LogFormat::ClaudeJsonl, Query::Tail, 200).unwrap();
        assert_eq!(page.items.len(), 200);
        assert_eq!(page.items[199]["id"], "a2999");
        assert_eq!(page.items[0]["id"], "a2800");
        assert!(page.has_more);
        let _ = std::fs::remove_file(p);
    }

    #[test]
    fn slug_and_scope() {
        assert_eq!(claude_slug("/Users/me/x.y_z"), "-Users-me-x-y-z");
        let root = std::env::temp_dir().join(format!("klark-claude-root-{}", std::process::id()));
        let proj = root.join(claude_slug("/w/app"));
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("s1.jsonl"), FIXTURE).unwrap();
        let outside =
            std::env::temp_dir().join(format!("klark-outside-{}.jsonl", std::process::id()));
        std::fs::write(&outside, FIXTURE).unwrap();
        assert!(claude_log(&root, Some(("id", "s1")), Some("/w/app")).is_some());
        // Found by id even when the cwd moved.
        assert!(claude_log(&root, Some(("id", "s1")), Some("/elsewhere")).is_some());
        // No session id: newest log for the cwd.
        assert!(claude_log(&root, None, Some("/w/app")).is_some());
        assert!(claude_log(&root, Some(("id", "../../etc/passwd")), None).is_none());
        assert!(claude_log(&root, Some(("path", outside.to_str().unwrap())), None).is_none());
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_file(outside);
    }
}
