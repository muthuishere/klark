//! `klark expose --agents`: expose only the panes running the listed agents.
//!
//! An entry is matched case-insensitively against each agent of every running
//! session (`agent.list`):
//!
//! 1. `session/x` matches the agent named `x` (or with pane id `x`) in that
//!    session only.
//! 2. A bare `x` matches agents NAMED `x` (`klark agent rename`, `klark msg
//!    name`) in any session.
//! 3. Only when no running agent is named `x` does a bare `x` match the agent
//!    TYPE (`claude`, `devin`, ...): a name always beats a type, so naming one
//!    agent `claude` exposes that agent and not every claude.
//!
//! The filter is re-evaluated against the live agent list, so an agent that
//! starts later or is renamed to a listed name appears, and one that exits or
//! is renamed away disappears.

use std::collections::HashSet;

use serde_json::{Map, Value};

use crate::sessions::is_id_key;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Entry {
    Bare(String),
    Scoped { session: String, agent: String },
}

/// The `--agents` list. Empty never happens: no filter means `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentFilter {
    entries: Vec<Entry>,
    raw: Vec<String>,
}

/// One agent as `agent.list` / `klark msg ls --local --json` report it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentInfo {
    pub session: String,
    pub pane_id: String,
    pub name: Option<String>,
    pub kind: Option<String>,
}

impl AgentInfo {
    /// From an `agent.list` item or a `msg ls --json` item.
    pub fn from_json(session: &str, v: &Value) -> Option<Self> {
        let s = |k: &str| {
            v.get(k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        Some(Self {
            session: s("session").unwrap_or_else(|| session.to_string()),
            pane_id: s("pane_id")?,
            name: s("name"),
            kind: s("agent").or_else(|| s("display_agent")),
        })
    }
}

fn eq(a: Option<&str>, b: &str) -> bool {
    a.is_some_and(|a| a.eq_ignore_ascii_case(b))
}

impl AgentFilter {
    /// Parse `--agents` values: each may hold several comma-separated names.
    pub fn parse<S: AsRef<str>>(values: &[S]) -> Result<Option<Self>, String> {
        let mut entries = Vec::new();
        let mut raw = Vec::new();
        for value in values {
            for item in value.as_ref().split(',') {
                let item = item.trim();
                if item.is_empty() {
                    continue;
                }
                let entry = match item.split('/').collect::<Vec<_>>().as_slice() {
                    [a] => Entry::Bare(a.to_string()),
                    [s, a] if !s.is_empty() && !a.is_empty() => Entry::Scoped {
                        session: s.to_string(),
                        agent: a.to_string(),
                    },
                    _ => {
                        return Err(format!(
                        "bad --agents entry {item:?}: use <name>, <agent-type> or <session>/<name>"
                    ))
                    }
                };
                if !raw.iter().any(|r: &String| r.eq_ignore_ascii_case(item)) {
                    entries.push(entry);
                    raw.push(item.to_string());
                }
            }
        }
        Ok((!entries.is_empty()).then_some(Self { entries, raw }))
    }

    /// The entries as given, for messages ("reviewer, builder").
    pub fn describe(&self) -> String {
        self.raw.join(", ")
    }

    /// Which of `agents` are exposed (same order). Needs the whole list: a
    /// bare entry means a type only when no agent carries it as a name.
    pub fn exposed(&self, agents: &[AgentInfo]) -> Vec<bool> {
        let named = |x: &str| agents.iter().any(|a| eq(a.name.as_deref(), x));
        let rules: Vec<(&Entry, bool)> = self
            .entries
            .iter()
            .map(|e| match e {
                Entry::Bare(x) => (e, named(x)),
                Entry::Scoped { .. } => (e, true),
            })
            .collect();
        agents
            .iter()
            .map(|a| {
                rules.iter().any(|(e, by_name)| match e {
                    Entry::Bare(x) if *by_name => eq(a.name.as_deref(), x),
                    Entry::Bare(x) => eq(a.kind.as_deref(), x),
                    Entry::Scoped { session, agent } => {
                        a.session.eq_ignore_ascii_case(session)
                            && (eq(a.name.as_deref(), agent)
                                || a.pane_id.eq_ignore_ascii_case(agent))
                    }
                })
            })
            .collect()
    }

    /// `(session, local pane id)` of every exposed agent.
    pub fn exposed_panes(&self, agents: &[AgentInfo]) -> HashSet<(String, String)> {
        agents
            .iter()
            .zip(self.exposed(agents))
            .filter(|(_, on)| *on)
            .map(|(a, _)| (a.session.clone(), a.pane_id.clone()))
            .collect()
    }
}

/// A snapshot (`session.snapshot` shape) cut down to `keep` panes: tabs and
/// workspaces left without a pane are dropped, and any id field (top-level
/// focus, a workspace's active tab) that points at something dropped is
/// removed, so nothing hidden leaks through a reference.
pub fn filter_snapshot(snapshot: &Value, keep: &dyn Fn(&str) -> bool) -> Value {
    let Some(obj) = snapshot.as_object() else {
        return snapshot.clone();
    };
    let arr = |k: &str| -> Vec<Value> {
        obj.get(k)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let id = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let panes: Vec<Value> = arr("panes")
        .into_iter()
        .filter(|p| id(p, "pane_id").is_some_and(|i| keep(&i)))
        .collect();
    let pane_ids: HashSet<String> = panes.iter().filter_map(|p| id(p, "pane_id")).collect();
    let tab_ids: HashSet<String> = panes.iter().filter_map(|p| id(p, "tab_id")).collect();
    let mut ws_ids: HashSet<String> = panes.iter().filter_map(|p| id(p, "workspace_id")).collect();
    let tabs: Vec<Value> = arr("tabs")
        .into_iter()
        .filter(|t| id(t, "tab_id").is_some_and(|i| tab_ids.contains(&i)))
        .collect();
    ws_ids.extend(tabs.iter().filter_map(|t| id(t, "workspace_id")));
    let workspaces: Vec<Value> = arr("workspaces")
        .into_iter()
        .filter(|w| id(w, "workspace_id").is_some_and(|i| ws_ids.contains(&i)))
        .collect();
    let known = |k: &str, v: &str| {
        if k == "pane_id" || k.ends_with("_pane_id") {
            pane_ids.contains(v)
        } else if k == "tab_id" || k.ends_with("_tab_id") {
            tab_ids.contains(v)
        } else {
            ws_ids.contains(v)
        }
    };
    let scrub = |m: &mut Map<String, Value>| {
        m.retain(|k, v| !is_id_key(k) || v.as_str().is_none_or(|s| known(k, s)));
    };
    let mut out = obj.clone();
    scrub(&mut out);
    for (k, items) in [("panes", panes), ("tabs", tabs), ("workspaces", workspaces)] {
        let items = items
            .into_iter()
            .map(|mut v| {
                if let Some(m) = v.as_object_mut() {
                    scrub(m);
                }
                v
            })
            .collect();
        out.insert(k.into(), Value::Array(items));
    }
    Value::Object(out)
}

/// A `workspace.list` / `tab.list` / `pane.list` result cut down to what a
/// filtered snapshot keeps. Every array of objects is filtered by its most
/// specific id (pane, else tab, else workspace).
pub fn filter_list_result(result: &Value, filtered_snapshot: &Value) -> Value {
    let ids = |arr: &str, key: &str| -> HashSet<String> {
        filtered_snapshot
            .get(arr)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|v| v.get(key).and_then(Value::as_str).map(str::to_string))
            .collect()
    };
    let (panes, tabs, wss) = (
        ids("panes", "pane_id"),
        ids("tabs", "tab_id"),
        ids("workspaces", "workspace_id"),
    );
    fn walk(v: &Value, f: &dyn Fn(&Value) -> bool) -> Value {
        match v {
            Value::Array(a) => {
                Value::Array(a.iter().filter(|x| f(x)).map(|x| walk(x, f)).collect())
            }
            Value::Object(m) => {
                Value::Object(m.iter().map(|(k, x)| (k.clone(), walk(x, f))).collect())
            }
            other => other.clone(),
        }
    }
    let keep = |item: &Value| {
        let s = |k: &str| item.get(k).and_then(Value::as_str);
        if let Some(p) = s("pane_id") {
            panes.contains(p)
        } else if let Some(t) = s("tab_id") {
            tabs.contains(t)
        } else if let Some(w) = s("workspace_id") {
            wss.contains(w)
        } else {
            true
        }
    };
    walk(result, &keep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn a(session: &str, pane: &str, name: Option<&str>, kind: &str) -> AgentInfo {
        AgentInfo {
            session: session.into(),
            pane_id: pane.into(),
            name: name.map(str::to_string),
            kind: Some(kind.into()),
        }
    }

    fn f(v: &[&str]) -> AgentFilter {
        AgentFilter::parse(v).unwrap().unwrap()
    }

    #[test]
    fn parse_splits_commas_and_rejects_bad_entries() {
        assert_eq!(AgentFilter::parse::<&str>(&[]).unwrap(), None);
        assert_eq!(AgentFilter::parse(&[" , "]).unwrap(), None);
        assert_eq!(
            f(&["reviewer, builder", "work/x", "Reviewer"]).describe(),
            "reviewer, builder, work/x"
        );
        assert!(AgentFilter::parse(&["a/b/c"])
            .unwrap_err()
            .contains("bad --agents entry"));
        assert!(AgentFilter::parse(&["/x"]).is_err());
    }

    #[test]
    fn names_types_and_scoped_addresses_match_case_insensitively() {
        let agents = vec![
            a("default", "p1", Some("reviewer"), "claude"),
            a("work", "p2", Some("builder"), "devin"),
            a("work", "p3", None, "claude"),
            a("ops", "p4", Some("reviewer"), "codex"),
        ];
        assert_eq!(
            f(&["REVIEWER"]).exposed(&agents),
            [true, false, false, true]
        );
        assert_eq!(
            f(&["work/Reviewer"]).exposed(&agents),
            [false, false, false, false]
        );
        assert_eq!(
            f(&["ops/reviewer"]).exposed(&agents),
            [false, false, false, true]
        );
        assert_eq!(
            f(&["work/p3"]).exposed(&agents),
            [false, false, true, false]
        );
        assert_eq!(f(&["Claude"]).exposed(&agents), [true, false, true, false]);
        assert_eq!(
            f(&["devin,reviewer"]).exposed(&agents),
            [true, true, false, true]
        );
        assert_eq!(f(&["ghost"]).exposed(&agents), [false; 4]);
    }

    #[test]
    fn a_name_beats_a_type() {
        let mut agents = vec![
            a("default", "p1", None, "claude"),
            a("work", "p2", None, "claude"),
        ];
        assert_eq!(f(&["claude"]).exposed(&agents), [true, true]);
        agents[1].name = Some("Claude".into());
        assert_eq!(f(&["claude"]).exposed(&agents), [false, true]);
        assert_eq!(
            f(&["claude"]).exposed_panes(&agents),
            HashSet::from([("work".to_string(), "p2".to_string())])
        );
    }

    fn snap() -> Value {
        json!({
            "focused_pane_id": "p2",
            "focused_workspace_id": "w2",
            "workspaces": [
                {"workspace_id": "w1", "active_tab_id": "t1"},
                {"workspace_id": "w2", "active_tab_id": "t3"}
            ],
            "tabs": [
                {"tab_id": "t1", "workspace_id": "w1"},
                {"tab_id": "t2", "workspace_id": "w1"},
                {"tab_id": "t3", "workspace_id": "w2"}
            ],
            "panes": [
                {"pane_id": "p1", "tab_id": "t2", "workspace_id": "w1"},
                {"pane_id": "p2", "tab_id": "t3", "workspace_id": "w2"}
            ]
        })
    }

    #[test]
    fn tree_keeps_only_exposed_panes_and_their_parents() {
        let out = filter_snapshot(&snap(), &|p| p == "p1");
        assert_eq!(
            out["panes"],
            json!([{"pane_id": "p1", "tab_id": "t2", "workspace_id": "w1"}])
        );
        assert_eq!(out["tabs"], json!([{"tab_id": "t2", "workspace_id": "w1"}]));
        // w1 stays; its active tab t1 has no exposed pane, so the reference goes.
        assert_eq!(out["workspaces"], json!([{"workspace_id": "w1"}]));
        assert!(out.get("focused_pane_id").is_none(), "{out}");
        assert!(out.get("focused_workspace_id").is_none(), "{out}");

        let none = filter_snapshot(&snap(), &|_| false);
        assert_eq!(none["panes"], json!([]));
        assert_eq!(none["workspaces"], json!([]));

        let all = filter_snapshot(&snap(), &|_| true);
        assert_eq!(all["focused_pane_id"], "p2");
        assert_eq!(all["workspaces"][1]["active_tab_id"], "t3");
    }

    #[test]
    fn list_results_follow_the_filtered_tree() {
        let kept = filter_snapshot(&snap(), &|p| p == "p2");
        let panes = json!({"panes": [{"pane_id": "p1"}, {"pane_id": "p2"}]});
        assert_eq!(
            filter_list_result(&panes, &kept),
            json!({"panes": [{"pane_id": "p2"}]})
        );
        let tabs = json!({"tabs": [{"tab_id": "t1"}, {"tab_id": "t3"}]});
        assert_eq!(
            filter_list_result(&tabs, &kept),
            json!({"tabs": [{"tab_id": "t3"}]})
        );
        let wss = json!({"workspaces": [{"workspace_id": "w1"}, {"workspace_id": "w2"}]});
        assert_eq!(
            filter_list_result(&wss, &kept),
            json!({"workspaces": [{"workspace_id": "w2"}]})
        );
    }
}
