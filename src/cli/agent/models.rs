//! `klark agent models` / `klark agent model`: list and switch the model of
//! a running agent (klark#52).
//!
//! What each agent type supports comes from built-in defaults (claude, devin)
//! merged with `models.toml` in the klark config dir; a user entry replaces
//! the fields it sets and can add new agent types:
//!
//! ```toml
//! [codex]
//! models = ["gpt-5", "o3"]
//! restart_args = ["-m", "{model}"]
//! ```
//!
//! A switch types the type's `switch` template plus Enter when the agent is
//! idle and checks the pane for `confirm`; otherwise (or with `--restart`)
//! it restarts the agent on the same conversation with `restart_args`.
//! A type whose switch command also saves the model as the CLI's default
//! (`switch_persists_default = true`; Claude's `/model` does, in
//! ~/.claude/settings.json) switches by restart unless `--live` is passed,
//! because the restart flag applies to that session only.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::api::schema::{
    AgentPromptParams, AgentReadParams, AgentStatus, AgentTarget, AgentWaitParams, Method,
    ReadFormat, ReadSource, Request,
};

const ID: &str = "cli:agent:model";
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ModelSpec {
    /// Known models (used when there is no list_command, and to warn).
    #[serde(default)]
    pub(crate) models: Option<Vec<String>>,
    /// Shell command that prints the models (devin's JSON or one per line).
    #[serde(default)]
    pub(crate) list_command: Option<String>,
    /// Text typed into an idle agent, `{model}` replaced.
    #[serde(default)]
    pub(crate) switch: Option<String>,
    /// Pane text that shows the live switch took effect.
    #[serde(default)]
    pub(crate) confirm: Option<String>,
    /// Flags added to the resume command, `{model}` replaced.
    #[serde(default)]
    pub(crate) restart_args: Option<Vec<String>>,
    /// The `switch` command also saves the model as the CLI's default for
    /// new sessions, so the default is to switch by restart instead.
    #[serde(default)]
    pub(crate) switch_persists_default: Option<bool>,
}

impl ModelSpec {
    fn overlay(&mut self, other: ModelSpec) {
        if other.models.is_some() {
            self.models = other.models;
        }
        if other.list_command.is_some() {
            self.list_command = other.list_command;
        }
        if other.switch.is_some() {
            self.switch = other.switch;
        }
        if other.confirm.is_some() {
            self.confirm = other.confirm;
        }
        if other.restart_args.is_some() {
            self.restart_args = other.restart_args;
        }
        if other.switch_persists_default.is_some() {
            self.switch_persists_default = other.switch_persists_default;
        }
    }
}

pub(crate) fn defaults() -> BTreeMap<String, ModelSpec> {
    let strs = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    BTreeMap::from([
        (
            "claude".to_string(),
            ModelSpec {
                models: Some(strs(&["opus", "sonnet", "haiku", "fable"])),
                list_command: None,
                switch: Some("/model {model}".into()),
                confirm: Some("Set model to".into()),
                restart_args: Some(strs(&["--model", "{model}"])),
                switch_persists_default: Some(true),
            },
        ),
        (
            "devin".to_string(),
            ModelSpec {
                models: None,
                list_command: Some("devin models list --format json".into()),
                switch: None,
                confirm: None,
                restart_args: Some(strs(&["--model", "{model}"])),
                switch_persists_default: None,
            },
        ),
    ])
}

/// Defaults overlaid with the user's file contents.
pub(crate) fn parse_config(text: &str) -> Result<BTreeMap<String, ModelSpec>, String> {
    let user: BTreeMap<String, ModelSpec> =
        toml::from_str(text).map_err(|e| format!("models.toml: {e}"))?;
    let mut all = defaults();
    for (kind, spec) in user {
        all.entry(kind).or_default().overlay(spec);
    }
    Ok(all)
}

fn config_path() -> std::path::PathBuf {
    crate::config::config_dir().join("models.toml")
}

fn load() -> Result<BTreeMap<String, ModelSpec>, String> {
    match std::fs::read_to_string(config_path()) {
        Ok(text) => parse_config(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(defaults()),
        Err(e) => Err(format!("{}: {e}", config_path().display())),
    }
}

pub(crate) fn expand(template: &str, model: &str) -> String {
    template.replace("{model}", model)
}

pub(crate) fn expand_all(templates: &[String], model: &str) -> Vec<String> {
    templates.iter().map(|t| expand(t, model)).collect()
}

/// Model ids from a list command's output: devin's `--format json`, or text
/// with one model id first on each line (indented lines when any are).
pub(crate) fn parse_model_list(output: &str) -> Vec<String> {
    if let Ok(value) = serde_json::from_str::<Value>(output) {
        let mut out = Vec::new();
        for family in value["families"].as_array().into_iter().flatten() {
            for variant in family["variants"].as_array().into_iter().flatten() {
                if let Some(id) = variant["model_uid"].as_str() {
                    out.push(id.to_owned());
                }
            }
            for alias in family["aliases"].as_array().into_iter().flatten() {
                if let Some(a) = alias.as_str() {
                    out.push(a.to_owned());
                }
            }
        }
        if let Some(list) = value.as_array() {
            out.extend(list.iter().filter_map(|v| v.as_str().map(str::to_owned)));
        }
        return out;
    }
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    let indented = lines
        .iter()
        .any(|l| l.starts_with(' ') || l.starts_with('\t'));
    lines
        .into_iter()
        .filter(|l| !indented || l.starts_with(' ') || l.starts_with('\t'))
        .filter_map(|l| l.split_whitespace().next())
        .filter(|w| !w.ends_with(':'))
        .map(str::to_owned)
        .collect()
}

fn list_models(spec: &ModelSpec) -> Result<Vec<String>, String> {
    if let Some(command) = &spec.list_command {
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .output()
            .map_err(|e| format!("{command}: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "{command} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        return Ok(parse_model_list(&String::from_utf8_lossy(&out.stdout)));
    }
    Ok(spec.models.clone().unwrap_or_default())
}

/// How a switch is carried out.
#[derive(Debug, PartialEq)]
pub(crate) enum SwitchPlan {
    /// Type this text and press Enter.
    Live(String),
    /// Restart on the same conversation with these extra flags.
    Restart(Vec<String>),
}

pub(crate) fn plan_switch(
    spec: &ModelSpec,
    model: &str,
    force_restart: bool,
    force_live: bool,
) -> Result<SwitchPlan, String> {
    if force_restart && force_live {
        return Err("--live and --restart can't be used together".into());
    }
    let persists = spec.switch_persists_default == Some(true);
    let live = !force_restart && (force_live || !persists || spec.restart_args.is_none());
    match (&spec.switch, &spec.restart_args) {
        (Some(template), _) if live => Ok(SwitchPlan::Live(expand(template, model))),
        (_, Some(args)) => Ok(SwitchPlan::Restart(expand_all(args, model))),
        (Some(_), None) => {
            Err("this agent type has no restart_args, so it can't switch by restart".into())
        }
        (None, None) => {
            Err("this agent type has neither switch nor restart_args in models.toml".into())
        }
    }
}

pub(crate) struct ModelsCmd {
    pub(crate) target: String,
    pub(crate) json: bool,
}

pub(crate) struct ModelCmd {
    pub(crate) target: String,
    pub(crate) model: String,
    pub(crate) restart: bool,
    pub(crate) live: bool,
    pub(crate) wait: bool,
    pub(crate) force: bool,
    pub(crate) json: bool,
}

pub(crate) fn models_args() -> Vec<clap::Arg> {
    use clap::{Arg, ArgAction};
    vec![
        Arg::new("target")
            .value_name("TARGET")
            .required(true)
            .help("Agent (pane id, name, session/agent) or an agent type such as claude or devin"),
        Arg::new("json")
            .long("json")
            .action(ArgAction::SetTrue)
            .help("JSON output"),
    ]
}

pub(crate) fn model_args() -> Vec<clap::Arg> {
    use clap::{Arg, ArgAction};
    vec![
        Arg::new("target").value_name("TARGET").required(true).help(
            "Pane id or agent name in this session, or session/agent and machine/session/agent",
        ),
        Arg::new("model").value_name("MODEL").required(true).help("Model to switch to"),
        Arg::new("restart").long("restart").action(ArgAction::SetTrue).help(
            "Restart on the same conversation with the model flag instead of typing the switch command (the default for Claude, whose /model also changes its saved default model)",
        ),
        Arg::new("live").long("live").action(ArgAction::SetTrue).conflicts_with("restart").help(
            "Type the switch command even if it also changes the agent CLI's saved default model (Claude's /model does)",
        ),
        Arg::new("wait").long("wait").action(ArgAction::SetTrue).help(
            "If the agent is working, wait until it is idle instead of refusing",
        ),
        Arg::new("force").long("force").action(ArgAction::SetTrue).help(
            "Switch even if the model is not in the agent type's list",
        ),
        Arg::new("json").long("json").action(ArgAction::SetTrue).help("JSON output"),
    ]
}

pub(crate) fn models_from(m: &clap::ArgMatches) -> ModelsCmd {
    ModelsCmd {
        target: m.get_one::<String>("target").cloned().unwrap_or_default(),
        json: m.get_flag("json"),
    }
}

pub(crate) fn model_from(m: &clap::ArgMatches) -> ModelCmd {
    ModelCmd {
        target: m.get_one::<String>("target").cloned().unwrap_or_default(),
        model: m.get_one::<String>("model").cloned().unwrap_or_default(),
        restart: m.get_flag("restart"),
        live: m.get_flag("live"),
        wait: m.get_flag("wait"),
        force: m.get_flag("force"),
        json: m.get_flag("json"),
    }
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

fn fail(json: bool, value: &Value) -> std::io::Result<i32> {
    if json {
        println!("{value}");
    } else {
        eprintln!("{}", value["error"]["message"].as_str().unwrap_or("failed"));
    }
    Ok(1)
}

/// The agent type: TARGET itself when it names a configured type, else the
/// kind of the agent it addresses.
fn resolve_kind(
    target: &str,
    config: &BTreeMap<String, ModelSpec>,
) -> std::io::Result<Result<String, Value>> {
    if config.contains_key(target) {
        return Ok(Ok(target.to_owned()));
    }
    let got = super::send_routed(target, ID, |t| {
        request(Method::AgentGet(AgentTarget { target: t }))
    })?;
    if got.get("error").is_some() {
        return Ok(Err(got));
    }
    Ok(Ok(got["result"]["agent"]["agent"]
        .as_str()
        .unwrap_or("unknown")
        .to_owned()))
}

pub(super) fn run_models(cmd: ModelsCmd) -> std::io::Result<i32> {
    let config = match load() {
        Ok(c) => c,
        Err(e) => return fail(cmd.json, &error("models_config_invalid", e)),
    };
    let kind = match resolve_kind(&cmd.target, &config)? {
        Ok(k) => k,
        Err(e) => return fail(cmd.json, &e),
    };
    let Some(spec) = config.get(&kind) else {
        return fail(
            cmd.json,
            &error(
                "unknown_agent_type",
                format!(
                    "no model config for agent type {kind}; add a [{kind}] table to {}",
                    config_path().display()
                ),
            ),
        );
    };
    let models = match list_models(spec) {
        Ok(m) => m,
        Err(e) => return fail(cmd.json, &error("models_list_failed", e)),
    };
    if cmd.json {
        println!("{}", json!({"agent_type": kind, "models": models}));
    } else {
        for m in models {
            println!("{m}");
        }
    }
    Ok(0)
}

pub(super) fn run_model(cmd: ModelCmd) -> std::io::Result<i32> {
    let config = match load() {
        Ok(c) => c,
        Err(e) => return fail(cmd.json, &error("models_config_invalid", e)),
    };
    let target = cmd.target.clone();
    let response = super::send_routed(&target, ID, |t| switch(&cmd, &config, &t))?;
    if cmd.json || response.get("error").is_some() {
        return super::super::print_response(&response);
    }
    let r = &response["result"];
    if let Some(w) = r["warning"].as_str() {
        eprintln!("warning: {w}");
    }
    println!(
        "{} switched to {} ({})",
        cmd.target,
        cmd.model,
        r["mode"].as_str().unwrap_or("")
    );
    Ok(0)
}

fn read_tail(target: &str) -> std::io::Result<String> {
    let r = request(Method::AgentRead(AgentReadParams {
        target: target.to_owned(),
        source: ReadSource::Visible,
        lines: Some(40),
        format: ReadFormat::Text,
        strip_ansi: true,
    }))?;
    Ok(r["result"]["read"]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned())
}

/// Runs inside the session that holds `target` (see `send_routed`).
fn switch(
    cmd: &ModelCmd,
    config: &BTreeMap<String, ModelSpec>,
    target: &str,
) -> std::io::Result<Value> {
    let got = request(Method::AgentGet(AgentTarget {
        target: target.to_owned(),
    }))?;
    if got.get("error").is_some() {
        return Ok(got);
    }
    let agent = &got["result"]["agent"];
    let kind = agent["agent"].as_str().unwrap_or("unknown").to_owned();
    let Some(spec) = config.get(&kind) else {
        return Ok(error(
            "unknown_agent_type",
            format!(
                "no model config for agent type {kind}; add a [{kind}] table to {}",
                config_path().display()
            ),
        ));
    };
    let mut warning = None;
    match list_models(spec) {
        Ok(models) if !models.is_empty() && !models.contains(&cmd.model) => {
            if spec.list_command.is_some() && !cmd.force {
                return Ok(error("unknown_model", format!(
                    "{} is not one of {kind}'s models (klark agent models {target}); pass --force to try anyway", cmd.model
                )));
            }
            warning = Some(format!("{} is not in {kind}'s model list", cmd.model));
        }
        _ => {}
    }
    let plan = match plan_switch(spec, &cmd.model, cmd.restart, cmd.live) {
        Ok(p) => p,
        Err(e) => return Ok(error("model_switch_unsupported", format!("{kind}: {e}"))),
    };
    if cmd.live && spec.switch_persists_default == Some(true) {
        let note = format!(
            "--live: {kind}'s switch command also changes the {kind} CLI's saved default model for new sessions"
        );
        eprintln!("warning: {note}");
        warning = Some(match warning {
            Some(w) => format!("{w}; {note}"),
            None => note,
        });
    }

    let status = agent["agent_status"].as_str().unwrap_or("unknown");
    if status == "blocked" {
        return Ok(error(
            "agent_blocked",
            format!("{target} is showing a dialog; answer it first"),
        ));
    }
    if status == "working" {
        if !cmd.wait {
            return Ok(error(
                "agent_working",
                format!("{target} is working; retry when idle or pass --wait"),
            ));
        }
        let waited = request(Method::AgentWait(AgentWaitParams {
            target: target.to_owned(),
            until: vec![AgentStatus::Idle, AgentStatus::Done],
            timeout_ms: None,
        }))?;
        if waited.get("error").is_some() {
            return Ok(waited);
        }
    }

    match plan {
        SwitchPlan::Live(text) => {
            let confirm = spec.confirm.clone();
            let before = read_tail(target)?;
            let count = |s: &str| {
                confirm
                    .as_deref()
                    .map(|c| s.matches(c).count())
                    .unwrap_or(0)
            };
            let prompted = request(Method::AgentPrompt(AgentPromptParams {
                target: target.to_owned(),
                text: text.clone(),
                wait: None,
            }))?;
            if prompted.get("error").is_some() {
                return Ok(prompted);
            }
            let Some(confirm_text) = confirm.clone() else {
                return Ok(
                    json!({"id": ID, "result": {"type": "agent_model", "mode": "live", "typed": text, "confirmed": false, "warning": warning}}),
                );
            };
            let deadline = Instant::now() + CONFIRM_TIMEOUT;
            loop {
                std::thread::sleep(Duration::from_millis(400));
                let after = read_tail(target)?;
                let lower = after.to_lowercase();
                if count(&after) > count(&before) {
                    let line = after
                        .lines()
                        .rev()
                        .find(|l| l.contains(&confirm_text))
                        .unwrap_or("")
                        .trim()
                        .to_owned();
                    return Ok(
                        json!({"id": ID, "result": {"type": "agent_model", "mode": "live", "typed": text, "confirmed": true, "pane_says": line, "warning": warning}}),
                    );
                }
                if lower.contains("select model") && lower.contains("esc to cancel") {
                    return Ok(error("model_picker_open", format!(
                        "{target} opened its model picker instead of switching (model {} not recognised?); press esc (klark agent send-keys {target} esc) or pick by hand", cmd.model
                    )));
                }
                if Instant::now() > deadline {
                    return Ok(error("model_switch_unconfirmed", format!(
                        "typed `{text}` into {target} but did not see \"{confirm_text}\"; check klark agent read {target}"
                    )));
                }
            }
        }
        SwitchPlan::Restart(extra_args) => {
            let params = super::restart::Restart {
                target: target.to_owned(),
                soft: false,
                force: false,
                kill: false,
                reason: Some(format!("model switch to {}", cmd.model)),
                timeout_ms: None,
                notice: false,
                notify_senders: false,
                extra_args,
            };
            let mut response = super::restart::restart(&params, target)?;
            if let Some(result) = response.get_mut("result") {
                result["type"] = "agent_model".into();
                result["mode"] = "restart".into();
                result["warning"] = warning.into();
            }
            Ok(response)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_cover_claude_and_devin() {
        let d = defaults();
        assert_eq!(d["claude"].switch.as_deref(), Some("/model {model}"));
        assert!(d["devin"].list_command.is_some());
        assert!(d["devin"].switch.is_none());
    }

    #[test]
    fn user_file_overrides_fields_and_adds_types() {
        let c = parse_config(
            "[claude]\nmodels = [\"opus\"]\n[codex]\nmodels = [\"gpt-5\"]\nrestart_args = [\"-m\", \"{model}\"]\n",
        )
        .unwrap();
        assert_eq!(c["claude"].models, Some(vec!["opus".to_string()]));
        assert_eq!(
            c["claude"].switch.as_deref(),
            Some("/model {model}"),
            "unset fields keep the default"
        );
        assert_eq!(
            plan_switch(&c["codex"], "o3", false, false).unwrap(),
            SwitchPlan::Restart(vec!["-m".into(), "o3".into()])
        );
        assert!(parse_config("[claude]\nbogus = 1\n").is_err());
        assert!(parse_config("").unwrap().contains_key("devin"));
    }

    #[test]
    fn plans_live_or_restart() {
        let d = defaults();
        assert_eq!(
            plan_switch(&d["claude"], "opus", false, true).unwrap(),
            SwitchPlan::Live("/model opus".into())
        );
        assert_eq!(
            plan_switch(&d["claude"], "opus", true, false).unwrap(),
            SwitchPlan::Restart(vec!["--model".into(), "opus".into()])
        );
        assert_eq!(
            plan_switch(&d["devin"], "swe-2-1", false, false).unwrap(),
            SwitchPlan::Restart(vec!["--model".into(), "swe-2-1".into()])
        );
        assert!(plan_switch(&ModelSpec::default(), "x", false, false).is_err());
    }

    #[test]
    fn persisting_type_restarts_by_default_and_live_on_request() {
        let d = defaults();
        assert_eq!(d["claude"].switch_persists_default, Some(true));
        assert_eq!(
            plan_switch(&d["claude"], "opus", false, false).unwrap(),
            SwitchPlan::Restart(vec!["--model".into(), "opus".into()])
        );
        assert_eq!(
            plan_switch(&d["claude"], "opus", false, true).unwrap(),
            SwitchPlan::Live("/model opus".into())
        );
        assert!(plan_switch(&d["claude"], "opus", true, true).is_err());
        // no restart_args: live is the only way, even for a persisting type
        let mut only_live = d["claude"].clone();
        only_live.restart_args = None;
        assert_eq!(
            plan_switch(&only_live, "opus", false, false).unwrap(),
            SwitchPlan::Live("/model opus".into())
        );
    }

    #[test]
    fn switch_persists_default_merges_per_field() {
        let c = parse_config("[claude]\nswitch_persists_default = false\n").unwrap();
        assert_eq!(c["claude"].switch_persists_default, Some(false));
        assert_eq!(c["claude"].switch.as_deref(), Some("/model {model}"));
        assert_eq!(
            plan_switch(&c["claude"], "opus", false, false).unwrap(),
            SwitchPlan::Live("/model opus".into())
        );
        let c = parse_config("[claude]\nmodels = [\"opus\"]\n").unwrap();
        assert_eq!(c["claude"].switch_persists_default, Some(true));
    }

    #[test]
    fn parses_devin_json_and_text_lists() {
        let json = r#"{"families":[{"aliases":["swe"],"variants":[{"model_uid":"swe-2-high"},{"model_uid":"swe-2-max"}]}]}"#;
        assert_eq!(
            parse_model_list(json),
            vec!["swe-2-high", "swe-2-max", "swe"]
        );
        let text = "Available models (2 families)\n\nSWE-2 (swe-2)\n  aliases: swe\n  swe-2-high    SWE-2 High\n  swe-2-max  SWE-2 Max\n";
        assert_eq!(parse_model_list(text), vec!["swe-2-high", "swe-2-max"]);
        assert_eq!(parse_model_list("a\nb\n"), vec!["a", "b"]);
    }
}
