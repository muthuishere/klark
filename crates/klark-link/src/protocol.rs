//! Wire protocol between a klark machine (the agent) and a klark hub.
//!
//! JSON text frames over one WebSocket that the machine dials out to the hub.
//! Every message is `{"type": "...", ...}`. The hub never receives pane bytes it
//! did not ask for: on connect it gets only the tree, and frames for a pane flow
//! only between `subscribe` and `unsubscribe`.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

/// Machine -> hub.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentMessage {
    /// First message on every connection.
    Hello {
        protocol: u32,
        machine_id: String,
        machine_name: String,
        klark_version: String,
    },
    /// Answer to `get_tree`: the workspace/tab/pane tree (`session.snapshot`)
    /// with a revision the hub hands back next time.
    Tree {
        rev: String,
        snapshot: serde_json::Value,
    },
    /// Answer to `get_tree` when the tree still has the revision the hub holds:
    /// nothing is resent.
    TreeUnchanged {
        rev: String,
    },
    /// Terminal output for a subscribed pane, base64 ANSI bytes.
    Frame {
        pane_id: String,
        bytes: String,
    },
    /// A subscribed pane's stream ended.
    Closed {
        pane_id: String,
        reason: String,
    },
    /// Answer to a hub `call`.
    Result {
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        result: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    Pong,
}

/// Hub -> machine.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HubMessage {
    /// Someone is viewing this pane: start streaming it.
    Subscribe {
        pane_id: String,
    },
    /// Nobody is viewing this pane any more: stop streaming it.
    Unsubscribe {
        pane_id: String,
    },
    /// Type text into a pane (`pane.send_text`).
    Input {
        pane_id: String,
        text: String,
    },
    /// Send logical keys to a pane (`pane.send_keys`), e.g. ["enter"], ["esc"], ["ctrl+c"].
    Keys {
        pane_id: String,
        keys: Vec<String>,
    },
    /// A read-only or allow-listed socket API call, answered with `result`.
    Call {
        id: String,
        method: String,
        #[serde(default)]
        params: serde_json::Value,
    },
    /// Resend the tree now.
    /// Pull the tree. Machines never push it: the hub asks only while someone
    /// is looking (ADR 0001, D11).
    GetTree {
        #[serde(default)]
        known_rev: Option<String>,
    },
    Ping,
}

/// Socket API methods the hub may invoke through `call`. Anything that can
/// create, close, or rewrite state goes through a dedicated message instead,
/// so a hub can never drive arbitrary klark commands.
pub const ALLOWED_CALLS: &[&str] = &[
    "ping",
    "session.snapshot",
    "pane.read",
    "agent.read",
    "pane.get",
    "workspace.list",
    "tab.list",
    "pane.list",
];

pub fn call_allowed(method: &str) -> bool {
    ALLOWED_CALLS.contains(&method)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_messages_round_trip() {
        let raw = r#"{"type":"subscribe","pane_id":"p1"}"#;
        let msg: HubMessage = serde_json::from_str(raw).unwrap();
        assert_eq!(
            msg,
            HubMessage::Subscribe {
                pane_id: "p1".into()
            }
        );

        let raw = r#"{"type":"call","id":"1","method":"pane.read"}"#;
        let msg: HubMessage = serde_json::from_str(raw).unwrap();
        assert!(matches!(msg, HubMessage::Call { params, .. } if params.is_null()));
    }

    #[test]
    fn agent_messages_are_tagged() {
        let msg = AgentMessage::Frame {
            pane_id: "p1".into(),
            bytes: "aGk=".into(),
        };
        assert_eq!(
            serde_json::to_string(&msg).unwrap(),
            r#"{"type":"frame","pane_id":"p1","bytes":"aGk="}"#
        );
    }

    #[test]
    fn mutating_calls_are_refused() {
        assert!(call_allowed("pane.read"));
        assert!(!call_allowed("pane.close"));
        assert!(!call_allowed("command.invoke"));
    }
}
