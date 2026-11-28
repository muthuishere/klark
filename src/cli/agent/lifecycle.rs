//! `klark agent resume | pin | unpin | close`: the agent lifecycle (klark#58).

use serde_json::{json, Value};

use crate::agent_lifecycle as lifecycle;
use crate::api::schema::{
    AgentCloseParams, AgentTarget, EmptyParams, Method, Request, WorkspaceCreateParams,
};

const RESUME_ID: &str = "cli:agent:resume";

fn request(id: &str, method: Method) -> std::io::Result<Value> {
    super::super::send_request(&Request {
        id: id.into(),
        method,
    })
}

// ---------------------------------------------------------------------------
// resume
// ---------------------------------------------------------------------------

pub(crate) fn resume_args() -> Vec<clap::Arg> {
    vec![clap::Arg::new("target").value_name("NAME").required(true).help(
        "Name of a closed agent (see: klark agent list --closed), or session/name",
    )]
}

/// `name` or `[machine/]session/name` into `(session, name)`.
pub(crate) fn split_closed_target(target: &str) -> (Option<String>, String) {
    let mut parts = target.rsplitn(3, '/');
    let name = parts.next().unwrap_or_default().to_owned();
    let session = parts.next().map(str::to_owned);
    (session, name)
}

pub(super) fn run_resume(target: &str) -> std::io::Result<i32> {
    crate::agent_lifecycle::load_keep_closed_from_config();
    let (session, name) = split_closed_target(target);
    let records = lifecycle::read_closed(&lifecycle::closed_log_path(), lifecycle::now_secs());
    // Without a session in the target prefer this session's agent, then the newest anywhere.
    let current = super::current_session_name();
    let record = session
        .as_deref()
        .and_then(|s| lifecycle::find_closed(&records, &name, Some(s)))
        .or_else(|| {
            if session.is_some() {
                None
            } else {
                current
                    .as_deref()
                    .and_then(|s| lifecycle::find_closed(&records, &name, Some(s)))
                    .or_else(|| lifecycle::find_closed(&records, &name, None))
            }
        });
    let Some(record) = record.cloned() else {
        return super::super::print_response(&super::cli_agent_error(
            RESUME_ID,
            "closed_agent_not_found",
            format!("no closed agent named {name} (see: klark agent list --closed)"),
        ));
    };
    let response = super::send_to_session(&record.session, name, RESUME_ID, &|_| {
        resume_in_session(&record)
    })?;
    super::super::print_response(&response)
}

/// Runs against the session that holds the closed agent.
fn resume_in_session(record: &lifecycle::ClosedRecord) -> std::io::Result<Value> {
    let name = record.name.clone().unwrap_or_else(|| record.agent.clone());
    let err = |code: &str, message: String| super::cli_agent_error(RESUME_ID, code, message);
    if crate::detect::parse_agent_label(&record.agent).is_none() {
        return Ok(err(
            "unknown_agent_kind",
            format!("{} is not an agent kind this klark can start", record.agent),
        ));
    }
    let running = request(
        RESUME_ID,
        Method::AgentGet(AgentTarget {
            target: name.clone(),
        }),
    )?;
    if running.get("error").is_none() {
        return Ok(err(
            "agent_name_taken",
            format!("an agent named {name} is already running in session {}", record.session),
        ));
    }
    if let Some(folder) = &record.folder {
        if !std::path::Path::new(folder).is_dir() {
            return Ok(err(
                "folder_missing",
                format!("{name}'s folder {folder} no longer exists"),
            ));
        }
    }
    let resume = lifecycle::resume_args(record);
    let note = match (&resume, &record.conversation_id) {
        (Some(_), _) => None,
        (None, None) => Some("no conversation id was recorded: started a new conversation".to_owned()),
        (None, Some(_)) => Some(format!(
            "{} has no resume command: started a new conversation",
            record.agent
        )),
    };
    let created = request(
        RESUME_ID,
        Method::WorkspaceCreate(WorkspaceCreateParams {
            source_workspace_id: None,
            cwd: record.folder.clone(),
            focus: false,
            label: record.workspace.clone(),
            env: Default::default(),
        }),
    )?;
    if created.get("error").is_some() {
        return Ok(created);
    }
    let Some(pane_id) = created["result"]["root_pane"]["pane_id"].as_str() else {
        return Ok(err(
            "resume_failed",
            "workspace.create did not return a pane".into(),
        ));
    };
    let args = resume.clone().unwrap_or_default();
    let started = match super::restart::start(&name, &record.agent, pane_id, &args)? {
        Ok(agent) => agent,
        Err(failure) => return Ok(failure),
    };
    let _ = lifecycle::remove_closed(&lifecycle::closed_log_path(), record, lifecycle::now_secs());
    let mut command = vec![crate::detect::parse_agent_label(&record.agent)
        .map(crate::detect::interactive_agent_executable)
        .unwrap_or(&record.agent)
        .to_owned()];
    command.extend(args);
    let mut result = json!({
        "type": "agent_resume",
        "resumed": resume.is_some(),
        "conversation": record.conversation_id,
        "command": command,
        "agent": started,
    });
    if let Some(note) = note {
        result["note"] = note.into();
    }
    Ok(json!({"id": RESUME_ID, "result": result}))
}

// ---------------------------------------------------------------------------
// targets across sessions
// ---------------------------------------------------------------------------

/// Every agent of every running session on this machine, annotated like `agent list -a`.
fn all_agents() -> Vec<Value> {
    let machine = crate::msg_cmd::my_machine();
    let mut agents = Vec::new();
    for (session, result) in crate::agent_index::list_running() {
        if let Ok(mut result) = result {
            if let Some(list) = result["agents"].as_array_mut() {
                for mut agent in list.drain(..) {
                    super::annotate_agent(&mut agent, &machine, &session);
                    agents.push(agent);
                }
            }
        }
    }
    agents
}

/// The agent a TARGET names among annotated rows: an address, `session/name`,
/// `session/pane`, or a bare name or pane id (this session's first).
pub(crate) fn match_target<'a>(
    agents: &'a [Value],
    target: &str,
    current: Option<&str>,
) -> Result<&'a Value, String> {
    let handle_of = |a: &Value| {
        a["name"]
            .as_str()
            .or_else(|| a["pane_id"].as_str())
            .unwrap_or_default()
            .to_owned()
    };
    let is_match = |a: &Value| {
        let session = a["session"].as_str().unwrap_or_default();
        let name = a["name"].as_str().unwrap_or_default();
        let pane = a["pane_id"].as_str().unwrap_or_default();
        a["address"].as_str() == Some(target)
            || format!("{session}/{name}") == target
            || format!("{session}/{pane}") == target
            || (!name.is_empty() && name == target)
            || pane == target
    };
    let found: Vec<&Value> = agents.iter().filter(|a| is_match(a)).collect();
    let in_current: Vec<&Value> = found
        .iter()
        .copied()
        .filter(|a| a["session"].as_str() == current)
        .collect();
    let pool = if in_current.len() == 1 { in_current } else { found };
    match pool.as_slice() {
        [one] => Ok(one),
        [] => Err(format!("no agent matches {target} (see: klark agent list -a)")),
        many => Err(format!(
            "{target} matches {} agents; use one of: {}",
            many.len(),
            many.iter()
                .map(|a| {
                    a["address"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| handle_of(a))
                })
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

// ---------------------------------------------------------------------------
// pin / unpin
// ---------------------------------------------------------------------------

pub(crate) fn pin_args() -> Vec<clap::Arg> {
    vec![clap::Arg::new("target").value_name("TARGET").required(true).help(
        "Agent name (or session/name): pinned agents are never closed by `klark agent close --idle-for` or [agents] close_idle_after",
    )]
}

pub(super) fn run_pin(target: &str, pin: bool) -> std::io::Result<i32> {
    let id = if pin { "cli:agent:pin" } else { "cli:agent:unpin" };
    let agents = all_agents();
    let current = super::current_session_name();
    let agent = match match_target(&agents, target, current.as_deref()) {
        Ok(agent) => agent.clone(),
        // Unpinning a closed or renamed agent by its pin key still works.
        Err(_) if !pin && lifecycle::load_pins(&lifecycle::pins_path()).contains(target) => {
            json!({"name": target.rsplit('/').next(), "session": target.split('/').next()})
        }
        Err(message) => {
            return super::super::print_response(&super::cli_agent_error(
                id,
                "agent_not_found",
                message,
            ))
        }
    };
    let (Some(name), Some(session)) = (agent["name"].as_str(), agent["session"].as_str()) else {
        return super::super::print_response(&super::cli_agent_error(
            id,
            "agent_unnamed",
            "only named agents can be pinned (klark agent rename <TARGET> <NAME>)",
        ));
    };
    let key = lifecycle::pin_key(session, name);
    let path = lifecycle::pins_path();
    let mut pins = lifecycle::load_pins(&path);
    let changed = if pin { pins.insert(key.clone()) } else { pins.remove(&key) };
    lifecycle::save_pins(&path, &pins)?;
    super::super::print_response(&json!({"id": id, "result": {
        "type": if pin { "agent_pin" } else { "agent_unpin" },
        "agent": key,
        "pinned": pin,
        "changed": changed,
    }}))
}

// ---------------------------------------------------------------------------
// close
// ---------------------------------------------------------------------------

const CLOSE_ID: &str = "cli:agent:close";

pub(crate) struct CloseCmd {
    pub(crate) target: Option<String>,
    pub(crate) idle_for: Option<String>,
    pub(crate) all_sessions: bool,
    pub(crate) dry_run: bool,
    pub(crate) json: bool,
}

pub(crate) fn close_args() -> Vec<clap::Arg> {
    use clap::{Arg, ArgAction};
    vec![
        Arg::new("target")
            .value_name("TARGET")
            .required(false)
            .conflicts_with("idle-for")
            .help("Agent to close: pane id or name, session/agent, machine/session/agent"),
        Arg::new("idle-for")
            .long("idle-for")
            .value_name("DURATION")
            .required_unless_present("target")
            .help("Close every idle or done agent that has been so for at least this long (30m, 3h, 1d)"),
        Arg::new("all-sessions")
            .long("all-sessions")
            .short('a')
            .action(ArgAction::SetTrue)
            .requires("idle-for")
            .help("With --idle-for: every running session on this machine, not just this one"),
        Arg::new("dry-run")
            .long("dry-run")
            .action(ArgAction::SetTrue)
            .help("Say what would be closed; close nothing"),
        Arg::new("json")
            .long("json")
            .action(ArgAction::SetTrue)
            .help("Print JSON instead of text"),
    ]
}

pub(crate) fn close_from(matches: &clap::ArgMatches) -> CloseCmd {
    CloseCmd {
        target: matches.get_one::<String>("target").cloned(),
        idle_for: matches.get_one::<String>("idle-for").cloned(),
        all_sessions: matches.get_flag("all-sessions"),
        dry_run: matches.get_flag("dry-run"),
        json: matches.get_flag("json"),
    }
}

fn row_summary(agent: &Value) -> Value {
    json!({
        "name": agent["name"],
        "session": agent["session"],
        "pane_id": agent["pane_id"],
        "agent": agent["agent"],
        "status": agent["agent_status"],
        "status_seconds": agent["status_seconds"],
        "address": agent["address"],
    })
}

/// Closes one agent through the session that owns it.
fn close_one(agent: &Value, reason: &str) -> std::io::Result<Value> {
    let session = agent["session"].as_str().unwrap_or_default().to_owned();
    let pane = agent["pane_id"].as_str().unwrap_or_default().to_owned();
    let reason = reason.to_owned();
    super::send_to_session(&session, pane, CLOSE_ID, &|pane| {
        request(
            CLOSE_ID,
            Method::AgentClose(AgentCloseParams {
                target: pane,
                reason: Some(reason.clone()),
            }),
        )
    })
}

pub(super) fn run_close(cmd: CloseCmd) -> std::io::Result<i32> {
    let current = super::current_session_name();
    let self_pane = std::env::var("KLARK_PANE_ID").ok();
    let result = if let Some(target) = &cmd.target {
        let agents = all_agents();
        let agent = match match_target(&agents, target, current.as_deref()) {
            Ok(agent) => agent.clone(),
            Err(message) => {
                return super::super::print_response(&super::cli_agent_error(
                    CLOSE_ID,
                    "agent_not_found",
                    message,
                ))
            }
        };
        let is_self = self_pane.as_deref() == agent["pane_id"].as_str()
            && current.as_deref() == agent["session"].as_str();
        if is_self {
            return super::super::print_response(&super::cli_agent_error(
                CLOSE_ID,
                "self_close",
                "this command runs inside that agent; close it from another pane",
            ));
        }
        let mut failed = Vec::new();
        let mut closed = Vec::new();
        if !cmd.dry_run {
            let response = close_one(&agent, lifecycle::REASON_USER)?;
            if response.get("error").is_some() {
                failed.push(json!({"agent": row_summary(&agent), "error": response["error"]}));
            } else {
                closed.push(row_summary(&agent));
            }
        } else {
            closed.push(row_summary(&agent));
        }
        json!({"type": "agent_close", "dry_run": cmd.dry_run, "reason": lifecycle::REASON_USER,
               "closed": closed, "skipped": [], "failed": failed})
    } else {
        let text = cmd.idle_for.clone().unwrap_or_default();
        let idle_for = match lifecycle::parse_duration(&text) {
            Ok(secs) => secs,
            Err(message) => {
                eprintln!("--idle-for: {message}");
                return Ok(2);
            }
        };
        let agents = if cmd.all_sessions {
            all_agents()
        } else {
            let machine = crate::msg_cmd::my_machine();
            let response = request(CLOSE_ID, Method::AgentList(EmptyParams::default()))?;
            if response.get("error").is_some() {
                return super::super::print_response(&response);
            }
            let session = current.clone().unwrap_or_default();
            let mut list = response["result"]["agents"].as_array().cloned().unwrap_or_default();
            for agent in &mut list {
                super::annotate_agent(agent, &machine, &session);
            }
            list
        };
        let pins = lifecycle::load_pins(&lifecycle::pins_path());
        let self_ref = match (current.as_deref(), self_pane.as_deref()) {
            (Some(s), Some(p)) => Some((s, p)),
            _ => None,
        };
        let selection = lifecycle::select_closable(&agents, idle_for, &pins, self_ref);
        let mut closed = Vec::new();
        let mut failed = Vec::new();
        for agent in &selection.close {
            if cmd.dry_run {
                closed.push(row_summary(agent));
                continue;
            }
            let response = close_one(agent, lifecycle::REASON_IDLE_CLOSE)?;
            if response.get("error").is_some() {
                failed.push(json!({"agent": row_summary(agent), "error": response["error"]}));
            } else {
                closed.push(row_summary(agent));
            }
        }
        let skipped: Vec<Value> = selection
            .skipped
            .iter()
            .map(|(agent, why)| {
                let mut row = row_summary(agent);
                row["reason"] = (*why).into();
                row
            })
            .collect();
        json!({"type": "agent_close", "dry_run": cmd.dry_run, "reason": lifecycle::REASON_IDLE_CLOSE,
               "idle_for_seconds": idle_for, "closed": closed, "skipped": skipped, "failed": failed})
    };
    let failed = result["failed"].as_array().is_some_and(|f| !f.is_empty());
    if cmd.json {
        println!("{}", json!({"id": CLOSE_ID, "result": result}));
    } else {
        print!("{}", close_report(&result));
    }
    Ok(i32::from(failed))
}

/// Text form of a close result.
pub(crate) fn close_report(result: &Value) -> String {
    let dry = result["dry_run"].as_bool().unwrap_or(false);
    let label = |a: &Value| {
        let name = a["name"].as_str().or_else(|| a["pane_id"].as_str()).unwrap_or("?");
        format!("{}/{}", a["session"].as_str().unwrap_or("?"), name)
    };
    let mut out = String::new();
    for a in result["closed"].as_array().into_iter().flatten() {
        let idle = a["status_seconds"]
            .as_u64()
            .map(|s| format!(" ({} {})", a["status"].as_str().unwrap_or(""), lifecycle::format_duration(s)))
            .unwrap_or_default();
        out.push_str(&format!(
            "{} {}{idle}\n",
            if dry { "would close" } else { "closed" },
            label(a)
        ));
    }
    for a in result["skipped"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "skipped {}: {}\n",
            label(a),
            a["reason"].as_str().unwrap_or("")
        ));
    }
    for f in result["failed"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "failed {}: {}\n",
            label(&f["agent"]),
            f["error"]["message"].as_str().unwrap_or("error")
        ));
    }
    if out.is_empty() {
        out.push_str("nothing to close\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(name: &str, session: &str, pane: &str) -> Value {
        json!({"name": name, "session": session, "pane_id": pane,
               "address": format!("m/{session}/{name}")})
    }

    #[test]
    fn targets_resolve_across_sessions() {
        let agents = vec![
            agent("a", "main", "p1"),
            agent("a", "other", "p2"),
            agent("b", "other", "p3"),
        ];
        let name = |r: Result<&Value, String>| r.map(|a| a["pane_id"].as_str().unwrap().to_owned());
        // This session's agent wins for a bare name.
        assert_eq!(name(match_target(&agents, "a", Some("main"))), Ok("p1".into()));
        // Ambiguous elsewhere.
        let err = match_target(&agents, "a", Some("third")).unwrap_err();
        assert!(err.contains("m/main/a") && err.contains("m/other/a"), "{err}");
        assert_eq!(name(match_target(&agents, "b", Some("main"))), Ok("p3".into()));
        assert_eq!(name(match_target(&agents, "other/a", None)), Ok("p2".into()));
        assert_eq!(name(match_target(&agents, "m/other/a", None)), Ok("p2".into()));
        assert!(match_target(&agents, "nope", None).is_err());
    }

    #[test]
    fn close_report_lists_closed_and_skipped() {
        let result = json!({"dry_run": true,
            "closed": [{"name":"a","session":"s","status":"idle","status_seconds":11520}],
            "skipped": [{"name":"p","session":"s","reason":"pinned"}],
            "failed": []});
        let text = close_report(&result);
        assert!(text.contains("would close s/a (idle 3h12m)"), "{text}");
        assert!(text.contains("skipped s/p: pinned"), "{text}");
        assert_eq!(close_report(&json!({"closed":[],"skipped":[],"failed":[]})), "nothing to close\n");
    }

    #[test]
    fn closed_targets_split_into_session_and_name() {
        assert_eq!(split_closed_target("bob"), (None, "bob".into()));
        assert_eq!(
            split_closed_target("main/bob"),
            (Some("main".into()), "bob".into())
        );
        assert_eq!(
            split_closed_target("mac/main/bob"),
            (Some("main".into()), "bob".into())
        );
    }
}
