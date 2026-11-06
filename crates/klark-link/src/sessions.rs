//! Exposing every running klark session through one link, with the hub
//! unaware of sessions.
//!
//! The default session's ids go on the wire unchanged (backward compatible).
//! Every other session's pane, tab and workspace ids are namespaced
//! `<session>/<id>`; a session name never contains `/` (it is limited to ASCII
//! letters, digits, `.`, `_` and `-`), so the first `/` splits it off. The
//! trees of all sessions are merged into one `session.snapshot`-shaped value,
//! and each non-default workspace's label names its session ("work · api"),
//! so the hub groups them visibly without knowing what a session is.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value};

/// The name of the session whose ids are not prefixed.
pub const DEFAULT_SESSION: &str = "default";

/// Lists the running sessions on this machine: (name, API socket path). The
/// main crate supplies it; this crate does not know where sessions live.
#[derive(Clone)]
pub struct SessionSource(pub Arc<dyn Fn() -> Vec<(String, PathBuf)> + Send + Sync>);

impl SessionSource {
    pub fn new(f: impl Fn() -> Vec<(String, PathBuf)> + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    pub fn running(&self) -> Vec<(String, PathBuf)> {
        (self.0)()
    }
}

impl std::fmt::Debug for SessionSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionSource(..)")
    }
}

/// A session name as klark accepts it (src/session.rs validate_name).
pub fn valid_session(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// The wire id for a local id of a session.
pub fn encode_id(session: &str, local: &str) -> String {
    if session == DEFAULT_SESSION {
        local.to_string()
    } else {
        format!("{session}/{local}")
    }
}

/// (session, local id) for a wire id. An id without a valid session prefix
/// belongs to the default session.
pub fn decode_id(wire: &str) -> (&str, &str) {
    match wire.split_once('/') {
        Some((session, local)) if valid_session(session) && !local.is_empty() => (session, local),
        _ => (DEFAULT_SESSION, wire),
    }
}

/// Keys of a tree item (or call params) that hold a pane, tab or workspace id.
pub fn is_id_key(key: &str) -> bool {
    ["pane_id", "tab_id", "workspace_id"]
        .iter()
        .any(|k| key == *k || key.ends_with(&format!("_{k}")))
}

/// One session's snapshot with its ids namespaced and its workspaces labelled.
/// Only the `workspaces`, `tabs` and `panes` arrays are kept: other top-level
/// fields (focus, version) describe the default session's own view.
pub fn namespace_snapshot(session: &str, snapshot: &Value) -> Value {
    let mut out = Map::new();
    for key in ["workspaces", "tabs", "panes"] {
        let items: Vec<Value> = snapshot
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|item| {
                let mut item = item.clone();
                if let Some(obj) = item.as_object_mut() {
                    for (k, v) in obj.iter_mut() {
                        if is_id_key(k) {
                            if let Some(id) = v.as_str() {
                                *v = Value::String(encode_id(session, id));
                            }
                        }
                    }
                    if key == "workspaces" && session != DEFAULT_SESSION {
                        let label = obj
                            .get("label")
                            .and_then(Value::as_str)
                            .filter(|l| !l.trim().is_empty())
                            .map(str::to_string)
                            .unwrap_or_else(|| {
                                let n = obj.get("number").and_then(Value::as_i64).unwrap_or(0);
                                format!("workspace {n}")
                            });
                        obj.insert(
                            "label".into(),
                            Value::String(format!("{session} · {label}")),
                        );
                        obj.insert("session".into(), Value::String(session.to_string()));
                    }
                }
                item
            })
            .collect();
        out.insert(key.into(), Value::Array(items));
    }
    Value::Object(out)
}

/// Merge per-session snapshots into one of the same shape. The default
/// session's snapshot is the base (all its fields kept, ids unchanged); every
/// other session's workspaces, tabs and panes are appended, namespaced. Parts
/// are merged in the given order, which callers keep stable (sorted names).
pub fn merge_snapshots(parts: &[(String, Value)]) -> Value {
    let mut base = parts
        .iter()
        .find(|(s, _)| s == DEFAULT_SESSION)
        .map(|(_, v)| v.clone())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()));
    let obj = base.as_object_mut().expect("object");
    for key in ["workspaces", "tabs", "panes"] {
        if !obj.get(key).is_some_and(Value::is_array) {
            obj.insert(key.into(), Value::Array(Vec::new()));
        }
    }
    for (session, snapshot) in parts.iter().filter(|(s, _)| s != DEFAULT_SESSION) {
        let ns = namespace_snapshot(session, snapshot);
        for key in ["workspaces", "tabs", "panes"] {
            if let (Some(Value::Array(dst)), Some(Value::Array(src))) =
                (obj.get_mut(key), ns.get(key))
            {
                dst.extend(src.iter().cloned());
            }
        }
    }
    base
}

/// Call params addressed to a non-default session: (session, params with the
/// prefix stripped). Every id-like value must name the same session; mixing
/// sessions in one call is an error.
pub fn route_params(params: &Value) -> Result<(String, Value), String> {
    let mut params = params.clone();
    let mut session: Option<String> = None;
    if let Some(obj) = params.as_object_mut() {
        for (k, v) in obj.iter_mut() {
            if !(is_id_key(k) || k == "target") {
                continue;
            }
            let Some(wire) = v.as_str() else { continue };
            let (s, local) = decode_id(wire);
            let (s, local) = (s.to_string(), local.to_string());
            match &session {
                Some(prev) if *prev != s => {
                    return Err(format!("ids from sessions {prev} and {s} in one call"))
                }
                _ => session = Some(s),
            }
            *v = Value::String(local);
        }
    }
    Ok((session.unwrap_or_else(|| DEFAULT_SESSION.into()), params))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ids_round_trip() {
        assert_eq!(encode_id("default", "p1"), "p1");
        assert_eq!(encode_id("work", "p1"), "work/p1");
        for (s, id) in [("default", "p1"), ("work", "p1"), ("a.b-c_d", "7")] {
            assert_eq!(decode_id(&encode_id(s, id)), (s, id));
        }
        // Not a session prefix: stays a default-session id.
        assert_eq!(decode_id("p1"), ("default", "p1"));
        assert_eq!(decode_id("bad name/p1"), ("default", "bad name/p1"));
        assert_eq!(decode_id("work/"), ("default", "work/"));
    }

    fn fixture(ws: &str, pane: &str) -> Value {
        json!({
            "focused_pane_id": pane,
            "workspaces": [{"workspace_id": ws, "label": "api", "number": 1, "active_tab_id": "t1"}],
            "tabs": [{"tab_id": "t1", "workspace_id": ws, "label": "main", "number": 1}],
            "panes": [{"pane_id": pane, "workspace_id": ws, "tab_id": "t1", "agent": "claude", "focused": true}]
        })
    }

    #[test]
    fn merge_namespaces_other_sessions() {
        let parts = vec![
            ("default".to_string(), fixture("w1", "p1")),
            ("work".to_string(), fixture("w1", "p1")),
        ];
        let m = merge_snapshots(&parts);
        assert_eq!(m["focused_pane_id"], "p1");
        assert_eq!(m["workspaces"][0]["workspace_id"], "w1");
        assert_eq!(m["workspaces"][0]["label"], "api");
        assert_eq!(m["workspaces"][1]["workspace_id"], "work/w1");
        assert_eq!(m["workspaces"][1]["label"], "work · api");
        assert_eq!(m["workspaces"][1]["active_tab_id"], "work/t1");
        assert_eq!(m["tabs"][1]["tab_id"], "work/t1");
        assert_eq!(m["tabs"][1]["workspace_id"], "work/w1");
        let p = &m["panes"][1];
        assert_eq!(
            (
                p["pane_id"].as_str(),
                p["workspace_id"].as_str(),
                p["tab_id"].as_str()
            ),
            (Some("work/p1"), Some("work/w1"), Some("work/t1"))
        );
        assert_eq!(p["agent"], "claude");
        assert_eq!(m["panes"].as_array().unwrap().len(), 2);

        // Stable rev for equal input, new rev when a session appears or changes.
        let rev = crate::tree_rev(&m);
        assert_eq!(rev, crate::tree_rev(&merge_snapshots(&parts)));
        let only_default = merge_snapshots(&parts[..1]);
        assert_ne!(rev, crate::tree_rev(&only_default));
        let changed = vec![parts[0].clone(), ("work".into(), fixture("w1", "p2"))];
        assert_ne!(rev, crate::tree_rev(&merge_snapshots(&changed)));
    }

    #[test]
    fn merge_without_default_and_unlabelled_workspace() {
        let snap = json!({"workspaces": [{"workspace_id": "w1", "number": 3}], "panes": []});
        let m = merge_snapshots(&[("work".into(), snap)]);
        assert_eq!(m["workspaces"][0]["label"], "work · workspace 3");
        assert_eq!(m["tabs"], json!([]));
    }

    #[test]
    fn routing_strips_the_prefix() {
        assert_eq!(
            route_params(&json!({"pane_id": "work/p3", "source": "visible"})).unwrap(),
            ("work".into(), json!({"pane_id": "p3", "source": "visible"}))
        );
        assert_eq!(
            route_params(&json!({"target": "p3"})).unwrap(),
            ("default".into(), json!({"target": "p3"}))
        );
        assert_eq!(
            route_params(&Value::Null).unwrap(),
            ("default".into(), Value::Null)
        );
        assert!(route_params(&json!({"pane_id": "work/p3", "tab_id": "t1"})).is_err());
    }
}
