//! Devin CLI transcripts, read from its live SQLite store
//! (`~/.local/share/devin/cli/sessions.db`).
//!
//! The conversation is a forest in `message_nodes` (one row per message,
//! `parent_node_id` NULL for roots). The visible conversation is the chain
//! from `sessions.main_chain_id` (the current leaf's `node_id`) up to its
//! root, so only that chain is walked, with a recursive CTE bounded by the
//! page size. Cursors are `node_id`s on that chain.
//!
//! The file is opened read-only (no WAL checkpoint, no writes); Devin keeps
//! writing to it while we read.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde_json::{json, Value};

use super::{take_newest, truncate, Group, Page, Query, MAX_LIMIT, TOOL_RESULT_CAP};

/// Tool-call inputs longer than this (as JSON) are sent as a truncated string.
const INPUT_CAP: usize = 1024;
/// Hard bound on chain depth, against a corrupt (cyclic) parent link.
const MAX_DEPTH: i64 = 1_000_000;

/// Devin's store, honouring XDG_DATA_HOME.
pub fn db_path() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| super::home().map(|h| h.join(".local/share")))
        .map(|d| d.join("devin/cli/sessions.db"))
}

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_millis(200))?;
    Ok(conn)
}

/// The session to read: the id klark recorded for the pane if Devin knows
/// it, else the most recently active visible session in `cwd`.
pub fn find_session(db: &Path, id: Option<&str>, cwd: Option<&str>) -> Option<String> {
    if !db.is_file() {
        return None;
    }
    let conn = open(db).ok()?;
    if let Some(id) = id {
        let hit: Option<String> = conn
            .query_row("SELECT id FROM sessions WHERE id = ?1", [id], |r| r.get(0))
            .optional()
            .ok()?;
        if hit.is_some() {
            return hit;
        }
    }
    conn.query_row(
        "SELECT id FROM sessions WHERE working_directory = ?1 AND NOT COALESCE(hidden, 0) \
         ORDER BY last_activity_at DESC LIMIT 1",
        [cwd?],
        |r| r.get(0),
    )
    .optional()
    .ok()?
}

fn ts_of(msg: &Value, created_at: Option<i64>) -> Option<Value> {
    if let Some(s) = msg.pointer("/metadata/created_at").and_then(Value::as_str) {
        return Some(json!(s));
    }
    let secs = created_at?;
    // Seconds, or milliseconds on newer rows.
    let secs = if secs > 100_000_000_000 {
        secs / 1000
    } else {
        secs
    };
    time::OffsetDateTime::from_unix_timestamp(secs)
        .ok()?
        .format(&time::format_description::well_known::Rfc3339)
        .ok()
        .map(Value::String)
}

fn short_input(args: &Value) -> Value {
    // Some providers send arguments as a JSON-encoded string.
    let v = match args {
        Value::String(s) => serde_json::from_str(s).unwrap_or_else(|_| args.clone()),
        _ => args.clone(),
    };
    let s = v.to_string();
    if s.len() <= INPUT_CAP {
        v
    } else {
        json!(truncate(&s, INPUT_CAP))
    }
}

/// Items from one `message_nodes.chat_message`. Ids are `devin:<node_id>`
/// (with `#n` for the n-th item after the first).
pub fn devin_items(chat: &str, node_id: i64, created_at: Option<i64>) -> Vec<Value> {
    let Ok(msg) = serde_json::from_str::<Value>(chat) else {
        return vec![];
    };
    let role = msg.get("role").and_then(Value::as_str).unwrap_or("");
    let ts = ts_of(&msg, created_at);
    let base = format!("devin:{node_id}");
    let cursor = node_id.to_string();
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
    let content = msg.get("content").and_then(Value::as_str).unwrap_or("");
    match role {
        "user" if !content.trim().is_empty() => {
            push(json!({"role": "user", "kind": "text", "text": content}));
        }
        "assistant" => {
            let thinking = msg
                .pointer("/thinking/thinking")
                .and_then(Value::as_str)
                .unwrap_or("");
            if !thinking.trim().is_empty() {
                push(json!({"role": "assistant", "kind": "thinking", "text": thinking}));
            }
            if !content.trim().is_empty() {
                push(json!({"role": "assistant", "kind": "text", "text": content}));
            }
            for call in msg
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                push(json!({
                    "role": "assistant",
                    "kind": "tool_use",
                    "tool": call.get("name").cloned().unwrap_or(Value::Null),
                    "tool_use_id": call.get("id").cloned().unwrap_or(Value::Null),
                    "input": short_input(call.get("arguments").unwrap_or(&Value::Null)),
                }));
            }
        }
        "tool" => {
            let ext = msg.pointer("/metadata/extensions");
            let text = if content.is_empty() {
                ext.and_then(|e| e.pointer("/chisel~1terminal_output/text"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
            } else {
                content
            };
            let ok = ext
                .and_then(|e| e.pointer("/chisel~1tool_result_meta/success"))
                .and_then(Value::as_bool)
                != Some(false);
            push(json!({
                "role": "tool",
                "kind": "tool_result",
                "tool_use_id": msg.get("tool_call_id").cloned().unwrap_or(Value::Null),
                "text": truncate(text, TOOL_RESULT_CAP),
                "ok": ok,
            }));
        }
        // system prompts / injected context are not part of the conversation.
        _ => {}
    }
    out
}

struct Node {
    id: i64,
    chat: String,
    created_at: Option<i64>,
}

/// Up to `n` chain nodes, newest first, starting at `start` and walking up.
fn walk(conn: &Connection, session: &str, start: i64, n: usize) -> rusqlite::Result<Vec<Node>> {
    let mut st = conn.prepare_cached(
        "WITH RECURSIVE c(nid, pid, d) AS ( \
           SELECT node_id, parent_node_id, 0 FROM message_nodes \
            WHERE session_id = ?1 AND node_id = ?2 \
           UNION ALL \
           SELECT m.node_id, m.parent_node_id, c.d + 1 FROM message_nodes m JOIN c \
             ON m.session_id = ?1 AND m.node_id = c.pid WHERE c.d + 1 < ?3 \
         ) \
         SELECT c.nid, m.chat_message, m.created_at FROM c JOIN message_nodes m \
           ON m.session_id = ?1 AND m.node_id = c.nid ORDER BY c.d",
    )?;
    let rows = st.query_map(
        params![session, start, n.min(MAX_DEPTH as usize) as i64],
        |r| {
            Ok(Node {
                id: r.get(0)?,
                chat: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                created_at: r.get(2).ok(),
            })
        },
    )?;
    rows.collect()
}

/// Depth of `target` below `leaf` on the chain (0 = the leaf), scanning ids only.
fn depth_of(
    conn: &Connection,
    session: &str,
    leaf: i64,
    target: i64,
) -> rusqlite::Result<Option<i64>> {
    conn.query_row(
        "WITH RECURSIVE c(nid, pid, d) AS ( \
           SELECT node_id, parent_node_id, 0 FROM message_nodes \
            WHERE session_id = ?1 AND node_id = ?2 \
           UNION ALL \
           SELECT m.node_id, m.parent_node_id, c.d + 1 FROM message_nodes m JOIN c \
             ON m.session_id = ?1 AND m.node_id = c.pid WHERE c.nid != ?3 AND c.d < ?4 \
         ) SELECT d FROM c WHERE nid = ?3",
        params![session, leaf, target, MAX_DEPTH],
        |r| r.get(0),
    )
    .optional()
}

fn groups(nodes: Vec<Node>) -> Vec<Group> {
    // `walk` is newest first; groups are oldest first.
    nodes
        .into_iter()
        .rev()
        .map(|n| (n.id as u64, devin_items(&n.chat, n.id, n.created_at)))
        .filter(|g| !g.1.is_empty())
        .collect()
}

/// Newest items before (above) chain node `start`, inclusive of `start`.
fn page_from(
    conn: &Connection,
    session: &str,
    start: i64,
    limit: usize,
) -> rusqlite::Result<(Vec<Value>, bool)> {
    // Most nodes yield one item; system nodes yield none, so widen until
    // enough items are found or the root is reached.
    let mut n = limit + 1;
    loop {
        let nodes = walk(conn, session, start, n)?;
        let reached_root = nodes.len() < n;
        let g = groups(nodes);
        let count: usize = g.iter().map(|g| g.1.len()).sum();
        if count > limit || reached_root || n >= MAX_DEPTH as usize {
            return Ok(take_newest(g, limit, !reached_root));
        }
        n *= 4;
    }
}

pub fn read_page(db: &Path, session: &str, query: Query, limit: usize) -> rusqlite::Result<Page> {
    let limit = limit.clamp(1, MAX_LIMIT);
    let conn = open(db)?;
    let leaf: Option<i64> = conn
        .query_row(
            "SELECT main_chain_id FROM sessions WHERE id = ?1",
            [session],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let Some(leaf) = leaf else {
        return Ok(Page {
            items: vec![],
            cursor: 0,
            has_more: false,
        });
    };
    let (items, has_more) = match query {
        Query::After(c) => match depth_of(&conn, session, leaf, c as i64)? {
            Some(0) => (vec![], false),
            // Nodes strictly newer than `c`: the `d` nearest the leaf.
            Some(d) => {
                let g = groups(walk(&conn, session, leaf, d as usize)?);
                take_newest(g, limit, false)
            }
            // Cursor no longer on the chain (branch switched): the tail.
            None => page_from(&conn, session, leaf, limit)?,
        },
        Query::Before(c) => {
            let parent: Option<i64> = conn
                .query_row(
                    "SELECT parent_node_id FROM message_nodes WHERE session_id = ?1 AND node_id = ?2",
                    params![session, c as i64],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            match parent {
                Some(p) => page_from(&conn, session, p, limit)?,
                None => (vec![], false),
            }
        }
        Query::Tail => page_from(&conn, session, leaf, limit)?,
    };
    Ok(Page {
        items,
        cursor: leaf as u64,
        has_more,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCHEMA: &str = "
CREATE TABLE sessions (id TEXT PRIMARY KEY, working_directory TEXT, backend_type TEXT, model TEXT,
  agent_mode TEXT, created_at INTEGER, last_activity_at INTEGER, title TEXT, main_chain_id INTEGER,
  hidden INTEGER DEFAULT 0, metadata TEXT);
CREATE TABLE message_nodes (row_id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT NOT NULL,
  node_id INTEGER NOT NULL, parent_node_id INTEGER, chat_message TEXT NOT NULL, created_at INTEGER,
  metadata TEXT, UNIQUE(session_id, node_id));
CREATE INDEX idx_mn_session ON message_nodes(session_id);";

    fn db(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "klark-transcript-devin-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("sessions.db");
        let c = Connection::open(&p).unwrap();
        c.execute_batch(SCHEMA).unwrap();
        c.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        let big = "x".repeat(10_000);
        let rows: Vec<(i64, Option<i64>, Value)> = vec![
            (0, None, json!({"role":"system","content":"sys prompt"})),
            (
                1,
                Some(0),
                json!({"role":"user","content":"fix the bug","metadata":{"created_at":"2026-11-06T10:00:00Z"}}),
            ),
            (
                2,
                Some(1),
                json!({"role":"assistant","content":"Looking.","thinking":{"thinking":"check main","signature":"s"},
                "tool_calls":[{"id":"call_1","name":"exec","arguments":{"command":"ls"},"index":0,"kind":"function"}]}),
            ),
            (
                3,
                Some(2),
                json!({"role":"tool","content":big,"tool_call_id":"call_1",
                "metadata":{"extensions":{"chisel/tool_result_meta":{"success":false,"kind":"execute"}}}}),
            ),
            // abandoned branch off node 2
            (
                10,
                Some(2),
                json!({"role":"assistant","content":"ABANDONED"}),
            ),
            (4, Some(3), json!({"role":"assistant","content":"Done."})),
            (5, Some(4), json!({"role":"user","content":"thanks"})),
            // an unrelated root
            (20, None, json!({"role":"user","content":"OTHER ROOT"})),
        ];
        for (n, p, m) in rows {
            c.execute(
                "INSERT INTO message_nodes(session_id,node_id,parent_node_id,chat_message,created_at) VALUES('s1',?1,?2,?3,1793850921)",
                params![n, p, m.to_string()],
            )
            .unwrap();
        }
        c.execute_batch(
            "INSERT INTO sessions(id,working_directory,created_at,last_activity_at,main_chain_id,hidden) VALUES
              ('s1','/w',1,10,5,0), ('old','/w',1,5,NULL,0), ('hid','/w',1,99,NULL,1);",
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
    fn transcript_devin_main_chain_only_with_tool_mapping() {
        let p = db("chain");
        let page = read_page(&p, "s1", Query::Tail, 200).unwrap();
        assert_eq!(page.cursor, 5);
        assert!(!page.has_more);
        assert_eq!(
            kinds(&page.items),
            [
                "user/text",
                "assistant/thinking",
                "assistant/text",
                "assistant/tool_use",
                "tool/tool_result",
                "assistant/text",
                "user/text"
            ]
        );
        let all = serde_json::to_string(&page.items).unwrap();
        assert!(
            !all.contains("ABANDONED")
                && !all.contains("OTHER ROOT")
                && !all.contains("sys prompt")
        );
        let ids: Vec<&str> = page
            .items
            .iter()
            .map(|i| i["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids[..4], ["devin:1", "devin:2", "devin:2#1", "devin:2#2"]);
        let tu = &page.items[3];
        assert_eq!(tu["tool"], "exec");
        assert_eq!(tu["tool_use_id"], "call_1");
        assert_eq!(tu["input"]["command"], "ls");
        let tr = &page.items[4];
        assert_eq!(tr["tool_use_id"], "call_1");
        assert_eq!(tr["ok"], false);
        assert!(tr["text"].as_str().unwrap().len() < TOOL_RESULT_CAP + 100);
        assert_eq!(page.items[0]["ts"], "2026-11-06T10:00:00Z");
        assert!(page.items[1]["ts"].as_str().unwrap().starts_with("2026-"));
        assert_eq!(page.items[1]["cursor"], "2");
    }

    #[test]
    fn transcript_devin_paging() {
        let p = db("paging");
        let tail = read_page(&p, "s1", Query::Tail, 2).unwrap();
        assert_eq!(kinds(&tail.items), ["assistant/text", "user/text"]);
        assert!(tail.has_more);
        let oldest = tail.items[0]["cursor"].as_str().unwrap().parse().unwrap();
        let older = read_page(&p, "s1", Query::Before(oldest), 2).unwrap();
        // node 3 (one item) then node 2 (three items) exceeds 2: whole groups only.
        assert_eq!(kinds(&older.items), ["tool/tool_result"]);
        assert!(older.has_more);
        let rest = read_page(&p, "s1", Query::Before(3), 200).unwrap();
        assert_eq!(rest.items.len(), 4);
        assert!(!rest.has_more);

        assert!(read_page(&p, "s1", Query::After(5), 200)
            .unwrap()
            .items
            .is_empty());
        let newer = read_page(&p, "s1", Query::After(2), 200).unwrap();
        assert_eq!(
            kinds(&newer.items),
            ["tool/tool_result", "assistant/text", "user/text"]
        );
        // a cursor on the abandoned branch falls back to the tail
        assert_eq!(
            read_page(&p, "s1", Query::After(10), 200)
                .unwrap()
                .items
                .len(),
            7
        );
    }

    #[test]
    fn transcript_devin_session_discovery_and_missing_db() {
        let p = db("find");
        assert_eq!(find_session(&p, Some("old"), None).as_deref(), Some("old"));
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
    /// `KLARK_DEVIN_SMOKE=1 cargo nextest run -E 'test(/devin_smoke/)' --no-capture`
    #[test]
    fn transcript_devin_smoke() {
        if std::env::var_os("KLARK_DEVIN_SMOKE").is_none() {
            return;
        }
        let p = db_path().unwrap();
        let conn = open(&p).unwrap();
        let mut st = conn
            .prepare(
                "SELECT id FROM sessions WHERE NOT COALESCE(hidden,0) AND main_chain_id IS NOT NULL \
                 ORDER BY last_activity_at DESC LIMIT 5",
            )
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
