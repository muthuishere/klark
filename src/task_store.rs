//! Durable tasks (klark#25, contract v2). klark's `tasks.db` is the only task
//! store: callers (dmcp, scripts, agents) create, deliver, follow and read
//! tasks through `klark task` / `klark msg send --task` and keep no task rows
//! of their own. Nothing depends on terminal scrollback.
//!
//! Lifecycle: `queued` (created, not sent) → `running` (delivered to the agent)
//! or `blocked` (the agent waits on a person; retryable) or `failed` → `done`
//! (the addressee answered with `klark msg reply --task`). Every change bumps
//! `updated` and appends one JSON line to `tasks.jsonl` (for `klark feed
//! --follow` and `klark task events`). Finished tasks are pruned after 30 days.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

/// The largest result kept. A longer one is cut and marked `truncated`.
pub const RESULT_CAP: usize = 64 * 1024;
/// The largest progress note kept.
pub const PROGRESS_CAP: usize = 2 * 1024;
/// Finished (done/failed) tasks older than this are pruned when the store opens.
pub const RETENTION: Duration = Duration::from_secs(30 * 24 * 3600);
const SCHEMA_VERSION: i64 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Task {
    pub id: String,
    /// Who the task belongs to (a user, or the sender); `--owner` reads are
    /// limited to it.
    pub owner: String,
    pub from: String,
    /// The addressee as shown (`machine/session/agent`).
    pub to: String,
    pub to_session: String,
    pub to_pane: String,
    /// The agent's stable id: its CLI conversation id, when klark knows it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    pub prompt: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub answered_by: Option<String>,
    /// Where the result came from: `agent` (its `msg reply --task`) or what
    /// the creator collected (`screen`, ...).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Unix seconds.
    pub created: i64,
    pub updated: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished: Option<i64>,
}

/// What a new task is.
#[derive(Debug, Clone, Default)]
pub struct NewTask {
    pub id: Option<String>,
    pub owner: Option<String>,
    pub from: String,
    pub to: String,
    pub to_session: String,
    pub to_pane: String,
    pub agent_id: Option<String>,
    pub parent_id: Option<String>,
    pub correlation_id: Option<String>,
    pub prompt: String,
}

/// A store error with a stable code for `--json` callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskError {
    pub code: &'static str,
    pub message: String,
}

impl TaskError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        TaskError {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for TaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<rusqlite::Error> for TaskError {
    fn from(e: rusqlite::Error) -> Self {
        TaskError::new("store_error", e.to_string())
    }
}

impl From<TaskError> for String {
    fn from(e: TaskError) -> String {
        e.message
    }
}

type R<T> = Result<T, TaskError>;

pub fn valid_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// A random UUID v4 (OS-seeded hashing; ids, not secrets).
pub fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let word = |salt: u64| {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(salt);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
        );
        h.write_u32(std::process::id());
        h.finish()
    };
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&word(1).to_le_bytes());
    b[8..].copy_from_slice(&word(2).to_le_bytes());
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// At most `cap` bytes, cut on a character boundary.
fn cut(text: &str, cap: usize) -> (&str, bool) {
    if text.len() <= cap {
        return (text, false);
    }
    let mut end = cap;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

pub struct Store {
    conn: Connection,
    events: PathBuf,
}

const COLUMNS: &str = "id, owner, from_addr, to_addr, to_session, to_pane, agent_id, parent_id, \
     correlation_id, prompt, status, progress, result, truncated, error, answered_by, source, \
     created, updated, started, finished";

fn row(r: &rusqlite::Row) -> rusqlite::Result<Task> {
    Ok(Task {
        id: r.get(0)?,
        owner: r.get(1)?,
        from: r.get(2)?,
        to: r.get(3)?,
        to_session: r.get(4)?,
        to_pane: r.get(5)?,
        agent_id: r.get(6)?,
        parent_id: r.get(7)?,
        correlation_id: r.get(8)?,
        prompt: r.get(9)?,
        status: r.get(10)?,
        progress: r.get(11)?,
        result: r.get(12)?,
        truncated: r.get::<_, i64>(13)? != 0,
        error: r.get(14)?,
        answered_by: r.get(15)?,
        source: r.get(16)?,
        created: r.get(17)?,
        updated: r.get(18)?,
        started: r.get(19)?,
        finished: r.get(20)?,
    })
}

/// Which tasks `list` returns.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub owner: Option<String>,
    pub from: Option<String>,
    pub statuses: Vec<String>,
    pub parent_id: Option<String>,
    pub correlation_id: Option<String>,
    pub limit: usize,
}

impl Store {
    pub fn open(dir: &Path) -> R<Self> {
        std::fs::create_dir_all(dir)
            .map_err(|e| TaskError::new("store_error", format!("{}: {e}", dir.display())))?;
        let path = dir.join("tasks.db");
        #[cfg(unix)]
        if !path.exists() {
            use std::os::unix::fs::OpenOptionsExt;
            // Prompts and results can be private: the file is the user's only.
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(&path);
        }
        let conn = Connection::open(&path)
            .map_err(|e| TaskError::new("store_error", format!("{}: {e}", path.display())))?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < SCHEMA_VERSION {
            // Earlier shapes were never released: start again.
            conn.execute_batch("DROP TABLE IF EXISTS tasks;")?;
        }
        conn.execute_batch(&format!(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS tasks (
                id TEXT PRIMARY KEY,
                owner TEXT NOT NULL,
                from_addr TEXT NOT NULL,
                to_addr TEXT NOT NULL,
                to_session TEXT NOT NULL,
                to_pane TEXT NOT NULL,
                agent_id TEXT,
                parent_id TEXT,
                correlation_id TEXT,
                prompt TEXT NOT NULL,
                status TEXT NOT NULL,
                progress TEXT,
                result TEXT,
                truncated INTEGER NOT NULL DEFAULT 0,
                error TEXT,
                answered_by TEXT,
                source TEXT,
                created INTEGER NOT NULL,
                updated INTEGER NOT NULL,
                started INTEGER,
                finished INTEGER
             );
             CREATE INDEX IF NOT EXISTS tasks_status ON tasks(status);
             CREATE INDEX IF NOT EXISTS tasks_owner ON tasks(owner);
             CREATE INDEX IF NOT EXISTS tasks_created ON tasks(created);
             PRAGMA user_version = {SCHEMA_VERSION};"
        ))?;
        let store = Store {
            conn,
            events: dir.join("tasks.jsonl"),
        };
        store.prune(RETENTION)?;
        Ok(store)
    }

    /// A queued task (nothing is sent). With an `id` already used: the same
    /// owner and prompt return the existing task (idempotent create); else
    /// `task_conflict`. A failed task's id may be reused (a retry).
    pub fn create(&self, t: NewTask) -> R<Task> {
        let id = t.id.clone().unwrap_or_else(new_id);
        if !valid_id(&id) {
            return Err(TaskError::new(
                "bad_request",
                format!("task id {id:?}: use 1-64 letters, digits, '_', '-' or '.'"),
            ));
        }
        let owner = t.owner.clone().unwrap_or_else(|| t.from.clone());
        if let Some(old) = self.get(&id)? {
            if old.status == "failed" {
                self.conn
                    .execute("DELETE FROM tasks WHERE id = ?1", params![id])?;
            } else if old.owner == owner && old.prompt == t.prompt && old.to_pane == t.to_pane {
                return Ok(old);
            } else {
                return Err(TaskError::new(
                    "task_conflict",
                    format!("task {id} already exists with another owner, agent or prompt"),
                ));
            }
        }
        let ts = now();
        self.conn.execute(
            &format!(
                "INSERT INTO tasks ({COLUMNS}) VALUES
                 (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'queued', NULL, NULL, 0, NULL, NULL, NULL, ?11, ?11, NULL, NULL)"
            ),
            params![
                id,
                owner,
                t.from,
                t.to,
                t.to_session,
                t.to_pane,
                t.agent_id,
                t.parent_id,
                t.correlation_id,
                t.prompt,
                ts
            ],
        )?;
        self.event(&id, None)?;
        self.need(&id)
    }

    fn need(&self, id: &str) -> R<Task> {
        self.get(id)?
            .ok_or_else(|| TaskError::new("task_not_found", format!("no task {id}")))
    }

    /// A caller's state change: `running` (delivered), `blocked` or `queued`
    /// (with a note), `failed` (with the error). `done` only comes from
    /// [`Store::answer`].
    pub fn set_status(&self, id: &str, status: &str, note: Option<&str>) -> R<Task> {
        let task = self.need(id)?;
        if !matches!(status, "queued" | "running" | "blocked" | "failed") {
            return Err(TaskError::new(
                "bad_request",
                format!("status {status}: use queued, running, blocked or failed (done comes from the agent's reply)"),
            ));
        }
        if matches!(task.status.as_str(), "done" | "failed") {
            return Err(TaskError::new(
                "task_finished",
                format!("task {id} is already {}", task.status),
            ));
        }
        let ts = now();
        let (error, progress) = match status {
            "failed" | "blocked" => (note, task.progress.as_deref()),
            _ => (None, note.or(task.progress.as_deref())),
        };
        self.conn.execute(
            "UPDATE tasks SET status = ?2, error = ?3, progress = ?4, updated = ?5,
             started = CASE WHEN ?2 = 'running' AND started IS NULL THEN ?5 ELSE started END,
             finished = CASE WHEN ?2 = 'failed' THEN ?5 ELSE finished END
             WHERE id = ?1",
            params![id, status, error, progress, ts],
        )?;
        self.event(id, note)?;
        self.need(id)
    }

    /// A progress note from the addressee while it works.
    pub fn progress(&self, id: &str, session: &str, pane: &str, text: &str) -> R<Task> {
        let task = self.addressee(id, session, pane)?;
        if task.status == "done" || task.status == "failed" {
            return Err(TaskError::new(
                "task_finished",
                format!("task {id} is already {}", task.status),
            ));
        }
        let (kept, _) = cut(text, PROGRESS_CAP);
        self.conn.execute(
            "UPDATE tasks SET progress = ?2, updated = ?3 WHERE id = ?1",
            params![id, kept, now()],
        )?;
        self.event(id, Some(kept))?;
        self.need(id)
    }

    fn addressee(&self, id: &str, session: &str, pane: &str) -> R<Task> {
        let task = self.need(id)?;
        if task.to_session != session || task.to_pane != pane {
            return Err(TaskError::new(
                "not_addressee",
                format!(
                    "task {id} was sent to {}; only that agent may answer it",
                    task.to
                ),
            ));
        }
        Ok(task)
    }

    /// The addressee's answer: `done`. Returns whether the result was cut.
    pub fn answer(&self, id: &str, session: &str, pane: &str, by: &str, result: &str) -> R<bool> {
        let task = self.addressee(id, session, pane)?;
        if task.status == "done" {
            return Err(TaskError::new(
                "task_finished",
                format!("task {id} is already answered"),
            ));
        }
        let (kept, truncated) = cut(result, RESULT_CAP);
        let ts = now();
        self.conn.execute(
            "UPDATE tasks SET status = 'done', result = ?2, truncated = ?3, answered_by = ?4,
             source = 'agent', error = NULL, updated = ?5, finished = ?5,
             started = COALESCE(started, ?5)
             WHERE id = ?1",
            params![id, kept, truncated, by, ts],
        )?;
        self.event(id, None)?;
        Ok(truncated)
    }

    /// The task's creator finishes it with a result it collected itself (an
    /// agent that never ran `msg reply --task`): `done` with `source`, no
    /// `answered_by`. Only the creator (`from`) may. Returns whether cut.
    pub fn complete(&self, id: &str, creator: &str, result: &str, source: &str) -> R<bool> {
        let task = self.need(id)?;
        if task.from != creator {
            return Err(TaskError::new(
                "not_creator",
                format!(
                    "task {id} was created by {}; only it may finish the task this way",
                    task.from
                ),
            ));
        }
        if matches!(task.status.as_str(), "done" | "failed") {
            return Err(TaskError::new(
                "task_finished",
                format!("task {id} is already {}", task.status),
            ));
        }
        let (kept, truncated) = cut(result, RESULT_CAP);
        let ts = now();
        self.conn.execute(
            "UPDATE tasks SET status = 'done', result = ?2, truncated = ?3, answered_by = NULL,
             source = ?4, error = NULL, updated = ?5, finished = ?5,
             started = COALESCE(started, ?5)
             WHERE id = ?1",
            params![id, kept, truncated, source, ts],
        )?;
        self.event(id, None)?;
        Ok(truncated)
    }

    pub fn get(&self, id: &str) -> R<Option<Task>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM tasks WHERE id = ?1"),
                params![id],
                row,
            )
            .optional()?)
    }

    /// `get` for one owner: another owner's task reads as not found.
    pub fn get_for(&self, id: &str, owner: Option<&str>) -> R<Task> {
        match self.get(id)? {
            Some(t) if owner.is_none_or(|o| o == t.owner) => Ok(t),
            _ => Err(TaskError::new("task_not_found", format!("no task {id}"))),
        }
    }

    /// Newest first.
    pub fn list(&self, f: &Filter) -> R<Vec<Task>> {
        let mut sql = format!("SELECT {COLUMNS} FROM tasks WHERE 1=1");
        let mut args: Vec<String> = Vec::new();
        for (col, val) in [
            ("owner", &f.owner),
            ("from_addr", &f.from),
            ("parent_id", &f.parent_id),
            ("correlation_id", &f.correlation_id),
        ] {
            if let Some(v) = val {
                args.push(v.clone());
                sql.push_str(&format!(" AND {col} = ?{}", args.len()));
            }
        }
        if !f.statuses.is_empty() {
            let marks: Vec<String> = f
                .statuses
                .iter()
                .map(|s| {
                    args.push(s.clone());
                    format!("?{}", args.len())
                })
                .collect();
            sql.push_str(&format!(" AND status IN ({})", marks.join(",")));
        }
        sql.push_str(&format!(
            " ORDER BY created DESC, rowid DESC LIMIT {}",
            f.limit.max(1)
        ));
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Until done or failed, or `timeout`; returns the task as it then is.
    pub fn wait(&self, id: &str, owner: Option<&str>, timeout: Duration) -> R<Task> {
        let deadline = Instant::now() + timeout;
        loop {
            let task = self.get_for(id, owner)?;
            if task.status == "done" || task.status == "failed" || Instant::now() >= deadline {
                return Ok(task);
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    }

    /// Remove finished tasks older than `age`; returns how many.
    pub fn prune(&self, age: Duration) -> R<usize> {
        let before = now() - age.as_secs() as i64;
        Ok(self.conn.execute(
            "DELETE FROM tasks WHERE status IN ('done', 'failed') AND updated < ?1",
            params![before],
        )?)
    }

    /// Events (lines of `tasks.jsonl`) at or after `since`, for one owner.
    pub fn events(&self, since: i64, owner: Option<&str>) -> Vec<serde_json::Value> {
        std::fs::read_to_string(&self.events)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|v| v["ts"].as_i64().unwrap_or(0) >= since)
            .filter(|v| owner.is_none_or(|o| v["owner"] == o))
            .collect()
    }

    /// One line per change in `tasks.jsonl` (for `klark feed --follow`).
    fn event(&self, id: &str, note: Option<&str>) -> R<()> {
        use std::io::Write;
        let Some(t) = self.get(id)? else {
            return Ok(());
        };
        let mut line = serde_json::json!({
            "ts": t.updated,
            "id": t.id,
            "status": t.status,
            "owner": t.owner,
            "from": t.from,
            "to": t.to,
        });
        for (key, val) in [
            ("note", note.map(str::to_string)),
            ("error", t.error.clone()),
            ("answered_by", t.answered_by.clone()),
            ("source", t.source.clone()),
            ("parent_id", t.parent_id.clone()),
            ("correlation_id", t.correlation_id.clone()),
        ] {
            if let Some(v) = val {
                line[key] = v.into();
            }
        }
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.events)
            .map_err(|e| {
                TaskError::new("store_error", format!("{}: {e}", self.events.display()))
            })?;
        writeln!(f, "{line}").map_err(|e| TaskError::new("store_error", e.to_string()))
    }
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

    fn new(id: Option<&str>, prompt: &str) -> NewTask {
        NewTask {
            id: id.map(str::to_string),
            owner: Some("u@x".into()),
            from: "m/s/dmcp".into(),
            to: "m/w/coder".into(),
            to_session: "w".into(),
            to_pane: "w1:p1".into(),
            agent_id: Some("conv-uuid".into()),
            parent_id: Some("p-1".into()),
            correlation_id: Some("c-1".into()),
            prompt: prompt.into(),
        }
    }

    #[test]
    fn ids_are_uuid_v4() {
        let a = new_id();
        let b = new_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "4", "{a}");
        assert!(matches!(&a[19..20], "8" | "9" | "a" | "b"), "{a}");
        assert!(valid_id(&a));
    }

    #[test]
    fn lifecycle_round_trips_and_survives_reopening() {
        let (s, dir) = store();
        let t = s.create(new(None, "fix it")).unwrap();
        assert_eq!(t.status, "queued");
        assert_eq!(t.parent_id.as_deref(), Some("p-1"));
        s.set_status(&t.id, "blocked", Some("agent on a dialog"))
            .unwrap();
        s.set_status(&t.id, "running", None).unwrap();
        s.progress(&t.id, "w", "w1:p1", "2 of 3 steps").unwrap();
        assert!(!s
            .answer(&t.id, "w", "w1:p1", "m/w/coder", "fixed in abc123")
            .unwrap());
        drop(s);
        let s = Store::open(&dir).unwrap();
        let got = s.get_for(&t.id, Some("u@x")).unwrap();
        assert_eq!(got.status, "done");
        assert_eq!(got.result.as_deref(), Some("fixed in abc123"));
        assert_eq!(got.progress.as_deref(), Some("2 of 3 steps"));
        assert_eq!(got.answered_by.as_deref(), Some("m/w/coder"));
        assert!(got.started.is_some() && got.finished.is_some());
        let statuses: Vec<String> = s
            .events(0, Some("u@x"))
            .iter()
            .map(|e| e["status"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            statuses,
            ["queued", "blocked", "running", "running", "done"]
        );
        assert!(s.events(0, Some("someone-else")).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn create_is_idempotent_and_conflicts_are_refused() {
        let (s, dir) = store();
        let a = s.create(new(Some("t_1"), "x")).unwrap();
        let again = s.create(new(Some("t_1"), "x")).unwrap();
        assert_eq!(a, again, "same id, owner and prompt: the same task");
        let e = s.create(new(Some("t_1"), "other")).unwrap_err();
        assert_eq!(e.code, "task_conflict");
        s.set_status("t_1", "failed", Some("agent gone")).unwrap();
        let retry = s.create(new(Some("t_1"), "other")).unwrap();
        assert_eq!(retry.status, "queued", "a failed id may be retried");
        assert_eq!(
            s.create(new(Some("bad id"), "x")).unwrap_err().code,
            "bad_request"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn only_the_addressee_answers_once_and_owners_are_isolated() {
        let (s, dir) = store();
        let t = s.create(new(None, "x")).unwrap();
        s.set_status(&t.id, "running", None).unwrap();
        let e = s.answer(&t.id, "w", "w1:p2", "other", "me").unwrap_err();
        assert_eq!(e.code, "not_addressee");
        assert_eq!(
            s.progress(&t.id, "w", "w9:p9", "x").unwrap_err().code,
            "not_addressee"
        );
        s.answer(&t.id, "w", "w1:p1", "m/w/coder", "done").unwrap();
        assert_eq!(
            s.answer(&t.id, "w", "w1:p1", "m/w/coder", "again")
                .unwrap_err()
                .code,
            "task_finished"
        );
        assert_eq!(
            s.set_status(&t.id, "running", None).unwrap_err().code,
            "task_finished"
        );
        assert_eq!(
            s.get_for(&t.id, Some("intruder")).unwrap_err().code,
            "task_not_found"
        );
        assert!(
            s.get_for(&t.id, None).is_ok(),
            "the machine's user sees all"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_creator_may_finish_with_a_collected_result() {
        let (s, dir) = store();
        let t = s.create(new(None, "x")).unwrap();
        s.set_status(&t.id, "running", None).unwrap();
        assert_eq!(
            s.complete(&t.id, "someone", "r", "screen")
                .unwrap_err()
                .code,
            "not_creator"
        );
        s.complete(&t.id, "m/s/dmcp", "from the screen", "screen")
            .unwrap();
        let got = s.get(&t.id).unwrap().unwrap();
        assert_eq!(got.status, "done");
        assert_eq!(got.source.as_deref(), Some("screen"));
        assert_eq!(got.answered_by, None);
        assert_eq!(
            s.complete(&t.id, "m/s/dmcp", "again", "screen")
                .unwrap_err()
                .code,
            "task_finished"
        );
        let t2 = s.create(new(None, "y")).unwrap();
        s.answer(&t2.id, "w", "w1:p1", "m/w/coder", "ok").unwrap();
        assert_eq!(
            s.get(&t2.id).unwrap().unwrap().source.as_deref(),
            Some("agent")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn list_filters_wait_cap_and_prune() {
        let (s, dir) = store();
        let a = s.create(new(Some("a"), "x")).unwrap();
        let mut other = new(Some("b"), "y");
        other.owner = Some("v@x".into());
        s.create(other).unwrap();
        s.set_status("a", "running", None).unwrap();
        let mine = s
            .list(&Filter {
                owner: Some("u@x".into()),
                statuses: vec!["queued".into(), "running".into()],
                limit: 10,
                ..Filter::default()
            })
            .unwrap();
        assert_eq!(
            mine.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["a"]
        );
        let queued = s
            .list(&Filter {
                statuses: vec!["queued".into()],
                limit: 10,
                ..Filter::default()
            })
            .unwrap();
        assert_eq!(queued.len(), 1);
        let by_corr = s
            .list(&Filter {
                correlation_id: Some("c-1".into()),
                limit: 10,
                ..Filter::default()
            })
            .unwrap();
        assert_eq!(by_corr.len(), 2);
        let t = s.wait(&a.id, None, Duration::from_millis(350)).unwrap();
        assert_eq!(t.status, "running", "timed out, still open");
        let big = "é".repeat(RESULT_CAP);
        assert!(s.answer("a", "w", "w1:p1", "m/w/coder", &big).unwrap());
        let t = s.wait("a", Some("u@x"), Duration::from_secs(5)).unwrap();
        assert!(t.truncated && t.result.unwrap().len() <= RESULT_CAP);
        assert_eq!(s.prune(Duration::from_secs(3600)).unwrap(), 0, "too new");
        s.conn
            .execute(
                "UPDATE tasks SET updated = updated - 100 WHERE id = 'a'",
                [],
            )
            .unwrap();
        assert_eq!(s.prune(Duration::from_secs(50)).unwrap(), 1, "done and old");
        assert!(s.get("b").unwrap().is_some(), "open tasks are kept");
        let _ = std::fs::remove_dir_all(dir);
    }
}
