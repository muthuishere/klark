//! Durable tasks (klark#25): a prompt sent with `klark msg send --task <id>`
//! is recorded in SQLite, the addressee answers with `klark msg reply --task
//! <id> "<result>"`, and anyone reads the outcome with `klark task get/wait`.
//! Nothing depends on terminal scrollback.
//!
//! The database is `tasks.db` next to `messages.jsonl` (the msg data folder),
//! and every state change also appends one JSON line to `tasks.jsonl`, so a
//! `klark feed --follow` can watch tasks. Callers (dmcp, scripts, agents) keep
//! their own task ids: an id is any 1..=64 of `[A-Za-z0-9_.-]`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

/// The largest result kept. A longer one is cut and marked `truncated`.
pub const RESULT_CAP: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Task {
    pub id: String,
    pub from: String,
    /// The addressee as shown (`machine/session/agent`).
    pub to: String,
    /// The addressee's session and pane: only that pane may answer.
    pub to_session: String,
    pub to_pane: String,
    pub prompt: String,
    /// `delivered`, `answered` or `failed`.
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answered_by: Option<String>,
    /// Unix seconds.
    pub created: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivered: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answered: Option<i64>,
}

pub fn valid_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// A fresh id: `t_` plus 12 hex digits.
pub fn new_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mix = nanos ^ (u128::from(std::process::id()) << 64);
    format!("t_{:012x}", (mix as u64) & 0xffff_ffff_ffff)
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

pub struct Store {
    conn: Connection,
    events: PathBuf,
}

impl Store {
    pub fn open(dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = dir.join("tasks.db");
        let conn = Connection::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY,
                from_addr TEXT NOT NULL,
                to_addr TEXT NOT NULL,
                to_session TEXT NOT NULL,
                to_pane TEXT NOT NULL,
                prompt TEXT NOT NULL,
                status TEXT NOT NULL,
                result TEXT,
                truncated INTEGER NOT NULL DEFAULT 0,
                error TEXT,
                answered_by TEXT,
                created INTEGER NOT NULL,
                delivered INTEGER,
                answered INTEGER
             );
             CREATE INDEX IF NOT EXISTS tasks_created ON tasks(created);",
        )
        .map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Store {
            conn,
            events: dir.join("tasks.jsonl"),
        })
    }

    /// Record a task about to be sent. An id already used is refused unless
    /// that task failed (a caller retrying it).
    pub fn create(
        &self,
        id: &str,
        from: &str,
        to: &str,
        to_session: &str,
        to_pane: &str,
        prompt: &str,
    ) -> Result<(), String> {
        if !valid_id(id) {
            return Err(format!(
                "task id {id:?}: use 1-64 letters, digits, '_', '-' or '.'"
            ));
        }
        if let Some(old) = self.get(id)? {
            if old.status != "failed" {
                return Err(format!("task {id} already exists ({})", old.status));
            }
            self.conn
                .execute("DELETE FROM tasks WHERE id = ?1", params![id])
                .map_err(|e| e.to_string())?;
        }
        self.conn
            .execute(
                "INSERT INTO tasks (id, from_addr, to_addr, to_session, to_pane, prompt, status, created)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'sending', ?7)",
                params![id, from, to, to_session, to_pane, prompt, now()],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    pub fn delivered(&self, id: &str) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE tasks SET status = 'delivered', delivered = ?2, error = NULL WHERE id = ?1",
                params![id, now()],
            )
            .map_err(|e| e.to_string())?;
        self.event(id)
    }

    pub fn failed(&self, id: &str, error: &str) -> Result<(), String> {
        self.conn
            .execute(
                "UPDATE tasks SET status = 'failed', error = ?2 WHERE id = ?1",
                params![id, error],
            )
            .map_err(|e| e.to_string())?;
        self.event(id)
    }

    /// Store the answer. Only the addressee's pane may answer; returns
    /// whether the result was cut at [`RESULT_CAP`].
    pub fn answer(
        &self,
        id: &str,
        session: &str,
        pane: &str,
        by: &str,
        result: &str,
    ) -> Result<bool, String> {
        let task = self
            .get(id)?
            .ok_or_else(|| format!("no task {id} (klark task list)"))?;
        if task.to_session != session || task.to_pane != pane {
            return Err(format!(
                "task {id} was sent to {}; only that agent may answer it",
                task.to
            ));
        }
        if task.status == "answered" {
            return Err(format!("task {id} is already answered"));
        }
        let (kept, truncated) = cap(result);
        self.conn
            .execute(
                "UPDATE tasks SET status = 'answered', result = ?2, truncated = ?3,
                 answered_by = ?4, answered = ?5, error = NULL WHERE id = ?1",
                params![id, kept, truncated, by, now()],
            )
            .map_err(|e| e.to_string())?;
        self.event(id)?;
        Ok(truncated)
    }

    pub fn get(&self, id: &str) -> Result<Option<Task>, String> {
        self.conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM tasks WHERE id = ?1"),
                params![id],
                row,
            )
            .optional()
            .map_err(|e| e.to_string())
    }

    /// The newest `n` tasks, newest first.
    pub fn list(&self, n: usize) -> Result<Vec<Task>, String> {
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT {COLUMNS} FROM tasks ORDER BY created DESC, rowid DESC LIMIT ?1"
            ))
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![n as i64], row)
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// Wait until the task is answered or failed, or `timeout` passes;
    /// returns the task as it then is.
    pub fn wait(&self, id: &str, timeout: Duration) -> Result<Task, String> {
        let deadline = Instant::now() + timeout;
        loop {
            let task = self
                .get(id)?
                .ok_or_else(|| format!("no task {id} (klark task list)"))?;
            if task.status == "answered" || task.status == "failed" || Instant::now() >= deadline {
                return Ok(task);
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    }

    /// One line per state change in `tasks.jsonl` (for `klark feed --follow`).
    fn event(&self, id: &str) -> Result<(), String> {
        use std::io::Write;
        let Some(task) = self.get(id)? else {
            return Ok(());
        };
        let mut line = serde_json::json!({
            "ts": now(),
            "id": task.id,
            "status": task.status,
            "from": task.from,
            "to": task.to,
        });
        if let Some(e) = &task.error {
            line["error"] = e.clone().into();
        }
        if let Some(by) = &task.answered_by {
            line["answered_by"] = by.clone().into();
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.events)
            .map_err(|e| format!("{}: {e}", self.events.display()))?;
        writeln!(f, "{line}").map_err(|e| e.to_string())
    }
}

const COLUMNS: &str = "id, from_addr, to_addr, to_session, to_pane, prompt, status, result, \
                       truncated, error, answered_by, created, delivered, answered";

fn row(r: &rusqlite::Row) -> rusqlite::Result<Task> {
    Ok(Task {
        id: r.get(0)?,
        from: r.get(1)?,
        to: r.get(2)?,
        to_session: r.get(3)?,
        to_pane: r.get(4)?,
        prompt: r.get(5)?,
        status: r.get(6)?,
        result: r.get(7)?,
        truncated: r.get::<_, i64>(8)? != 0,
        error: r.get(9)?,
        answered_by: r.get(10)?,
        created: r.get(11)?,
        delivered: r.get(12)?,
        answered: r.get(13)?,
    })
}

/// At most [`RESULT_CAP`] bytes, cut on a character boundary.
fn cap(text: &str) -> (&str, bool) {
    if text.len() <= RESULT_CAP {
        return (text, false);
    }
    let mut end = RESULT_CAP;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (Store, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "klark-task-store-{}-{}",
            std::process::id(),
            new_id()
        ));
        (Store::open(&dir).unwrap(), dir)
    }

    #[test]
    fn a_task_round_trips_and_survives_reopening() {
        let (s, dir) = store();
        s.create("t_1", "m/s/cli", "m/w/coder", "w", "w1:p1", "fix it")
            .unwrap();
        s.delivered("t_1").unwrap();
        assert_eq!(s.get("t_1").unwrap().unwrap().status, "delivered");
        let truncated = s
            .answer("t_1", "w", "w1:p1", "m/w/coder", "fixed in abc123")
            .unwrap();
        assert!(!truncated);
        drop(s);
        let s = Store::open(&dir).unwrap();
        let t = s.get("t_1").unwrap().unwrap();
        assert_eq!(t.status, "answered");
        assert_eq!(t.result.as_deref(), Some("fixed in abc123"));
        assert_eq!(t.answered_by.as_deref(), Some("m/w/coder"));
        assert!(t.delivered.is_some() && t.answered.is_some());
        let events = std::fs::read_to_string(dir.join("tasks.jsonl")).unwrap();
        let statuses: Vec<String> = events
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["status"].to_string())
            .collect();
        assert_eq!(statuses, ["\"delivered\"", "\"answered\""]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn only_the_addressee_answers_and_only_once() {
        let (s, dir) = store();
        s.create("t_2", "m/s/cli", "m/w/coder", "w", "w1:p1", "x")
            .unwrap();
        s.delivered("t_2").unwrap();
        let e = s
            .answer("t_2", "w", "w1:p2", "m/w/other", "me")
            .unwrap_err();
        assert!(e.contains("only that agent may answer"), "{e}");
        s.answer("t_2", "w", "w1:p1", "m/w/coder", "done").unwrap();
        let e = s
            .answer("t_2", "w", "w1:p1", "m/w/coder", "again")
            .unwrap_err();
        assert!(e.contains("already answered"), "{e}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ids_are_checked_reused_only_after_failure_and_results_capped() {
        let (s, dir) = store();
        assert!(s.create("bad id", "a", "b", "w", "p", "x").is_err());
        assert!(valid_id(&new_id()));
        s.create("t_3", "a", "b", "w", "p", "x").unwrap();
        assert!(s.create("t_3", "a", "b", "w", "p", "x").is_err(), "in use");
        s.failed("t_3", "agent_blocked").unwrap();
        assert_eq!(
            s.get("t_3").unwrap().unwrap().error.as_deref(),
            Some("agent_blocked")
        );
        s.create("t_3", "a", "b", "w", "p", "retry").unwrap();
        s.delivered("t_3").unwrap();
        let big = "é".repeat(RESULT_CAP);
        assert!(s.answer("t_3", "w", "p", "b", &big).unwrap());
        let t = s.get("t_3").unwrap().unwrap();
        assert!(t.truncated && t.result.unwrap().len() <= RESULT_CAP);
        assert!(s.get("nope").unwrap().is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn wait_returns_at_answer_or_timeout() {
        let (s, dir) = store();
        s.create("t_4", "a", "b", "w", "p", "x").unwrap();
        s.delivered("t_4").unwrap();
        let t = s.wait("t_4", Duration::from_millis(400)).unwrap();
        assert_eq!(t.status, "delivered", "timed out, still open");
        s.answer("t_4", "w", "p", "b", "ok").unwrap();
        assert_eq!(
            s.wait("t_4", Duration::from_secs(5)).unwrap().status,
            "answered"
        );
        assert!(s.wait("missing", Duration::from_millis(10)).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
