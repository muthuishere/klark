//! Wire protocol between a klark machine (the agent) and a klark hub.
//!
//! JSON messages over plain HTTP that the machine dials out to the hub (ADR 0013).
//! Every message is `{"type": "...", ...}`. The hub never receives pane bytes it
//! did not ask for: on connect it gets only the tree, and frames for a pane flow
//! only between `subscribe` and `unsubscribe`.

use serde::{Deserialize, Serialize};

/// v2 adds `list_agents` and `deliver` (agent messaging, ADR 0009).
/// v3 adds `pane_size`, `fetch_asset` and `get_transcript` (ADR 0011).
/// v4 is the same messages over plain HTTP: NDJSON stream down, POST segments up (ADR 0013).
/// v5 adds `get_file` (the chat's file viewer, ADR 0011 G).
pub const PROTOCOL_VERSION: u32 = 5;

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
    /// v3: the size a subscribed pane is observed at. Sent on subscribe and
    /// whenever it changes; never for a pane that is not subscribed.
    PaneSize {
        pane_id: String,
        cols: u16,
        rows: u16,
    },
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
    /// v2: list every agent in every running local session, answered with
    /// `result {id, result: [{session, name, agent, pane_id, status, address}]}`.
    ListAgents {
        id: String,
    },
    /// v2: deliver a message to a local agent as a prompt (ADR 0009), answered
    /// with `result {id, result: {delivered: true}}` or `result {id, error}`.
    Deliver {
        id: String,
        session: String,
        agent: String,
        from: String,
        text: String,
    },
    /// v3: download asset `sha256` from the hub, store it under the uploads
    /// dir and prompt the pane with its path (ADR 0011 B). Answered with
    /// `result {id, result: {path}}` or `result {id, error}`.
    FetchAsset {
        id: String,
        sha256: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        mime: Option<String>,
        size: u64,
        pane_id: String,
        #[serde(default)]
        caption: Option<String>,
    },
    /// v3: the pane agent's conversation, read from its local session log
    /// (or the screen). `after` returns newer items, `before` older ones.
    GetTranscript {
        id: String,
        pane_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<u32>,
    },
    /// v5: a file a chat linked to (absolute, `~/...` or relative to the pane's
    /// cwd). The machine checks it itself (`files::check`) and answers
    /// `result {id, result: {name, path, size, data(base64)}}` or `result {id, error}`.
    GetFile {
        id: String,
        pane_id: String,
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_bytes: Option<u64>,
    },
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
    fn v2_messaging_shapes() {
        assert!(PROTOCOL_VERSION >= 2);
        let msg: HubMessage = serde_json::from_str(r#"{"type":"list_agents","id":"7"}"#).unwrap();
        assert_eq!(msg, HubMessage::ListAgents { id: "7".into() });

        let raw = r#"{"type":"deliver","id":"8","session":"work","agent":"reviewer","from":"m/default/cli","text":"hi"}"#;
        let msg: HubMessage = serde_json::from_str(raw).unwrap();
        assert_eq!(
            msg,
            HubMessage::Deliver {
                id: "8".into(),
                session: "work".into(),
                agent: "reviewer".into(),
                from: "m/default/cli".into(),
                text: "hi".into(),
            }
        );
        assert_eq!(serde_json::to_string(&msg).unwrap(), raw);

        let ok = AgentMessage::Result {
            id: "8".into(),
            result: Some(serde_json::json!({"delivered": true})),
            error: None,
        };
        assert_eq!(
            serde_json::to_string(&ok).unwrap(),
            r#"{"type":"result","id":"8","result":{"delivered":true}}"#
        );
        let err = AgentMessage::Result {
            id: "8".into(),
            result: None,
            error: Some("blocked".into()),
        };
        assert_eq!(
            serde_json::to_string(&err).unwrap(),
            r#"{"type":"result","id":"8","error":"blocked"}"#
        );
    }

    #[test]
    fn v3_shapes() {
        assert!(PROTOCOL_VERSION >= 3);
        let size = AgentMessage::PaneSize {
            pane_id: "p1".into(),
            cols: 80,
            rows: 24,
        };
        assert_eq!(
            serde_json::to_string(&size).unwrap(),
            r#"{"type":"pane_size","pane_id":"p1","cols":80,"rows":24}"#
        );
        let raw = r#"{"type":"fetch_asset","id":"9","sha256":"ab","name":"a.png","mime":"image/png","size":3,"pane_id":"p1","caption":"look"}"#;
        let msg: HubMessage = serde_json::from_str(raw).unwrap();
        assert_eq!(
            msg,
            HubMessage::FetchAsset {
                id: "9".into(),
                sha256: "ab".into(),
                name: Some("a.png".into()),
                mime: Some("image/png".into()),
                size: 3,
                pane_id: "p1".into(),
                caption: Some("look".into()),
            }
        );
        let raw = r#"{"type":"fetch_asset","id":"9","sha256":"ab","name":null,"mime":"image/png","size":3,"pane_id":"p1"}"#;
        let msg: HubMessage = serde_json::from_str(raw).unwrap();
        assert!(matches!(
            msg,
            HubMessage::FetchAsset {
                name: None,
                caption: None,
                ..
            }
        ));
        let msg: HubMessage = serde_json::from_str(
            r#"{"type":"get_transcript","id":"1","pane_id":"p1","before":"42","limit":50}"#,
        )
        .unwrap();
        assert_eq!(
            msg,
            HubMessage::GetTranscript {
                id: "1".into(),
                pane_id: "p1".into(),
                after: None,
                before: Some("42".into()),
                limit: Some(50),
            }
        );
    }

    #[test]
    fn v4_get_file() {
        let msg: HubMessage = serde_json::from_str(
            r#"{"type":"get_file","id":"3","pane_id":"p1","path":"src/a.rs:4","max_bytes":10485760}"#,
        )
        .unwrap();
        assert_eq!(
            msg,
            HubMessage::GetFile {
                id: "3".into(),
                pane_id: "p1".into(),
                path: "src/a.rs:4".into(),
                max_bytes: Some(10_485_760),
            }
        );
    }

    #[test]
    fn mutating_calls_are_refused() {
        assert!(call_allowed("pane.read"));
        assert!(!call_allowed("pane.close"));
        assert!(!call_allowed("command.invoke"));
    }
}
