//! The agents of every klark session on this machine (ADR 0009).
//!
//! One place that knows how to enumerate running sessions, list their agents,
//! parse `[machine/][session/]agent` addresses and resolve them to a session
//! socket. Used by `klark msg` and by `klark agent` cross-session targets.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ---------------------------------------------------------------- addresses

/// `[machine/][session/]agent`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Address {
    pub machine: Option<String>,
    pub session: Option<String>,
    pub agent: String,
}

pub fn parse_address(s: &str) -> Result<Address, String> {
    let parts: Vec<&str> = s.trim().split('/').collect();
    if parts.iter().any(|p| p.trim().is_empty()) {
        return Err(format!(
            "bad address {s:?}: use <agent>, <session>/<agent> or <machine>/<session>/<agent>"
        ));
    }
    let owned = |p: &str| p.trim().to_string();
    match parts.as_slice() {
        [a] => Ok(Address {
            machine: None,
            session: None,
            agent: owned(a),
        }),
        [s, a] => Ok(Address {
            machine: None,
            session: Some(owned(s)),
            agent: owned(a),
        }),
        [m, s, a] => Ok(Address {
            machine: Some(owned(m)),
            session: Some(owned(s)),
            agent: owned(a),
        }),
        _ => Err(format!(
            "bad address {s:?}: at most <machine>/<session>/<agent>"
        )),
    }
}

/// One agent in a local session; `--local --json` prints exactly these fields
/// (the shape the link answers `list_agents` with).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalAgent {
    pub session: String,
    pub name: Option<String>,
    pub agent: Option<String>,
    pub pane_id: String,
    pub status: String,
    pub address: String,
    /// The git worktree the agent works in, when its folder is a linked
    /// worktree (`git worktree add`), named by the worktree's folder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    /// The folder the agent works in (full path).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// `worktree` for a linked git worktree, `main` for a main checkout;
    /// absent outside git.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout: Option<String>,
    /// The checked-out branch; absent when detached or unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The terminal title the agent sets: usually what it is working on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Where a folder sits in git: its checkout kind, linked worktree name and
/// branch. Read from `.git` and `HEAD` files only (no `git` process).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Checkout {
    /// `"worktree"` or `"main"`.
    pub kind: &'static str,
    /// The linked worktree's folder name.
    pub worktree: Option<String>,
    pub branch: Option<String>,
}

/// The checkout containing `dir`: the nearest folder with a `.git`. A `.git`
/// file pointing into another repository's `worktrees/` is a linked worktree;
/// a `.git` directory (or a submodule's `.git` file) is a main checkout.
/// `None` outside git.
pub fn checkout(dir: &Path) -> Option<Checkout> {
    let mut cur = Some(dir);
    while let Some(d) = cur {
        let git = d.join(".git");
        if git.is_dir() {
            return Some(Checkout {
                kind: "main",
                worktree: None,
                branch: head_branch(&git),
            });
        }
        if git.is_file() {
            let gitdir = std::fs::read_to_string(&git).ok().and_then(|text| {
                let path = text
                    .lines()
                    .next()?
                    .strip_prefix("gitdir:")?
                    .trim()
                    .to_string();
                Some(d.join(path))
            });
            let linked = gitdir.as_deref().is_some_and(|gitdir| {
                gitdir
                    .parent()
                    .and_then(Path::file_name)
                    .is_some_and(|name| name == "worktrees")
            });
            return Some(Checkout {
                kind: if linked { "worktree" } else { "main" },
                worktree: linked
                    .then(|| d.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .flatten(),
                branch: gitdir.as_deref().and_then(head_branch),
            });
        }
        cur = d.parent();
    }
    None
}

fn head_branch(gitdir: &Path) -> Option<String> {
    let head = std::fs::read_to_string(gitdir.join("HEAD")).ok()?;
    head.trim()
        .strip_prefix("ref: refs/heads/")
        .map(str::to_string)
}

impl LocalAgent {
    pub(crate) fn matches(&self, agent: &str) -> bool {
        self.name.as_deref() == Some(agent) || self.pane_id == agent
    }
    /// The agent part of its address: the name if it has one, else the pane id.
    pub(crate) fn handle(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.pane_id)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    Local { session: String, agent: String },
    Remote { to: String },
}

/// Resolve an address per ADR 0009: short forms search local agents only; a
/// full address on this machine is local; another machine goes via the hub.
pub fn resolve(addr: &Address, agents: &[LocalAgent], my_machine: &str) -> Result<Route, String> {
    if let Some(machine) = &addr.machine {
        if machine != my_machine {
            return Ok(Route::Remote {
                to: format!(
                    "{machine}/{}/{}",
                    addr.session.as_deref().unwrap_or("default"),
                    addr.agent
                ),
            });
        }
    }
    let found: Vec<&LocalAgent> = agents
        .iter()
        .filter(|a| addr.session.as_deref().is_none_or(|s| a.session == s))
        .filter(|a| a.matches(&addr.agent))
        .collect();
    match found.as_slice() {
        [one] => Ok(Route::Local {
            session: one.session.clone(),
            agent: one.pane_id.clone(),
        }),
        [] => {
            let place = match &addr.session {
                Some(s) => format!("in session {s}"),
                None => "in any running session".to_string(),
            };
            Err(format!(
                "no agent {} {place} on this machine (see: klark msg ls)",
                addr.agent
            ))
        }
        many => Err(format!(
            "{} is ambiguous; use one of: {}",
            addr.agent,
            many.iter()
                .map(|a| format!("{}/{}", a.session, a.pane_id))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

// ---------------------------------------------------------------- local sessions

pub(crate) fn session_socket(session: &str) -> PathBuf {
    let name = (session != crate::session::DEFAULT_SESSION_NAME).then_some(session);
    crate::session::api_socket_path_for(name)
}

pub(crate) fn call(socket: &Path, method: &str, params: Value) -> Result<Value, String> {
    let bin = std::env::current_exe().unwrap_or_else(|_| "klark".into());
    klark_link::upstream::Upstream::new(socket.to_path_buf(), bin).call(method, params)
}

pub(crate) fn agents_from_list(session: &str, machine: &str, result: &Value) -> Vec<LocalAgent> {
    result
        .get("agents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let pane_id = a.get("pane_id")?.as_str()?.to_string();
            let s = |k: &str| {
                a.get(k)
                    .and_then(Value::as_str)
                    .filter(|v| !v.is_empty())
                    .map(str::to_string)
            };
            let cwd = s("foreground_cwd").or_else(|| s("cwd"));
            let checkout = cwd.as_deref().and_then(|c| checkout(Path::new(c)));
            let mut agent = LocalAgent {
                session: session.to_string(),
                name: s("name"),
                agent: s("agent").or_else(|| s("display_agent")),
                pane_id,
                status: s("agent_status").unwrap_or_else(|| "unknown".into()),
                address: String::new(),
                worktree: checkout.as_ref().and_then(|c| c.worktree.clone()),
                cwd,
                checkout: checkout.as_ref().map(|c| c.kind.to_string()),
                branch: checkout.and_then(|c| c.branch),
                title: s("terminal_title_stripped"),
            };
            agent.address = format!("{machine}/{session}/{}", agent.handle());
            Some(agent)
        })
        .collect()
}

pub(crate) fn running_sessions() -> Vec<String> {
    crate::session::list_sessions()
        .unwrap_or_default()
        .into_iter()
        .filter(|s| s.running)
        .map(|s| s.name)
        .collect()
}

/// `agent.list` of every running session, in `klark session list` order. A
/// session that fails to answer is reported, not skipped silently.
pub(crate) fn list_running() -> Vec<(String, Result<Value, String>)> {
    running_sessions()
        .into_iter()
        .map(|session| {
            let result = call(&session_socket(&session), "agent.list", json!({}));
            (session, result)
        })
        .collect()
}

pub(crate) fn local_agents(machine: &str) -> Vec<LocalAgent> {
    let mut out = Vec::new();
    for (session, result) in list_running() {
        match result {
            Ok(result) => out.extend(agents_from_list(&session, machine, &result)),
            Err(e) => eprintln!("klark msg: session {session}: {e}"),
        }
    }
    out
}

// ---------------------------------------------------------------- klark agent targets

/// A `klark agent` target that names a session: `session/agent` or
/// `machine/session/agent` where machine is this machine. `Ok(None)` for a
/// bare target (pane id or name), which keeps today's current-session meaning.
pub(crate) fn split_agent_target(
    target: &str,
    my_machine: &str,
) -> Result<Option<(String, String)>, String> {
    if !target.contains('/') {
        return Ok(None);
    }
    let addr = parse_address(target)?;
    if let Some(machine) = &addr.machine {
        if machine != my_machine {
            return Err(format!(
                "{target}: machine {machine} is not this machine ({my_machine}); klark agent reaches \
                 sessions on this machine only. To message it via the hub: klark msg send {target} \"<text>\""
            ));
        }
    }
    let session = addr
        .session
        .unwrap_or_else(|| crate::session::DEFAULT_SESSION_NAME.to_string());
    Ok(Some((session, addr.agent)))
}

/// A bare name that is not in the current session: find it in the others.
/// `Ok(None)` when no other session has it; an error listing every full
/// address when more than one does. Pane ids never match: they only mean a
/// pane of the current session.
pub(crate) fn find_elsewhere<'a>(
    name: &str,
    agents: &'a [LocalAgent],
    current_session: Option<&str>,
) -> Result<Option<&'a LocalAgent>, String> {
    let found: Vec<&LocalAgent> = agents
        .iter()
        .filter(|a| Some(a.session.as_str()) != current_session)
        .filter(|a| a.name.as_deref() == Some(name))
        .collect();
    match found.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(one)),
        many => Err(format!(
            "agent target {name} is not in this session and is ambiguous across sessions; use one of: {}",
            many.iter()
                .map(|a| a.address.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(session: &str, name: Option<&str>, pane: &str) -> LocalAgent {
        let mut a = LocalAgent {
            session: session.into(),
            name: name.map(str::to_string),
            agent: Some("claude".into()),
            pane_id: pane.into(),
            status: "idle".into(),
            address: String::new(),
            worktree: None,
            cwd: None,
            checkout: None,
            branch: None,
            title: None,
        };
        a.address = format!("m1/{session}/{}", a.handle());
        a
    }

    #[test]
    fn agent_targets_split_into_session_and_agent() {
        let s = |t: &str| split_agent_target(t, "m1");
        assert_eq!(s("rev"), Ok(None));
        assert_eq!(s("w1:p2"), Ok(None));
        assert_eq!(s("work/rev"), Ok(Some(("work".into(), "rev".into()))));
        assert_eq!(s("m1/work/rev"), Ok(Some(("work".into(), "rev".into()))));
        assert_eq!(s("default/p1"), Ok(Some(("default".into(), "p1".into()))));
        let other = s("m2/work/rev").unwrap_err();
        assert!(
            other.contains("machine m2 is not this machine (m1)")
                && other.contains("klark msg send m2/work/rev"),
            "{other}"
        );
        assert!(s("work/").unwrap_err().contains("bad address"));
        assert!(s("a/b/c/d").unwrap_err().contains("at most"));
    }

    #[test]
    fn bare_names_fall_back_to_other_sessions_unless_ambiguous() {
        let agents = vec![
            agent("default", Some("rev"), "p1"),
            agent("work", Some("rev"), "p2"),
            agent("ops", Some("rev"), "p3"),
            agent("work", Some("coder"), "p4"),
            agent("ops", None, "p5"),
        ];
        let found = find_elsewhere("coder", &agents, Some("default")).unwrap();
        assert_eq!(found.map(|a| a.address.as_str()), Some("m1/work/coder"));
        // The current session is excluded: it already answered "not found".
        assert_eq!(find_elsewhere("coder", &agents, Some("work")), Ok(None));
        assert_eq!(find_elsewhere("p5", &agents, Some("default")), Ok(None));
        assert_eq!(find_elsewhere("ghost", &agents, None), Ok(None));
        let amb = find_elsewhere("rev", &agents, Some("default")).unwrap_err();
        assert!(
            amb.contains("ambiguous") && amb.contains("m1/work/rev") && amb.contains("m1/ops/rev"),
            "{amb}"
        );
        assert!(!amb.contains("m1/default/rev"), "{amb}");
    }
}

#[cfg(test)]
mod worktree_tests {
    #[test]
    fn linked_worktree_is_named_main_checkout_is_not() {
        let base = std::env::temp_dir().join(format!("klark-wt-{}", std::process::id()));
        let main = base.join("repo");
        let wt = base.join("repo-feature");
        std::fs::create_dir_all(main.join(".git")).unwrap();
        std::fs::create_dir_all(wt.join("src")).unwrap();
        std::fs::write(
            wt.join(".git"),
            "gitdir: ../repo/.git/worktrees/repo-feature\n",
        )
        .unwrap();
        assert_eq!(super::checkout(&main).unwrap().worktree, None);
        assert_eq!(
            super::checkout(&wt.join("src"))
                .unwrap()
                .worktree
                .as_deref(),
            Some("repo-feature")
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn checkout_reads_kind_worktree_and_branch_from_files() {
        let base = std::env::temp_dir().join(format!("klark-co-{}", std::process::id()));
        let main = base.join("repo");
        let wt = base.join("repo-feature");
        let sub = main.join("vendor/lib");
        let gitdir = main.join(".git/worktrees/repo-feature");
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::create_dir_all(main.join(".git/modules/lib")).unwrap();
        std::fs::create_dir_all(wt.join("src")).unwrap();
        std::fs::create_dir_all(sub.join("src")).unwrap();
        std::fs::write(main.join(".git/HEAD"), "ref: refs/heads/master\n").unwrap();
        std::fs::write(gitdir.join("HEAD"), "ref: refs/heads/feat/x\n").unwrap();
        std::fs::write(main.join(".git/modules/lib/HEAD"), "0123abcd\n").unwrap();
        std::fs::write(
            wt.join(".git"),
            "gitdir: ../repo/.git/worktrees/repo-feature\n",
        )
        .unwrap();
        std::fs::write(sub.join(".git"), "gitdir: ../../.git/modules/lib\n").unwrap();

        let c = super::checkout(&main.join("src")).unwrap();
        assert_eq!(
            (c.kind, c.worktree, c.branch.as_deref()),
            ("main", None, Some("master"))
        );
        let c = super::checkout(&wt.join("src")).unwrap();
        assert_eq!(c.kind, "worktree");
        assert_eq!(c.worktree.as_deref(), Some("repo-feature"));
        assert_eq!(c.branch.as_deref(), Some("feat/x"));
        // A submodule is its own main checkout; a detached HEAD has no branch.
        let c = super::checkout(&sub).unwrap();
        assert_eq!((c.kind, c.worktree, c.branch), ("main", None, None));
        assert_eq!(super::checkout(&base), None);
        let _ = std::fs::remove_dir_all(&base);
    }
}
