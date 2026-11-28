//! `klark agent resume | pin | unpin | close`: the agent lifecycle (klark#58).

use serde_json::{json, Value};

use crate::agent_lifecycle as lifecycle;
use crate::api::schema::{
    AgentCloseParams, AgentStartParams, AgentTarget, EmptyParams, Method, Request,
    WorkspaceCreateParams,
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

#[allow(dead_code)]
fn _unused(_: AgentCloseParams, _: AgentStartParams, _: EmptyParams) {}

#[cfg(test)]
mod tests {
    use super::*;

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
