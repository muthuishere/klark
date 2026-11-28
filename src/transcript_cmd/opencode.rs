//! OpenCode transcripts, read from its live SQLite store
//! (`~/.local/share/opencode/opencode.db`, OpenCode 2.x).
//!
//! OpenCode 2 keeps one row per conversation entry in `session_message`
//! (`session_id`, `type`, `seq`, `data` JSON), numbered by `seq` within a
//! session; sessions (with their directory) are in `session_v2`. The shapes are
//! the `SessionMessage*` types of OpenCode's client (packages/client, generated
//! types.ts): `user` {text}, `assistant` {content: text | reasoning | tool},
//! `shell` {command, output}, `idle` {outcome, error}, `compaction`, and entries
//! that are not conversation (system, synthetic, agent/model switches).
//! Cursors are `seq` values. The file is opened read-only.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde_json::{json, Value};

use super::{take_newest, truncate, Group, Page, Query, MAX_LIMIT, TOOL_RESULT_CAP};

/// Tool-call inputs longer than this (as JSON) are sent as a truncated string.
const INPUT_CAP: usize = 1024;

/// OpenCode's store, honouring XDG_DATA_HOME.
pub fn db_path() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| super::home().map(|h| h.join(".local/share")))
        .map(|d| d.join("opencode/opencode.db"))
}

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_millis(200))?;
    Ok(conn)
}

/// The session to read: the id the klark plugin reported for the pane if the
/// store has it, else the most recently updated top-level session in `cwd`.
pub fn find_session(db: &Path, id: Option<&str>, cwd: Option<&str>) -> Option<String> {
    if !db.is_file() {
        return None;
    }
    let conn = open(db).ok()?;
    if let Some(id) = id {
        let hit: Option<String> = conn
            .query_row("SELECT id FROM session_v2 WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()
            .ok()?;
        if hit.is_some() {
            return hit;
        }
    }
    conn.query_row(
        "SELECT id FROM session_v2 WHERE directory = ?1 AND parent_id IS NULL \
         AND time_archived IS NULL ORDER BY time_updated DESC LIMIT 1",
        [cwd?],
        |r| r.get(0),
    )
    .optional()
    .ok()?
}

fn ts_of(ms: Option<i64>) -> Option<Value> {
    let ms = ms?;
    let secs = if ms > 100_000_000_000 { ms / 1000 } else { ms };
    time::OffsetDateTime::from_unix_timestamp(secs)
        .ok()?
        .format(&time::format_description::well_known::Rfc3339)
        .ok()
        .map(Value::String)
}

/// A long input keeps its shape with each long string cut, so the hub can still
/// summarise the call by its path or command (a write's content is what is long).
fn short_input(v: &Value) -> Value {
    if v.to_string().len() <= INPUT_CAP {
        return v.clone();
    }
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, x)| {
                    let x = match x {
                        Value::String(s) if s.len() > INPUT_CAP / 4 => {
                            json!(truncate(s, INPUT_CAP / 4))
                        }
                        Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null => {
                            x.clone()
                        }
                        other => json!(truncate(&other.to_string(), INPUT_CAP / 4)),
                    };
                    (k.clone(), x)
                })
                .collect(),
        ),
        other => json!(truncate(&other.to_string(), INPUT_CAP)),
    }
}

/// A tool's output: its text parts, a file part as its name.
fn tool_output(state: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    for c in state
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match c.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(t) = c.get("text").and_then(Value::as_str) {
                    parts.push(t.to_string());
                }
            }
            Some("file") => {
                let name = c
                    .get("name")
                    .or_else(|| c.get("filename"))
                    .or_else(|| c.get("path"))
                    .and_then(Value::as_str)
                    .unwrap_or("file");
                parts.push(format!("[{name}]"));
            }
            _ => {}
        }
    }
    if parts.is_empty() {
        if let Some(e) = state.get("error") {
            parts.push(error_text(e));
        }
    }
    parts.join("\n")
}

fn error_text(e: &Value) -> String {
    match e {
        Value::String(s) => s.clone(),
        _ => e
            .get("message")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| e.to_string()),
    }
}

/// OpenCode's own tool names (packages/tui/src/util/tool-display.ts canonicalToolName).
fn canonical_tool(name: &str) -> &str {
    match name {
        "bash" => "shell",
        "task" => "subagent",
        "apply_patch" => "patch",
        other => other,
    }
}

/// A patch's input is the whole patch text; give it the first file it touches as
/// `path` so the hub can summarise the call by that file.
fn patch_path(input: &mut Value) {
    let Some(obj) = input.as_object_mut() else {
        return;
    };
    if obj.contains_key("path") {
        return;
    }
    let file = obj.get("patchText").and_then(Value::as_str).and_then(|p| {
        p.lines().find_map(|l| {
            ["*** Update File: ", "*** Add File: ", "*** Delete File: "]
                .iter()
                .find_map(|pre| l.strip_prefix(pre))
                .map(|f| f.trim().to_string())
        })
    });
    if let Some(f) = file {
        obj.insert("path".into(), json!(f));
    }
}

/// Items from one `session_message` row. Ids are `opencode:<seq>` (with `#n`
/// for the n-th item after the first).
pub fn opencode_items(ty: &str, data: &str, seq: i64, created_ms: Option<i64>) -> Vec<Value> {
    let Ok(msg) = serde_json::from_str::<Value>(data) else {
        return vec![];
    };
    let ts = ts_of(
        msg.pointer("/time/created")
            .and_then(Value::as_i64)
            .or(created_ms),
    );
    let base = format!("opencode:{seq}");
    let cursor = seq.to_string();
    let mut out: Vec<Value> = Vec::new();
    let mut push = |mut item: Value| {
        let id = if out.is_empty() {
            base.clone()
        } else {
            format!("{base}#{}", out.len())
        };
        item["id"] = json!(id);
        item["cursor"] = json!(cursor);
        if let Some(ts) = &ts {
            item["ts"] = ts.clone();
        }
        out.push(item);
    };
    let text = |k: &str| msg.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    match ty {
        "user" => {
            let mut t = text("text");
            for f in msg
                .get("files")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(n) = f.get("name").and_then(Value::as_str) {
                    t.push_str(&format!("\n[{n}]"));
                }
            }
            if !t.trim().is_empty() {
                push(json!({"role": "user", "kind": "text", "text": t}));
            }
        }
        "assistant" => {
            for part in msg
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                match part.get("type").and_then(Value::as_str) {
                    // As OpenCode's TUI does (packages/tui/src/routes/session/message-parts.tsx):
                    // text is trimmed; reasoning loses OpenRouter's "[REDACTED]" placeholder.
                    Some("text") => {
                        let t = part
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .trim();
                        if !t.is_empty() {
                            push(json!({"role": "assistant", "kind": "text", "text": t}));
                        }
                    }
                    Some("reasoning") => {
                        let t = part.get("text").and_then(Value::as_str).unwrap_or("");
                        let t = t.replacen("[REDACTED]", "", 1);
                        let t = t.trim();
                        if !t.is_empty() {
                            push(json!({"role": "assistant", "kind": "thinking", "text": t}));
                        }
                    }
                    Some("tool") => {
                        let state = part.get("state").cloned().unwrap_or(Value::Null);
                        let id = part.get("id").cloned().unwrap_or(Value::Null);
                        let name = part.get("name").and_then(Value::as_str).unwrap_or("");
                        let mut input = state.get("input").cloned().unwrap_or(Value::Null);
                        patch_path(&mut input);
                        push(json!({
                            "role": "assistant",
                            "kind": "tool_use",
                            "tool": canonical_tool(name),
                            "tool_use_id": id,
                            "input": short_input(&input),
                        }));
                        let status = state.get("status").and_then(Value::as_str).unwrap_or("");
                        if matches!(status, "completed" | "error") {
                            push(json!({
                                "role": "tool",
                                "kind": "tool_result",
                                "tool_use_id": id,
                                "text": truncate(&tool_output(&state), TOOL_RESULT_CAP),
                                "ok": status == "completed",
                            }));
                        }
                    }
                    _ => {}
                }
            }
            // A failed turn's error is also on the "idle" entry that follows: shown once, from there.
        }
        // "!command" typed in OpenCode: the person's command and its output.
        "shell" => {
            let cmd = text("command");
            if !cmd.trim().is_empty() {
                push(json!({"role": "user", "kind": "text", "text": format!("!{cmd}")}));
                let output = msg
                    .pointer("/output/output")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let status = msg.get("status").and_then(Value::as_str).unwrap_or("");
                if !output.is_empty() || status != "running" {
                    push(json!({
                        "role": "tool",
                        "kind": "tool_result",
                        "tool_use_id": Value::Null,
                        "text": truncate(output, TOOL_RESULT_CAP),
                        "ok": status == "exited" && msg.get("exit").and_then(Value::as_i64).unwrap_or(0) == 0,
                    }));
                }
            }
        }
        // A turn that ended in an error (out of credit, provider down): what the TUI shows in red.
        "idle" => {
            if msg.get("outcome").and_then(Value::as_str) == Some("failed") {
                if let Some(e) = msg.get("error") {
                    push(
                        json!({"role": "assistant", "kind": "text", "text": format!("Error: {}", error_text(e))}),
                    );
                }
            }
        }
        "compaction" if msg.get("status").and_then(Value::as_str) == Some("completed") => {
            push(json!({"role": "assistant", "kind": "text", "text": "_Conversation compacted._"}));
        }
        // system, synthetic, agent-switched, model-switched, ...: not conversation.
        _ => {}
    }
    out
}

/// Up to `n` rows with `seq < upper` (all when None), oldest first, as groups;
/// the bool is true when older rows exist.
fn rows_before(
    conn: &Connection,
    session: &str,
    upper: Option<i64>,
    n: usize,
) -> rusqlite::Result<(Vec<Group>, bool)> {
    let mut st = conn.prepare_cached(
        "SELECT seq, type, data, time_created FROM session_message \
         WHERE session_id = ?1 AND seq < ?2 ORDER BY seq DESC LIMIT ?3",
    )?;
    let rows: Vec<(i64, String, String, Option<i64>)> = st
        .query_map(
            params![session, upper.unwrap_or(i64::MAX), (n + 1) as i64],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3).ok())),
        )?
        .collect::<rusqlite::Result<_>>()?;
    let more = rows.len() > n;
    let groups = rows
        .into_iter()
        .take(n)
        .rev()
        .map(|(seq, ty, data, t)| (seq as u64, opencode_items(&ty, &data, seq, t)))
        .filter(|g| !g.1.is_empty())
        .collect();
    Ok((groups, more))
}

/// Newest items with `seq < upper`: many rows yield no item, so widen until
/// enough are found or the first row is reached.
fn page_before(
    conn: &Connection,
    session: &str,
    upper: Option<i64>,
    limit: usize,
) -> rusqlite::Result<(Vec<Value>, bool)> {
    let mut n = limit + 1;
    loop {
        let (g, more) = rows_before(conn, session, upper, n)?;
        let count: usize = g.iter().map(|g| g.1.len()).sum();
        if count > limit || !more || n >= 1_000_000 {
            return Ok(take_newest(g, limit, more));
        }
        n *= 4;
    }
}

pub fn read_page(db: &Path, session: &str, query: Query, limit: usize) -> rusqlite::Result<Page> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let conn = open(db)?;
    let last: i64 = conn.query_row(
        "SELECT COALESCE(MAX(seq), -1) FROM session_message WHERE session_id = ?1",
        [session],
        |r| r.get(0),
    )?;
    let (items, has_more) = match query {
        Query::After(c) if (c as i64) <= last => {
            let mut st = conn.prepare_cached(
                "SELECT seq, type, data, time_created FROM session_message \
                 WHERE session_id = ?1 AND seq > ?2 ORDER BY seq",
            )?;
            let groups: Vec<Group> = st
                .query_map(params![session, c as i64], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get(3).ok(),
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
                .into_iter()
                .map(|(seq, ty, data, t)| (seq as u64, opencode_items(&ty, &data, seq, t)))
                .filter(|g| !g.1.is_empty())
                .collect();
            take_newest(groups, limit, false)
        }
        Query::Before(c) => page_before(&conn, session, Some(c as i64), limit)?,
        // Tail, or a cursor past the end (session rewound): the tail.
        _ => page_before(&conn, session, None, limit)?,
    };
    Ok(Page {
        items,
        cursor: last.max(0) as u64,
        has_more,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &str = "
CREATE TABLE session_v2 (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, parent_id TEXT, slug TEXT NOT NULL,
  directory TEXT NOT NULL, title TEXT, version TEXT NOT NULL, time_created INTEGER NOT NULL,
  time_updated INTEGER NOT NULL, time_archived INTEGER);
CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL,
  seq INTEGER NOT NULL, time_created INTEGER NOT NULL, time_updated INTEGER NOT NULL, data TEXT NOT NULL);
CREATE UNIQUE INDEX session_message_session_seq_idx ON session_message (session_id, seq);";

    fn db(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "klark-transcript-opencode-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("opencode.db");
        let c = Connection::open(&p).unwrap();
        c.execute_batch(SCHEMA).unwrap();
        c.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        let big = "x".repeat(10_000);
        let t = 1_795_768_096_336i64;
        let rows: Vec<(i64, &str, Value)> = vec![
            (
                1,
                "system",
                json!({"time":{"created":t},"text":"SYSTEM PROMPT"}),
            ),
            (2, "model-switched", json!({"time":{"created":t}})),
            (
                4,
                "user",
                json!({"time":{"created":t},"text":"read the brief","files":[{"name":"shot.png"}],"agents":[]}),
            ),
            (
                7,
                "assistant",
                json!({"time":{"created":t},"agent":"build","content":[
                    {"type":"reasoning","text":"[REDACTED]start with the brief\n"},
                    {"type":"text","text":"I'll read the brief first.\n\n"},
                    {"type":"tool","id":"call-1","name":"read","state":{"status":"completed","input":{"path":"~/brief.md"},
                      "content":[{"type":"text","text":big}]}},
                    {"type":"tool","id":"call-2","name":"bash","state":{"status":"error","input":{"command":"false"},"error":{"message":"exit 1"}}},
                    {"type":"tool","id":"call-3","name":"write","state":{"status":"running","input":{"path":"/w/big.txt","content":big}}}
                ]}),
            ),
            (
                8,
                "synthetic",
                json!({"time":{"created":t},"text":"INJECTED"}),
            ),
            (
                9,
                "shell",
                json!({"time":{"created":t},"command":"ls","status":"exited","exit":0,"output":{"output":"a\nb","cursor":0,"size":3,"truncated":false}}),
            ),
            (
                10,
                "assistant",
                json!({"time":{"created":t},"content":[],"finish":"error"}),
            ),
            (
                11,
                "idle",
                json!({"time":{"created":t},"outcome":"failed","error":{"type":"provider.quota","message":"Insufficient balance."}}),
            ),
            (
                12,
                "idle",
                json!({"time":{"created":t},"outcome":"succeeded"}),
            ),
            (
                13,
                "compaction",
                json!({"time":{"created":t},"status":"completed","reason":"auto","summary":"S"}),
            ),
            (14, "user", json!({"time":{"created":t},"text":"thanks"})),
        ];
        for (seq, ty, m) in rows {
            c.execute(
                "INSERT INTO session_message(id,session_id,type,seq,time_created,time_updated,data) VALUES(?1,'s1',?2,?3,?4,?4,?5)",
                params![format!("m{seq}"), ty, seq, t, m.to_string()],
            )
            .unwrap();
        }
        c.execute(
            "INSERT INTO session_message(id,session_id,type,seq,time_created,time_updated,data) VALUES('o1','other','user',1,1,1,'{\"text\":\"OTHER SESSION\"}')",
            [],
        )
        .unwrap();
        c.execute_batch(
            "INSERT INTO session_v2(id,project_id,parent_id,slug,directory,version,time_created,time_updated,time_archived) VALUES
              ('s1','p',NULL,'a','/w','2',1,10,NULL), ('old','p',NULL,'b','/w','2',1,5,NULL),
              ('child','p','s1','c','/w','2',1,99,NULL), ('arch','p',NULL,'d','/w','2',1,98,5);",
        )
        .unwrap();
        p
    }

    fn kinds(items: &[Value]) -> Vec<String> {
        items
            .iter()
            .map(|i| {
                format!(
                    "{}/{}",
                    i["role"].as_str().unwrap(),
                    i["kind"].as_str().unwrap()
                )
            })
            .collect()
    }

    #[test]
    fn transcript_opencode_maps_every_entry_type() {
        let p = db("map");
        let page = read_page(&p, "s1", Query::Tail, 200).unwrap();
        assert_eq!(page.cursor, 14);
        assert!(!page.has_more);
        assert_eq!(
            kinds(&page.items),
            [
                "user/text",
                "assistant/thinking",
                "assistant/text",
                "assistant/tool_use",
                "tool/tool_result",
                "assistant/tool_use",
                "tool/tool_result",
                "assistant/tool_use",
                "user/text",
                "tool/tool_result",
                "assistant/text",
                "assistant/text",
                "user/text",
            ]
        );
        let all = serde_json::to_string(&page.items).unwrap();
        assert!(
            !all.contains("SYSTEM PROMPT")
                && !all.contains("INJECTED")
                && !all.contains("OTHER SESSION")
        );
        let it = &page.items;
        assert_eq!(it[0]["text"], "read the brief\n[shot.png]");
        assert_eq!(it[0]["id"], "opencode:4");
        assert_eq!(it[0]["cursor"], "4");
        assert!(it[0]["ts"].as_str().unwrap().starts_with("2026-11-"));
        assert_eq!(it[1]["id"], "opencode:7");
        assert_eq!(it[2]["id"], "opencode:7#1");
        assert_eq!(it[3]["tool"], "read");
        assert_eq!(it[3]["tool_use_id"], "call-1");
        assert_eq!(it[3]["input"]["path"], "~/brief.md");
        assert_eq!(it[4]["tool_use_id"], "call-1");
        assert_eq!(it[4]["ok"], true);
        assert!(it[4]["text"].as_str().unwrap().len() < TOOL_RESULT_CAP + 100);
        assert_eq!(it[1]["text"], "start with the brief");
        assert_eq!(it[2]["text"], "I'll read the brief first.");
        assert_eq!(it[5]["tool"], "shell");
        assert_eq!(it[6]["ok"], false);
        assert_eq!(it[6]["text"], "exit 1");
        assert_eq!(it[8]["text"], "!ls");
        assert_eq!(it[9]["text"], "a\nb");
        assert_eq!(it[9]["ok"], true);
        assert_eq!(it[10]["text"], "Error: Insufficient balance.");
        // a long write keeps its path and cuts its content
        assert_eq!(it[7]["input"]["path"], "/w/big.txt");
        assert!(it[7]["input"]["content"].as_str().unwrap().len() < 400);
        // the failed assistant entry (seq 10) does not repeat the idle error
        assert_eq!(all.matches("Insufficient balance").count(), 1);
    }

    #[test]
    fn transcript_opencode_patch_gets_its_file_as_path() {
        let mut i = json!({"patchText": "*** Begin Patch\n*** Update File: /w/a.css\n@@\n-x\n+y\n*** End Patch"});
        patch_path(&mut i);
        assert_eq!(i["path"], "/w/a.css");
        let mut keep = json!({"path": "/w/b", "patchText": "*** Add File: /w/c"});
        patch_path(&mut keep);
        assert_eq!(keep["path"], "/w/b");
        assert_eq!(canonical_tool("apply_patch"), "patch");
        assert_eq!(canonical_tool("read"), "read");
    }

    #[test]
    fn transcript_opencode_paging() {
        let p = db("paging");
        let tail = read_page(&p, "s1", Query::Tail, 2).unwrap();
        assert_eq!(kinds(&tail.items), ["assistant/text", "user/text"]);
        assert!(tail.has_more);
        let oldest: u64 = tail.items[0]["cursor"].as_str().unwrap().parse().unwrap();
        assert_eq!(oldest, 13);
        let older = read_page(&p, "s1", Query::Before(oldest), 2).unwrap();
        assert_eq!(older.items[0]["text"], "Error: Insufficient balance.");
        assert!(older.has_more);
        // whole rows only: the shell row has two items
        let shell = read_page(&p, "s1", Query::Before(11), 1).unwrap();
        assert_eq!(kinds(&shell.items), ["user/text", "tool/tool_result"]);
        let first = read_page(&p, "s1", Query::Before(7), 200).unwrap();
        assert_eq!(kinds(&first.items), ["user/text"]);
        assert!(!first.has_more);

        assert!(read_page(&p, "s1", Query::After(14), 200)
            .unwrap()
            .items
            .is_empty());
        let newer = read_page(&p, "s1", Query::After(11), 200).unwrap();
        assert_eq!(kinds(&newer.items), ["assistant/text", "user/text"]);
        // a cursor past the end (session rewound) gives the tail
        assert_eq!(
            read_page(&p, "s1", Query::After(99), 200)
                .unwrap()
                .items
                .len(),
            13
        );
        // an unknown session is empty, not an error
        let none = read_page(&p, "nope", Query::Tail, 200).unwrap();
        assert!(none.items.is_empty() && none.cursor == 0);
    }

    #[test]
    fn transcript_opencode_session_discovery_and_missing_db() {
        let p = db("find");
        assert_eq!(find_session(&p, Some("old"), None).as_deref(), Some("old"));
        // newest top-level, unarchived session in the folder
        assert_eq!(
            find_session(&p, Some("nope"), Some("/w")).as_deref(),
            Some("s1")
        );
        assert_eq!(find_session(&p, None, Some("/elsewhere")), None);
        let missing = p.with_file_name("absent.db");
        assert_eq!(find_session(&missing, Some("s1"), Some("/w")), None);
        assert!(!missing.exists(), "read-only open must not create the db");
    }

    /// Read-only smoke run against the real store: prints counts only.
    /// `KLARK_OPENCODE_SMOKE=1 cargo test opencode_smoke -- --nocapture`
    #[test]
    fn transcript_opencode_smoke() {
        if std::env::var_os("KLARK_OPENCODE_SMOKE").is_none() {
            return;
        }
        let p = db_path().unwrap();
        let conn = open(&p).unwrap();
        let mut st = conn
            .prepare("SELECT id FROM session_v2 WHERE parent_id IS NULL ORDER BY time_updated DESC LIMIT 5")
            .unwrap();
        let sids: Vec<String> = st
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for (n, sid) in sids.iter().enumerate() {
            let page = read_page(&p, sid, Query::Tail, 200).unwrap();
            let mut counts = std::collections::BTreeMap::new();
            for k in kinds(&page.items) {
                *counts.entry(k).or_insert(0) += 1;
            }
            println!(
                "SMOKE recent#{n} items={} has_more={} counts={counts:?}",
                page.items.len(),
                page.has_more
            );
        }
    }
}
