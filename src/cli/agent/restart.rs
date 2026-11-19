//! `klark agent restart`: stop an agent and resume the same conversation in
//! the same pane, so whatever was queued in its input goes away (klark#30).
//!
//! Built from existing API calls on the client side, so it works against a
//! running server of any 0.11 version:
//!
//! 1. `agent.get` the target and refuse what a restart would hurt: a dialog
//!    on screen, a git / deploy / install command running under the agent, no
//!    conversation id to resume, no shell under the agent (quitting would close
//!    the pane), or the caller running inside the agent itself.
//! 2. Note the messages delivered since the agent last wrote one, so they can
//!    be listed as possibly unhandled.
//! 3. SIGTERM the agent process (the agent CLIs exit cleanly and keep their
//!    conversation), and `--kill` SIGKILLs it if it does not exit in time.
//! 4. `agent.start` the same kind in the same pane with its resume arguments,
//!    keeping Claude Code's own launch flags.
//! 5. Prompt the agent once with what happened.
//!
//! `--soft` only presses Escape: the current turn stops and, in Claude Code,
//! the queued messages run next. Nothing is restarted.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::api::schema::{
    AgentPromptParams, AgentSendKeysParams, AgentStartParams, AgentTarget, Method,
    PaneProcessInfoParams, Request,
};

const ID: &str = "cli:agent:restart";
const DEFAULT_STOP_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_START_TIMEOUT: Duration = Duration::from_secs(60);
/// The most recent possibly unhandled messages listed.
const MAX_LISTED: usize = 10;
const LISTED_TEXT_CHARS: usize = 200;

pub(crate) struct Restart {
    pub(crate) target: String,
    pub(crate) soft: bool,
    pub(crate) force: bool,
    pub(crate) kill: bool,
    pub(crate) reason: Option<String>,
    pub(crate) timeout_ms: Option<u64>,
    pub(crate) notice: bool,
    pub(crate) notify_senders: bool,
}

pub(crate) fn args() -> Vec<clap::Arg> {
    use clap::{Arg, ArgAction};
    vec![
        Arg::new("target").value_name("TARGET").required(true).help(
            "Pane id or agent name in this session, or session/agent and machine/session/agent",
        ),
        Arg::new("soft").long("soft").action(ArgAction::SetTrue).help(
            "Only press Escape: stop the current turn, keep the agent running (queued messages run next)",
        ),
        Arg::new("force").long("force").action(ArgAction::SetTrue).help(
            "Restart even if the agent shows a dialog or runs a git, deploy or install command",
        ),
        Arg::new("kill").long("kill").action(ArgAction::SetTrue).help(
            "SIGKILL the agent if it has not exited when --timeout runs out",
        ),
        Arg::new("reason")
            .long("reason")
            .value_name("TEXT")
            .help("Why, recorded in the message log and told to the agent"),
        Arg::new("timeout")
            .long("timeout")
            .value_name("MS")
            .value_parser(clap::value_parser!(u64))
            .help("How long the agent gets to exit (default 10000)"),
        Arg::new("no-notice")
            .long("no-notice")
            .action(ArgAction::SetTrue)
            .help("Don't prompt the resumed agent with what happened"),
        Arg::new("notify-senders")
            .long("notify-senders")
            .action(ArgAction::SetTrue)
            .help("Tell the senders of possibly unhandled messages to resend them"),
    ]
}

pub(crate) fn from_matches(matches: &clap::ArgMatches) -> Restart {
    Restart {
        target: matches
            .get_one::<String>("target")
            .cloned()
            .unwrap_or_default(),
        soft: matches.get_flag("soft"),
        force: matches.get_flag("force"),
        kill: matches.get_flag("kill"),
        reason: matches.get_one::<String>("reason").cloned(),
        timeout_ms: matches.get_one::<u64>("timeout").copied(),
        notice: !matches.get_flag("no-notice"),
        notify_senders: matches.get_flag("notify-senders"),
    }
}

pub(super) fn run(params: Restart) -> std::io::Result<i32> {
    if params.soft && (params.kill || params.notify_senders) {
        eprintln!(
            "--soft only presses Escape; it can't be combined with --kill or --notify-senders"
        );
        return Ok(2);
    }
    let target = params.target.clone();
    let response = super::send_routed(&target, ID, |target| restart(&params, &target))?;
    super::super::print_response(&response)
}

fn error(code: &str, message: impl Into<String>) -> Value {
    super::cli_agent_error(ID, code, message)
}

fn request(method: Method) -> std::io::Result<Value> {
    super::super::send_request(&Request {
        id: ID.into(),
        method,
    })
}

/// Runs inside the session that holds `target` (see `send_routed`).
fn restart(params: &Restart, target: &str) -> std::io::Result<Value> {
    let got = request(Method::AgentGet(AgentTarget {
        target: target.to_owned(),
    }))?;
    if got.get("error").is_some() {
        // agent_not_found lets send_routed look in the other sessions.
        return Ok(got);
    }
    let agent = got["result"]["agent"].clone();
    let status = agent["agent_status"]
        .as_str()
        .unwrap_or("unknown")
        .to_owned();
    let mut forced = Vec::new();
    if status == "blocked" {
        if !params.force {
            return Ok(error(
                "agent_blocked",
                format!(
                    "{target} is showing a dialog; answer it (klark agent read {target}) or pass --force"
                ),
            ));
        }
        forced.push("dialog on screen".to_string());
    }

    if params.soft {
        let sent = request(Method::AgentSendKeys(AgentSendKeysParams {
            target: target.to_owned(),
            keys: vec!["esc".into()],
        }))?;
        if sent.get("error").is_some() {
            return Ok(sent);
        }
        let to = agent_address(agent["terminal_id"].as_str().unwrap_or_default(), &agent)
            .unwrap_or_else(|| target.to_owned());
        record(params, &to, "soft");
        return Ok(json!({"id": ID, "result": {
            "type": "agent_restart",
            "mode": "soft",
            "was": status,
            "forced": forced,
            "note": "Escape sent: the current turn stops; Claude Code then runs its queued messages",
        }}));
    }

    hard_restart(params, target, agent, status, forced)
}

#[cfg(not(unix))]
fn hard_restart(
    _params: &Restart,
    _target: &str,
    _agent: Value,
    _status: String,
    _forced: Vec<String>,
) -> std::io::Result<Value> {
    Ok(error(
        "unsupported_platform",
        "klark agent restart needs a Unix process table; use --soft on this platform",
    ))
}

#[cfg(unix)]
fn hard_restart(
    params: &Restart,
    target: &str,
    agent: Value,
    status: String,
    mut forced: Vec<String>,
) -> std::io::Result<Value> {
    let kind = agent["agent"].as_str().unwrap_or_default().to_owned();
    let Some(detected) = crate::detect::parse_agent_label(&kind) else {
        return Ok(error(
            "agent_not_ready",
            format!("{target} is not a known agent kind"),
        ));
    };
    let pane_id = agent["pane_id"].as_str().unwrap_or_default().to_owned();
    let terminal_id = agent["terminal_id"].as_str().unwrap_or_default().to_owned();

    // The conversation to come back to.
    let session = &agent["agent_session"];
    let session_ref = match (
        session["kind"].as_str(),
        session["value"].as_str().filter(|v| !v.is_empty()),
    ) {
        (Some("id"), Some(value)) => crate::agent_resume::AgentSessionRef::id(value),
        (Some("path"), Some(value)) => crate::agent_resume::AgentSessionRef::path(value),
        _ => None,
    };
    let plan = session_ref.as_ref().and_then(|session_ref| {
        crate::agent_resume::plan(
            session["source"].as_str().unwrap_or_default(),
            &kind,
            session_ref,
        )
    });
    let (Some(session_ref), Some(plan)) = (session_ref, plan) else {
        return Ok(error(
            "no_conversation",
            format!("klark doesn't know {target}'s conversation id, so a restart would start a new conversation"),
        ));
    };

    // The agent process, and the shell it returns to.
    let info = request(Method::PaneProcessInfo(PaneProcessInfoParams {
        pane_id: Some(pane_id.clone()),
    }))?;
    if info.get("error").is_some() {
        return Ok(info);
    }
    let info = &info["result"]["process_info"];
    let shell_pid = info["shell_pid"].as_u64().map(|p| p as i32);
    let executable = crate::detect::interactive_agent_executable(detected);
    let agent_process = info["foreground_processes"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| is_agent_process(p, &kind, executable));
    let Some(agent_process) = agent_process else {
        return Ok(error(
            "agent_process_not_found",
            format!("no {kind} process found in pane {pane_id}"),
        ));
    };
    let pid = agent_process["pid"].as_u64().unwrap_or(0) as i32;
    let original_argv: Vec<String> = agent_process["argv"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| a.as_str().map(str::to_owned))
        .collect();

    let table = process::table();
    if process::is_ancestor(&table, pid, std::process::id() as i32) {
        return Ok(error(
            "self_restart",
            "this command runs inside the agent it would restart; run it from another pane",
        ));
    }
    if shell_pid.is_none() || process::parent(&table, pid) != shell_pid {
        return Ok(error(
            "no_shell",
            format!("{target} isn't running under a shell in pane {pane_id}; stopping it would close the pane"),
        ));
    }
    let busy = busy_commands(&table, pid);
    if !busy.is_empty() {
        if !params.force {
            return Ok(error(
                "agent_busy_command",
                format!(
                    "{target} is running {}; wait for it or pass --force",
                    busy.join(", ")
                ),
            ));
        }
        forced.extend(busy.iter().map(|b| format!("running {b}")));
    }

    // Arguments to resume with.
    let (args, not_carried) = resume_args(&kind, &original_argv, &plan.argv);
    let name = agent["name"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| kind.clone());
    let address = agent_address(&terminal_id, &agent);
    let unhandled = address
        .as_deref()
        .map(possibly_unhandled)
        .unwrap_or_default();

    // Stop.
    let stop_timeout = params
        .timeout_ms
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_STOP_TIMEOUT);
    process::signal(pid, libc::SIGTERM);
    let mut signal = "TERM";
    if !process::wait_gone(pid, stop_timeout) {
        if !params.kill {
            return Ok(error(
                "stop_timeout",
                format!(
                    "{target} (pid {pid}) didn't exit within {}ms of SIGTERM; retry with --kill",
                    stop_timeout.as_millis()
                ),
            ));
        }
        for descendant in process::descendants(&table, pid) {
            process::signal(descendant, libc::SIGKILL);
        }
        process::signal(pid, libc::SIGKILL);
        signal = "KILL";
        if !process::wait_gone(pid, Duration::from_secs(5)) {
            return Ok(error(
                "stop_failed",
                format!("{target} (pid {pid}) survived SIGKILL"),
            ));
        }
    }

    // Resume in the same pane.
    let started = start(&name, &kind, &pane_id, &args)?;
    let new_agent = match started {
        Ok(agent) => agent,
        Err(mut failure) => {
            failure["error"]["message"] = format!(
                "{} (stopped; resume by hand: {} {})",
                failure["error"]["message"]
                    .as_str()
                    .unwrap_or("start failed"),
                executable,
                args.join(" ")
            )
            .into();
            return Ok(failure);
        }
    };
    let new_target = new_agent["name"].as_str().unwrap_or(&name).to_owned();

    let who = crate::msg_cmd::my_address();
    record(params, address.as_deref().unwrap_or(target), "restart");
    let mut notice_sent = false;
    if params.notice {
        let text = notice(&who, params.reason.as_deref(), &unhandled);
        let prompted = request(Method::AgentPrompt(AgentPromptParams {
            target: new_target.clone(),
            text,
            wait: None,
        }))?;
        notice_sent = prompted.get("error").is_none();
    }
    let mut notified = Vec::new();
    if params.notify_senders {
        if let Some(address) = &address {
            notified = notify_senders(address, &who, &unhandled);
        }
    }

    Ok(json!({"id": ID, "result": {
        "type": "agent_restart",
        "mode": "restart",
        "was": status,
        "forced": forced,
        "conversation": session_ref.value,
        "stopped_pid": pid,
        "signal": signal,
        "command": std::iter::once(executable.to_owned()).chain(args).collect::<Vec<_>>(),
        "dropped_args": not_carried,
        "possibly_unhandled": unhandled.iter().map(|m| json!({"ts": m.ts, "from": m.from, "text": m.text})).collect::<Vec<_>>(),
        "notice_sent": notice_sent,
        "senders_notified": notified,
        "agent": new_agent,
    }}))
}

#[cfg(unix)]
fn is_agent_process(process: &Value, kind: &str, executable: &str) -> bool {
    let base = |s: &str| s.rsplit('/').next().unwrap_or(s).to_owned();
    let argv0 = process["argv0"].as_str().map(base).unwrap_or_default();
    let name = process["name"].as_str().map(base).unwrap_or_default();
    [argv0.as_str(), name.as_str()]
        .iter()
        .any(|n| *n == kind || *n == executable || n.trim_end_matches(".js") == executable)
        || process["argv"]
            .as_array()
            .into_iter()
            .flatten()
            .take(2)
            .filter_map(Value::as_str)
            .any(|a| base(a) == executable || base(a).trim_end_matches(".js") == executable)
}

/// Starts the agent and waits until it is ready, like `klark agent start`.
#[cfg(unix)]
fn start(
    name: &str,
    kind: &str,
    pane_id: &str,
    args: &[String],
) -> std::io::Result<Result<Value, Value>> {
    // After the agent exits, the server takes a moment to notice (the name
    // stays taken) and the shell to draw its prompt (the pane stays busy).
    let deadline = Instant::now() + Duration::from_secs(15);
    let response = loop {
        let response = request(Method::AgentStart(AgentStartParams {
            name: name.to_owned(),
            kind: kind.to_owned(),
            pane_id: pane_id.to_owned(),
            args: args.to_vec(),
            timeout_ms: Some(DEFAULT_START_TIMEOUT.as_millis() as u64),
        }))?;
        let busy = matches!(
            response["error"]["code"].as_str(),
            Some("agent_pane_busy" | "agent_pane_unavailable" | "agent_name_taken")
        );
        if busy && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        break response;
    };
    if response.get("error").is_some() {
        return Ok(Err(response));
    }
    let Some(terminal_id) = response["result"]["agent"]["terminal_id"].as_str() else {
        return Ok(Err(error(
            "agent_start_failed",
            "agent start response did not include terminal_id",
        )));
    };
    super::wait_for_named_agent(name, pane_id, DEFAULT_START_TIMEOUT, kind, terminal_id)
}

/// The resume command's arguments. Claude Code keeps the flags it was started
/// with (`--dangerously-skip-permissions`, `--model`, ...) minus the ones that
/// pick a conversation or replay a prompt; other agents get their plain resume
/// command and the dropped flags are reported.
fn resume_args(kind: &str, original: &[String], plan: &[String]) -> (Vec<String>, Vec<String>) {
    let resume: Vec<String> = plan.iter().skip(1).cloned().collect();
    let flags: Vec<String> = original.iter().skip(1).cloned().collect();
    if kind != "claude" {
        let dropped = flags.into_iter().filter(|f| !resume.contains(f)).collect();
        return (resume, dropped);
    }
    // Flags that never take a value; any other flag takes the next token
    // unless it is written --flag=value.
    const BOOLEAN: &[&str] = &[
        "--dangerously-skip-permissions",
        "--allow-dangerously-skip-permissions",
        "--verbose",
        "-d",
        "--debug",
        "--ide",
        "--strict-mcp-config",
        "--chrome",
        "--no-chrome",
        "--replay-user-messages",
        "--include-partial-messages",
        "--disable-slash-commands",
    ];
    // Flags that choose the conversation (or print and exit): resume replaces them.
    const SESSION: &[&str] = &[
        "-c",
        "--continue",
        "-r",
        "--resume",
        "--session-id",
        "--fork-session",
        "-p",
        "--print",
        "--from-pr",
    ];
    const SESSION_BOOLEAN: &[&str] = &["-c", "--continue", "--fork-session", "-p", "--print"];
    let mut kept = Vec::new();
    let mut dropped = Vec::new();
    let mut i = 0;
    while i < flags.len() {
        let flag = &flags[i];
        let bare = flag.split('=').next().unwrap_or(flag);
        let has_inline = flag.contains('=');
        if !flag.starts_with('-') {
            dropped.push(flag.clone()); // a prompt: don't replay it
            i += 1;
            continue;
        }
        if SESSION.contains(&bare) {
            dropped.push(flag.clone());
            let takes_value = !has_inline && !SESSION_BOOLEAN.contains(&bare);
            // --resume / -r may be given without an id (opens a picker).
            if takes_value && flags.get(i + 1).is_some_and(|v| !v.starts_with('-')) {
                dropped.push(flags[i + 1].clone());
                i += 1;
            }
            i += 1;
            continue;
        }
        kept.push(flag.clone());
        if !has_inline && !BOOLEAN.contains(&bare) {
            if let Some(value) = flags.get(i + 1).filter(|v| !v.starts_with('-')) {
                kept.push(value.clone());
                i += 1;
            }
        }
        i += 1;
    }
    kept.extend(resume);
    (kept, dropped)
}

/// Commands under the agent that a restart would cut off halfway.
#[cfg(unix)]
fn busy_commands(table: &[process::Proc], pid: i32) -> Vec<String> {
    let mut found: Vec<String> = process::descendants(table, pid)
        .into_iter()
        .filter_map(|p| table.iter().find(|q| q.pid == p))
        .filter_map(|p| risky(&p.command))
        .collect();
    found.sort();
    found.dedup();
    found
}

fn risky(command: &str) -> Option<String> {
    let words: Vec<&str> = command.split_whitespace().collect();
    let base = |w: &str| w.rsplit('/').next().unwrap_or(w).to_owned();
    let program = base(words.first()?);
    let sub = words.get(1).map(|w| w.to_string()).unwrap_or_default();
    let named = |what: &str| Some(format!("{what} ({})", truncate(command, 80)));
    match program.as_str() {
        "git" => {
            const MUTATING: &[&str] = &[
                "rebase",
                "merge",
                "commit",
                "push",
                "pull",
                "am",
                "cherry-pick",
                "revert",
                "reset",
                "checkout",
                "switch",
                "stash",
                "clone",
                "fetch",
                "worktree",
                "tag",
                "filter-branch",
                "gc",
            ];
            // `git -C dir commit` and similar: the subcommand is the first word not an option.
            let sub = words
                .iter()
                .skip(1)
                .scan(false, |skip, w| {
                    let r = if *skip { None } else { Some(*w) };
                    *skip = matches!(*w, "-C" | "-c" | "--git-dir" | "--work-tree");
                    Some(r)
                })
                .flatten()
                .find(|w| !w.starts_with('-'))
                .unwrap_or("");
            MUTATING.contains(&sub).then(|| format!("git {sub}"))
        }
        "kamal" | "terraform" | "tofu" | "helm" | "ansible-playbook" | "flyctl" | "fly"
        | "pulumi" | "vercel" | "netlify" => named(&program),
        "kubectl"
            if matches!(
                sub.as_str(),
                "apply" | "delete" | "rollout" | "replace" | "patch"
            ) =>
        {
            named("kubectl")
        }
        "wrangler" if matches!(sub.as_str(), "deploy" | "publish" | "pages") => named("wrangler"),
        "docker" if matches!(sub.as_str(), "push" | "build" | "compose" | "buildx") => {
            named("docker")
        }
        "npm" | "pnpm" | "yarn" | "bun"
            if matches!(
                sub.as_str(),
                "install" | "i" | "add" | "ci" | "publish" | "update"
            ) =>
        {
            named(&program)
        }
        "cargo" if matches!(sub.as_str(), "publish" | "install") => named("cargo"),
        "pip" | "pip3" | "uv" if matches!(sub.as_str(), "install" | "sync") => named(&program),
        "brew" | "apt" | "apt-get" | "dnf" | "yum" => named(&program),
        "gh" if matches!(sub.as_str(), "release" | "pr" | "repo") => {
            let action = words.get(2).copied().unwrap_or("");
            matches!(action, "create" | "merge" | "upload" | "delete" | "edit")
                .then(|| format!("gh {sub} {action}"))
        }
        _ if words.iter().any(|w| {
            let w = base(w);
            w.contains("deploy") || w.contains("migrate") || w == "release.sh"
        }) =>
        {
            named("a deploy or migration")
        }
        _ => None,
    }
}

fn truncate(text: &str, chars: usize) -> String {
    let mut out: String = text.chars().take(chars).collect();
    if text.chars().count() > chars {
        out.push('…');
    }
    out
}

/// machine/session/handle of the agent, found by its terminal id.
fn agent_address(terminal_id: &str, agent: &Value) -> Option<String> {
    let machine = crate::msg_cmd::my_machine();
    crate::agent_index::list_running()
        .into_iter()
        .find_map(|(session, result)| {
            let result = result.ok()?;
            let found = result["agents"]
                .as_array()?
                .iter()
                .any(|a| a["terminal_id"].as_str() == Some(terminal_id));
            found.then_some(session)
        })
        .map(|session| {
            let handle = agent["name"]
                .as_str()
                .or(agent["pane_id"].as_str())
                .unwrap_or_default();
            format!("{machine}/{session}/{handle}")
        })
}

/// Messages delivered to the agent since it last sent one: klark knows they
/// were typed in, not that they were read, so they are only "possibly"
/// unhandled.
fn possibly_unhandled(address: &str) -> Vec<crate::msg_cmd::LogEntry> {
    let log = crate::msg_cmd::tail_log(&crate::session::data_dir_for(None), 2000);
    unhandled_since_last_sent(log, address)
}

fn unhandled_since_last_sent(
    log: Vec<crate::msg_cmd::LogEntry>,
    address: &str,
) -> Vec<crate::msg_cmd::LogEntry> {
    let last_sent = log
        .iter()
        .rposition(|e| e.dir == "sent" && e.from == address)
        .map(|i| i + 1)
        .unwrap_or(0);
    let mut out: Vec<_> = log
        .into_iter()
        .skip(last_sent)
        .filter(|e| e.dir == "received" && e.to == address && e.ok)
        .collect();
    let skip = out.len().saturating_sub(MAX_LISTED);
    out.drain(..skip);
    for e in &mut out {
        e.text = truncate(&e.text, LISTED_TEXT_CHARS);
    }
    out
}

fn notice(who: &str, reason: Option<&str>, unhandled: &[crate::msg_cmd::LogEntry]) -> String {
    let mut text = format!(
        "[klark restart] You were restarted by {who}{}. This is the same conversation, resumed. \
         Whatever you were running stopped: your current turn, background helper agents and shell commands. \
         Check the state of any work in progress before continuing.",
        reason.map(|r| format!(" ({r})")).unwrap_or_default()
    );
    if unhandled.is_empty() {
        text.push_str(" No klark messages were waiting on you.");
    } else {
        text.push_str(&format!(
            " These {} klark message(s) arrived after your last reply and may not have been handled:",
            unhandled.len()
        ));
        for m in unhandled {
            text.push_str(&format!(
                " [{} from {}] {}",
                m.ts,
                m.from,
                m.text.replace('\n', " ")
            ));
        }
    }
    text
}

fn notify_senders(address: &str, who: &str, unhandled: &[crate::msg_cmd::LogEntry]) -> Vec<String> {
    let mut senders: Vec<&str> = unhandled
        .iter()
        .map(|m| m.from.as_str())
        .filter(|s| *s != who && *s != address)
        .collect();
    senders.sort();
    senders.dedup();
    let Ok(exe) = std::env::current_exe() else {
        return Vec::new();
    };
    senders
        .into_iter()
        .filter(|sender| {
            let text = format!(
                "[klark restart] {address} was restarted by {who}. A message you sent it may not have been handled; resend it if you still need it."
            );
            std::process::Command::new(&exe)
                .args(["msg", "send", sender, &text])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        })
        .map(str::to_owned)
        .collect()
}

/// Records who restarted whom, and why, in the message log.
fn record(params: &Restart, to: &str, mode: &str) {
    let entry = crate::msg_cmd::LogEntry {
        ts: time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default(),
        dir: format!("restart:{mode}"),
        from: crate::msg_cmd::my_address(),
        to: to.to_owned(),
        text: params.reason.clone().unwrap_or_default(),
        ok: true,
        error: None,
    };
    let _ = crate::msg_cmd::append_log(&crate::session::data_dir_for(None), &entry);
}

#[cfg(unix)]
mod process {
    use std::time::{Duration, Instant};

    pub(super) struct Proc {
        pub(super) pid: i32,
        pub(super) ppid: i32,
        pub(super) command: String,
    }

    pub(super) fn table() -> Vec<Proc> {
        let Ok(out) = std::process::Command::new("ps")
            .args(["-A", "-o", "pid=,ppid=,command="])
            .output()
        else {
            return Vec::new();
        };
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| {
                let mut parts = line.trim_start().splitn(2, char::is_whitespace);
                let pid = parts.next()?.parse().ok()?;
                let rest = parts.next()?.trim_start();
                let mut parts = rest.splitn(2, char::is_whitespace);
                let ppid = parts.next()?.parse().ok()?;
                let command = parts.next().unwrap_or("").trim().to_owned();
                Some(Proc { pid, ppid, command })
            })
            .collect()
    }

    pub(super) fn parent(table: &[Proc], pid: i32) -> Option<i32> {
        table.iter().find(|p| p.pid == pid).map(|p| p.ppid)
    }

    pub(super) fn descendants(table: &[Proc], pid: i32) -> Vec<i32> {
        let mut out = Vec::new();
        let mut frontier = vec![pid];
        while let Some(parent) = frontier.pop() {
            for p in table.iter().filter(|p| p.ppid == parent && p.pid != parent) {
                if !out.contains(&p.pid) {
                    out.push(p.pid);
                    frontier.push(p.pid);
                }
            }
        }
        out
    }

    /// Whether `ancestor` is `pid` or one of its ancestors.
    pub(super) fn is_ancestor(table: &[Proc], ancestor: i32, pid: i32) -> bool {
        let mut current = Some(pid);
        let mut hops = 0;
        while let Some(p) = current {
            if p == ancestor {
                return true;
            }
            if p <= 1 || hops > 256 {
                return false;
            }
            current = parent(table, p);
            hops += 1;
        }
        false
    }

    pub(super) fn signal(pid: i32, signal: i32) {
        if pid > 1 {
            // SAFETY: kill(2) with a positive pid only signals that process.
            unsafe {
                libc::kill(pid, signal);
            }
        }
    }

    pub(super) fn alive(pid: i32) -> bool {
        // SAFETY: signal 0 checks existence and permission, delivers nothing.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    pub(super) fn wait_gone(pid: i32, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if !alive(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        !alive(pid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn claude_keeps_its_flags_and_swaps_the_conversation() {
        let plan = s(&["claude", "--resume", "NEW"]);
        let (args, dropped) = resume_args(
            "claude",
            &s(&[
                "claude",
                "--dangerously-skip-permissions",
                "--model",
                "haiku",
                "--resume",
                "OLD",
            ]),
            &plan,
        );
        assert_eq!(
            args,
            s(&[
                "--dangerously-skip-permissions",
                "--model",
                "haiku",
                "--resume",
                "NEW"
            ])
        );
        assert_eq!(dropped, s(&["--resume", "OLD"]));

        let (args, dropped) = resume_args(
            "claude",
            &s(&[
                "claude",
                "-c",
                "--permission-mode=plan",
                "--verbose",
                "fix the bug",
            ]),
            &plan,
        );
        assert_eq!(
            args,
            s(&["--permission-mode=plan", "--verbose", "--resume", "NEW"])
        );
        assert_eq!(
            dropped,
            s(&["-c", "fix the bug"]),
            "the opening prompt is not replayed"
        );

        let (args, _) = resume_args(
            "claude",
            &s(&["claude", "--session-id", "X", "--add-dir", "/a"]),
            &plan,
        );
        assert_eq!(args, s(&["--add-dir", "/a", "--resume", "NEW"]));
    }

    #[test]
    fn other_agents_get_their_resume_command_and_report_dropped_flags() {
        let (args, dropped) = resume_args(
            "codex",
            &s(&["codex", "--yolo"]),
            &s(&["codex", "resume", "ID"]),
        );
        assert_eq!(args, s(&["resume", "ID"]));
        assert_eq!(dropped, s(&["--yolo"]));
    }

    #[test]
    fn risky_commands_are_named_and_reads_are_not() {
        assert_eq!(
            risky("git -C /repo rebase main").as_deref(),
            Some("git rebase")
        );
        assert_eq!(
            risky("/usr/bin/git commit -m x").as_deref(),
            Some("git commit")
        );
        assert_eq!(risky("git status"), None);
        assert_eq!(risky("git log --oneline"), None);
        assert!(risky("kamal deploy -d prod").is_some());
        assert!(risky("bash infra/deploy/prod/deploy.sh --check").is_some());
        assert!(risky("npm install").is_some());
        assert_eq!(risky("npm test"), None);
        assert_eq!(risky("gh pr merge 26").as_deref(), Some("gh pr merge"));
        assert_eq!(risky("gh pr view 26"), None);
        assert_eq!(risky("cargo test"), None);
        assert_eq!(risky("find / -name x"), None);
    }

    fn entry(dir: &str, from: &str, to: &str, text: &str) -> crate::msg_cmd::LogEntry {
        crate::msg_cmd::LogEntry {
            ts: "t".into(),
            dir: dir.into(),
            from: from.into(),
            to: to.into(),
            text: text.into(),
            ok: true,
            error: None,
        }
    }

    #[test]
    fn possibly_unhandled_is_what_arrived_after_the_last_reply() {
        let me = "m/s/agent";
        let log = vec![
            entry("received", "m/s/a", me, "old, answered"),
            entry("sent", me, "m/s/a", "reply"),
            entry("received", "m/s/b", me, "new one"),
            entry("received", "m/s/b", "m/s/other", "not for me"),
            entry("sent", "m/s/c", me, "my own send log, not a delivery"),
            entry("received", "m/s/c", me, "second"),
        ];
        let got: Vec<String> = unhandled_since_last_sent(log, me)
            .into_iter()
            .map(|e| e.text)
            .collect();
        assert_eq!(got, s(&["new one", "second"]));

        let many: Vec<_> = (0..15)
            .map(|i| entry("received", "m/s/a", me, &format!("{i}")))
            .collect();
        let got = unhandled_since_last_sent(many, me);
        assert_eq!(got.len(), MAX_LISTED);
        assert_eq!(got.last().unwrap().text, "14", "the most recent are kept");
    }

    #[test]
    fn the_notice_lists_what_may_be_unhandled() {
        let text = notice(
            "m/s/me",
            Some("stuck"),
            &[entry("received", "m/s/b", "x", "do\nthis")],
        );
        assert!(text.starts_with("[klark restart] You were restarted by m/s/me (stuck)."));
        assert!(text.contains("[t from m/s/b] do this"));
        assert!(notice("w", None, &[]).contains("No klark messages were waiting"));
    }

    #[cfg(unix)]
    #[test]
    fn process_tree_helpers() {
        use process::*;
        let table = vec![
            Proc {
                pid: 10,
                ppid: 1,
                command: "zsh".into(),
            },
            Proc {
                pid: 20,
                ppid: 10,
                command: "claude".into(),
            },
            Proc {
                pid: 30,
                ppid: 20,
                command: "zsh -c x".into(),
            },
            Proc {
                pid: 40,
                ppid: 30,
                command: "git rebase main".into(),
            },
            Proc {
                pid: 50,
                ppid: 1,
                command: "other".into(),
            },
        ];
        assert_eq!(parent(&table, 20), Some(10));
        let mut d = descendants(&table, 20);
        d.sort();
        assert_eq!(d, vec![30, 40]);
        assert!(is_ancestor(&table, 20, 40));
        assert!(!is_ancestor(&table, 20, 50));
        assert_eq!(busy_commands(&table, 20), s(&["git rebase"]));
    }
}
