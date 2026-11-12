use serde::{Deserialize, Serialize};

/// One feed as `feed.list` reports it (ADR 0015). Times are unix seconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FeedInfo {
    pub id: String,
    pub label: String,
    /// `stream`, `follow` or `poll`.
    pub kind: String,
    /// One line per subscriber: `session/agent`, with its own state.
    pub agents: Vec<String>,
    /// What `klark feed list` shows under STATE.
    pub state: String,
    pub running: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_delivery: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FeedTarget {
    pub id: String,
}
