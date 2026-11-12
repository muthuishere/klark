//! What a feed is on disk: its definition (`<id>.json`, written by the CLI
//! under the store lock) and its status (`<id>.status.json`, written by the
//! worker). Both are plain data; nothing here touches a process.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::schedule::{ErrorTracker, IdleClock, RateLimit};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Stream,
    Follow,
    Poll,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Stream => "stream",
            Mode::Follow => "follow",
            Mode::Poll => "poll",
        }
    }
}

/// One agent subscribed to a feed. Stored as session + agent (its name, or
/// its pane id when it has none), because pane ids change across restarts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sub {
    /// Stable number inside the feed; names the pending file.
    pub n: u32,
    pub session: String,
    pub agent: String,
    pub instruction: String,
    /// Why this subscription stopped itself, if it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<String>,
    /// Stopped by the rate limit twice within 24 hours: only a user shell
    /// may start it again.
    #[serde(default)]
    pub user_only: bool,
    /// Joined a running poll feed and has not had the current output yet.
    #[serde(default)]
    pub fresh: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeedDef {
    pub id: String,
    pub mode: Mode,
    /// The command, or the path for `--follow`.
    pub command: String,
    pub cwd: String,
    /// `--message-gap` (stream, follow) or `--interval` (poll), seconds.
    pub timing_secs: u64,
    /// `lines`, `unified` or a command (poll only).
    pub diff: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    pub max_bytes: usize,
    pub max_per_hour: u32,
    /// 0 = never.
    pub idle_stop_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_secs: Option<u64>,
    pub label: String,
    pub use_small_subagent: bool,
    pub restore: bool,
    /// `user` or `agent <session>/<agent>`.
    pub created_by: String,
    /// Unix seconds.
    pub created_at: u64,
    pub subs: Vec<Sub>,
    pub next_sub: u32,
    /// Why the whole feed stopped itself (command ended, idle, every
    /// subscription rate-stopped); `None` while it may run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<String>,
}

impl FeedDef {
    /// What two `feed add` must share to be the same feed.
    pub fn join_key(&self) -> String {
        join_key(
            self.mode,
            &self.command,
            &self.cwd,
            self.timing_secs,
            &self.diff,
        )
    }

    pub fn active_subs(&self) -> impl Iterator<Item = &Sub> {
        self.subs.iter().filter(|s| s.stopped.is_none())
    }
}

pub fn join_key(mode: Mode, command: &str, cwd: &str, timing: u64, diff: &str) -> String {
    // The timing joins only for poll: two streams of one command with
    // different gaps are still one command.
    let timing = if mode == Mode::Poll { timing } else { 0 };
    format!("{}\0{command}\0{cwd}\0{timing}\0{diff}", mode.as_str())
}

/// Where a `--follow` feed is in its file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FollowPos {
    pub dev: u64,
    pub ino: u64,
    pub offset: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubStatus {
    #[serde(default)]
    pub rate: RateLimit,
    /// Unix seconds of the last delivery to this agent.
    #[serde(default)]
    pub last_sent: Option<u64>,
    /// Unix seconds since the agent was last seen idle, while it is not.
    #[serde(default)]
    pub busy_since: Option<u64>,
    /// Why it waits (agent not found, name ambiguous), if it does.
    #[serde(default)]
    pub waiting: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Status {
    /// `starting`, `running`, `waiting`, `stopped`, `failed`.
    pub state: String,
    #[serde(default)]
    pub detail: Option<String>,
    #[serde(default)]
    pub worker_pid: Option<u32>,
    /// The command's process group, its leader's start time as `ps` prints
    /// it, and its command line, so a leftover can be told from a reused pid.
    #[serde(default)]
    pub pgid: Option<i32>,
    #[serde(default)]
    pub child_start: Option<String>,
    #[serde(default)]
    pub child_cmd: Option<String>,
    #[serde(default)]
    pub last_output: Option<u64>,
    #[serde(default)]
    pub last_sent: Option<u64>,
    #[serde(default)]
    pub filtered: u64,
    #[serde(default)]
    pub batch_seq: u64,
    #[serde(default)]
    pub follow: Option<FollowPos>,
    #[serde(default)]
    pub idle: IdleClock,
    #[serde(default)]
    pub errors: ErrorTracker,
    #[serde(default)]
    pub filter_errors: ErrorTracker,
    #[serde(default)]
    pub subs: BTreeMap<u32, SubStatus>,
}

/// `7d`, `12h`, `30m`, `45s`, a bare number of seconds, or `0` (never).
pub fn parse_duration(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, unit) = match s.find(|c: char| !c.is_ascii_digit()) {
        Some(i) => s.split_at(i),
        None => (s, "s"),
    };
    let n: u64 = num.parse().ok()?;
    let mult = match unit.trim() {
        "s" | "sec" | "secs" => 1,
        "m" | "min" => 60,
        "h" => 3600,
        "d" => 86_400,
        "w" => 7 * 86_400,
        _ => return None,
    };
    n.checked_mul(mult)
}

pub fn format_duration(secs: u64) -> String {
    match secs {
        0 => "never".into(),
        s if s % 86_400 == 0 => format!("{}d", s / 86_400),
        s if s % 3600 == 0 => format!("{}h", s / 3600),
        s if s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("7d"), Some(7 * 86_400));
        assert_eq!(parse_duration("30m"), Some(1800));
        assert_eq!(parse_duration("45"), Some(45));
        assert_eq!(parse_duration("0"), Some(0));
        assert_eq!(parse_duration("soon"), None);
        assert_eq!(parse_duration("3x"), None);
        assert_eq!(format_duration(7 * 86_400), "7d");
        assert_eq!(format_duration(90), "90s");
        assert_eq!(format_duration(0), "never");
    }

    #[test]
    fn stream_timing_does_not_split_a_feed() {
        assert_eq!(
            join_key(Mode::Stream, "tail", "/", 10, "lines"),
            join_key(Mode::Stream, "tail", "/", 30, "lines")
        );
        assert_ne!(
            join_key(Mode::Poll, "ls", "/", 10, "lines"),
            join_key(Mode::Poll, "ls", "/", 30, "lines")
        );
    }
}
