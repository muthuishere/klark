use std::time::Duration;

use bytes::Bytes;

use crate::api::schema::{
    AgentPromptParams, AgentRenameParams, AgentSendKeysParams, AgentStartParams, AgentTarget,
    PaneReadResult, ResponseResult,
};
use crate::app::App;

use super::responses::{encode_error, encode_error_body, encode_success};

const AGENT_PROMPT_SUBMIT_DELAY: Duration = Duration::from_millis(300);

/// A pane counts as being typed in for this long after its last key.
const USER_TYPING_WINDOW: Duration = Duration::from_secs(5);

/// How long to wait between the text and Enter. A long paste takes agents
/// longer to take in (Devin renders it as "[Pasted text]"), and an Enter that
/// lands while the paste is still being read is swallowed, so the prompt sits
/// unsent. 300 ms, plus 1 ms per 16 bytes, at most 1.5 s.
fn submit_delay(text_len: usize) -> Duration {
    let extra = (text_len / 16).min(1200) as u64;
    AGENT_PROMPT_SUBMIT_DELAY + Duration::from_millis(extra)
}

// Codex's Windows input reader does not surface bracketed paste. It detects the prompt as a
// "paste burst" and, while that burst is buffered, rewrites a following Enter into a newline
// instead of submitting. The burst only flushes after an idle timeout, so any size-based delay is
// a timing guess that fails when ConPTY delivery lags it. Codex flushes a buffered burst
// synchronously when it receives a non-character key, so appending one after the paste gives the
// submission a deterministic paste boundary regardless of prompt size or delivery speed.
#[cfg(windows)]
fn append_codex_paste_boundary(runtime: &crate::terminal::TerminalRuntime, text: &mut Vec<u8>) {
    let keys = match crate::app::api_helpers::encode_api_keys(runtime, &["right".to_string()]) {
        Ok(keys) => keys,
        Err(key) => {
            tracing::warn!(key = %key, "failed to encode Codex paste boundary key");
            return;
        }
    };
    if let Some(key) = keys.into_iter().find(|bytes| !bytes.is_empty()) {
        text.extend_from_slice(&key);
    }
}

impl App {
    pub(super) fn handle_agent_list(&mut self, id: String) -> String {
        encode_success(
            id,
            ResponseResult::AgentList {
                agents: self.collect_agent_infos(),
            },
        )
    }

    pub(super) fn handle_agent_get(&mut self, id: String, target: AgentTarget) -> String {
        self.reconcile_managed_agent_target(&target.target);
        let agent = match self.agent_info_for_target(&target.target) {
            Ok(agent) => agent,
            Err(err) => return encode_error_body(id, self.agent_target_error_body(err)),
        };

        encode_success(id, ResponseResult::AgentInfo { agent })
    }

    pub(super) fn handle_agent_focus(&mut self, id: String, target: AgentTarget) -> String {
        let agent = match self.focus_agent_target(&target.target) {
            Ok(agent) => agent,
            Err(err) => return encode_error_body(id, self.agent_target_error_body(err)),
        };

        encode_success(id, ResponseResult::AgentInfo { agent })
    }

    /// `[agents] close_idle_after`: every few minutes, close the agents that
    /// have been idle that long, by the same rules as `klark agent close
    /// --idle-for` (never working, blocked or pinned agents). Returns whether
    /// anything was closed.
    pub(crate) fn close_idle_agents_if_due(&mut self, now: std::time::Instant) -> bool {
        const CHECK_EVERY: Duration = Duration::from_secs(300);
        let Some(idle_for) = self.state.close_idle_after_secs else {
            return false;
        };
        if now.duration_since(self.state.last_idle_close_check) < CHECK_EVERY {
            return false;
        }
        self.state.last_idle_close_check = now;
        let session = crate::session::active_name()
            .unwrap_or_else(|| crate::session::DEFAULT_SESSION_NAME.to_owned());
        let rows: Vec<serde_json::Value> = self
            .collect_agent_infos()
            .into_iter()
            .filter_map(|agent| serde_json::to_value(agent).ok())
            .map(|mut row| {
                row["session"] = session.clone().into();
                row
            })
            .collect();
        let pins = crate::agent_lifecycle::load_pins(&crate::agent_lifecycle::pins_path());
        let selection = crate::agent_lifecycle::select_closable(&rows, idle_for, &pins, None);
        let mut closed = false;
        for row in selection.close {
            let Some(pane_id) = row["pane_id"].as_str() else {
                continue;
            };
            let response = self.handle_agent_close(
                "idle-close".into(),
                crate::api::schema::AgentCloseParams {
                    target: pane_id.to_owned(),
                    reason: Some(crate::agent_lifecycle::REASON_IDLE_CLOSE.into()),
                },
            );
            closed |= !response.contains("\"error\"");
            tracing::info!(pane = pane_id, idle_for, "closed idle agent");
        }
        closed
    }

    pub(super) fn handle_agent_close(
        &mut self,
        id: String,
        params: crate::api::schema::AgentCloseParams,
    ) -> String {
        let reason = match params.reason.as_deref() {
            None | Some(crate::agent_lifecycle::REASON_USER) => crate::agent_lifecycle::REASON_USER,
            Some(crate::agent_lifecycle::REASON_IDLE_CLOSE) => {
                crate::agent_lifecycle::REASON_IDLE_CLOSE
            }
            Some(other) => {
                return encode_error(
                    id,
                    "invalid_close_reason",
                    format!("close reason must be user or idle-close, not {other}"),
                )
            }
        };
        let agent = match self.agent_info_for_target(&params.target) {
            Ok(agent) => agent,
            Err(err) => return encode_error_body(id, self.agent_target_error_body(err)),
        };
        self.state.close_reason = reason;
        let closed = self.close_pane(
            id.clone(),
            &crate::api::schema::PaneTarget {
                pane_id: agent.pane_id.clone(),
            },
        );
        self.state.close_reason = crate::agent_lifecycle::REASON_USER;
        match closed {
            Ok(()) => encode_success(id, ResponseResult::AgentInfo { agent }),
            Err(response) => response,
        }
    }

    pub(super) fn handle_agent_rename(&mut self, id: String, params: AgentRenameParams) -> String {
        let agent = match self.rename_agent_target(&params.target, params.name) {
            Ok(agent) => agent,
            Err(err) => return encode_error_body(id, self.agent_rename_error_body(err)),
        };

        encode_success(id, ResponseResult::AgentInfo { agent })
    }

    pub(super) fn handle_agent_start(&mut self, id: String, params: AgentStartParams) -> String {
        let (agent, argv) = match self.start_agent(params) {
            Ok(started) => started,
            Err(err) => return encode_error_body(id, self.agent_start_error_body(err)),
        };

        encode_success(id, ResponseResult::AgentStarted { agent, argv })
    }

    pub(crate) fn handle_deferred_agent_api_request(
        &mut self,
        request: crate::api::schema::Request,
        respond_to: std::sync::mpsc::Sender<String>,
    ) -> bool {
        let crate::api::schema::Method::AgentPrompt(params) = request.method else {
            return false;
        };
        let validated = match self.validate_agent_prompt(request.id, &params) {
            Ok(validated) => validated,
            Err(response) => {
                let _ = respond_to.send(response);
                return true;
            }
        };
        // Start the completion waiter before submitting, so a refused thread
        // fails the request while nothing has been sent yet.
        let (handoff_tx, handoff_rx) = std::sync::mpsc::channel::<SubmittedAgentPrompt>();
        let waiter_respond_to = respond_to.clone();
        let target = params.target.clone();
        let spawned = crate::thread_spawn::spawn_named("klark-agent-prompt", move || {
            let Ok(SubmittedAgentPrompt {
                id,
                agent,
                completion,
                draft,
                input_draft,
                prompt_chars,
                leftover_prompt_chars,
            }) = handoff_rx.recv()
            else {
                return;
            };
            let response = match completion.recv() {
                Ok(Ok(())) => encode_success(id, ResponseResult::AgentPrompted { agent, draft }),
                Ok(Err(err)) if crate::pty::enter_guard::is_agent_became_blocked(&err) => {
                    // The text went in, Enter did not. Nothing more is typed:
                    // with a dialog focused, any key (even Backspace) goes to
                    // the dialog. The prompt stays unsent in the input; the
                    // next prompt erases it first.
                    input_draft
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .unsubmitted_prompt_chars += prompt_chars;
                    agent_blocked_error(
                        id,
                        format!(
                            "agent {target} became blocked before the prompt was submitted; \
                             Enter was not sent and nothing was submitted (the prompt text \
                             remains unsent in the agent's input)"
                        ),
                        draft,
                    )
                }
                Ok(Err(err)) if crate::pty::enter_guard::is_agent_blocked_before_typing(&err) => {
                    // Nothing was written, not even the erase: the person's
                    // draft and any leftover prompt are still in the input.
                    let mut d = input_draft.lock().unwrap_or_else(|p| p.into_inner());
                    d.unsubmitted_prompt_chars += leftover_prompt_chars;
                    if let Some(draft) = draft {
                        d.text.insert_str(0, &draft);
                    }
                    drop(d);
                    agent_blocked_error(
                        id,
                        format!(
                            "agent {target} became blocked before the prompt was typed; \
                             nothing was typed or submitted"
                        ),
                        None,
                    )
                }
                Ok(Err(err)) if err.kind() == std::io::ErrorKind::TimedOut => {
                    encode_error(id, "timeout", err.to_string())
                }
                Ok(Err(err)) => encode_error(id, "agent_prompt_failed", err.to_string()),
                Err(_) => encode_error(id, "agent_prompt_failed", "pty actor closed"),
            };
            let _ = waiter_respond_to.send(response);
        });
        if let Err(err) = spawned {
            tracing::warn!(err = %err, "failed to spawn agent prompt thread");
            let _ = respond_to.send(encode_error(
                validated.id,
                "agent_prompt_failed",
                format!("could not start prompt completion waiter: {err}"),
            ));
            return true;
        }
        match self.submit_agent_prompt(validated, &params) {
            Ok(submission) => {
                let _ = handoff_tx.send(submission);
            }
            Err(response) => {
                let _ = respond_to.send(response);
            }
        }
        true
    }

    /// Runs every check that can reject a prompt without touching the pane.
    fn validate_agent_prompt(
        &self,
        id: String,
        params: &AgentPromptParams,
    ) -> Result<ValidatedAgentPrompt, String> {
        if params.text.is_empty() {
            return Err(encode_error(
                id,
                "empty_agent_prompt",
                "agent prompt must not be empty",
            ));
        }
        let resolved = match self.resolve_agent_target(&params.target) {
            Ok(resolved) => resolved,
            Err(err) => return Err(encode_error_body(id, self.agent_target_error_body(err))),
        };
        let Some(terminal_id) = self
            .state
            .workspaces
            .get(resolved.ws_idx)
            .and_then(|workspace| workspace.terminal_id(resolved.pane_id))
            .cloned()
        else {
            return Err(agent_not_found(id, &params.target));
        };
        let Some(terminal) = self.state.terminals.get(&terminal_id) else {
            return Err(agent_not_found(id, &params.target));
        };
        if terminal.state == crate::detect::AgentState::Blocked {
            return Err(agent_blocked_up_front(id, &params.target));
        }
        let Some(expected_agent) = terminal.effective_known_agent() else {
            return Err(agent_not_ready(id, &params.target));
        };
        if terminal.managed_agent_launch_pending() {
            return Err(agent_not_ready(id, &params.target));
        }
        let Some(runtime) = self.lookup_runtime_sender(resolved.ws_idx, resolved.pane_id) else {
            return Err(agent_not_found(id, &params.target));
        };
        if !super::super::agents::runtime_hosts_agent(runtime, expected_agent) {
            return Err(encode_error(
                id,
                "agent_not_ready",
                format!(
                    "agent {} is no longer the pane foreground process",
                    params.target
                ),
            ));
        }
        // `terminal.state` is published from the detector's last pass and can
        // lag the screen; scan the screen as it is now as well.
        if runtime.screen_shows_agent_blocked(expected_agent) {
            return Err(agent_blocked_up_front(id, &params.target));
        }
        let app_blocked = terminal.blocked_signal();
        let Some(agent) = self.agent_info(resolved.ws_idx, resolved.pane_id) else {
            return Err(agent_not_found(id, &params.target));
        };
        Ok(ValidatedAgentPrompt {
            id,
            ws_idx: resolved.ws_idx,
            pane_id: resolved.pane_id,
            expected_agent,
            agent,
            app_blocked,
        })
    }

    fn submit_agent_prompt(
        &mut self,
        validated: ValidatedAgentPrompt,
        params: &AgentPromptParams,
    ) -> Result<SubmittedAgentPrompt, String> {
        let ValidatedAgentPrompt {
            id,
            ws_idx,
            pane_id,
            expected_agent,
            agent,
            app_blocked,
        } = validated;
        let Some(runtime) = self.lookup_runtime_sender(ws_idx, pane_id) else {
            return Err(agent_not_found(id, &params.target));
        };
        #[cfg(windows)]
        let submit_deadline = params
            .wait
            .as_ref()
            .and_then(|wait| wait.submission_deadline);
        #[cfg(not(windows))]
        let submit_deadline = None;
        if expected_agent == crate::detect::Agent::GithubCopilot {
            // Copilot ignores synthetic Enter after focus loss until it receives focus gained.
            let focus = match crate::ghostty::encode_focus(crate::ghostty::FocusEvent::Gained) {
                Ok(focus) => focus,
                Err(err) => {
                    return Err(encode_error(id, "agent_prompt_failed", err.to_string()));
                }
            };
            if let Err(err) = runtime.try_send_bytes(Bytes::from(focus)) {
                return Err(encode_error(id, "agent_prompt_failed", err.to_string()));
            }
        }
        // Someone may be typing in this pane. Lift their unsent draft out of the
        // way with Backspaces, so Enter submits only this prompt; the caller gets
        // the draft back to retype. Mid-edit (arrows, history) within the last
        // few seconds, the draft can't be known: refuse instead of mixing texts.
        // An earlier prompt left unsubmitted (agent became blocked before its
        // Enter) is erased too, but is not the person's and is not returned.
        let (draft, leftover_prompt_chars) = {
            let mut d = runtime.input_draft();
            let recent = d
                .last_input
                .is_some_and(|t| t.elapsed() < USER_TYPING_WINDOW);
            if d.unknown && recent {
                return Err(encode_error(
                    id,
                    "agent_user_typing",
                    "someone is editing this agent's input; try again shortly",
                ));
            }
            let taken = (!d.unknown && !d.text.is_empty()).then(|| std::mem::take(&mut d.text));
            if d.unknown {
                // Stale and unknowable: assume it was cleared, start tracking afresh.
                d.unknown = false;
                d.text.clear();
            }
            (taken, std::mem::take(&mut d.unsubmitted_prompt_chars))
        };
        let (text, enter) =
            crate::app::api_helpers::encode_api_submission_parts(runtime, &params.text);
        let erase_chars = leftover_prompt_chars + draft.as_ref().map_or(0, |d| d.chars().count());
        let text = if erase_chars > 0 {
            let mut erase = vec![0x7f_u8; erase_chars];
            erase.extend_from_slice(&text);
            erase
        } else {
            text
        };
        #[cfg(windows)]
        let text = if expected_agent == crate::detect::Agent::Codex {
            let mut text = text;
            append_codex_paste_boundary(runtime, &mut text);
            text
        } else {
            text
        };
        let enter_guard = runtime.agent_prompt_enter_guard(expected_agent, app_blocked);
        let input_draft = runtime.input_draft_handle();
        let completion = match runtime.queue_user_input_submission(
            Bytes::from(text),
            Bytes::from(enter),
            submit_delay(params.text.len()),
            submit_deadline,
            Some(enter_guard),
        ) {
            Ok(completion) => completion,
            Err(err) => {
                // Nothing was written: the leftover prompt is still in the input.
                input_draft
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .unsubmitted_prompt_chars += leftover_prompt_chars;
                return Err(encode_error(id, "agent_prompt_failed", err.to_string()));
            }
        };
        Ok(SubmittedAgentPrompt {
            id,
            agent,
            completion,
            draft,
            input_draft,
            prompt_chars: params.text.chars().count(),
            leftover_prompt_chars,
        })
    }

    pub(super) fn handle_agent_read(
        &mut self,
        id: String,
        params: crate::api::schema::AgentReadParams,
    ) -> String {
        let resolved = match self.resolve_agent_target(&params.target) {
            Ok(resolved) => resolved,
            Err(err) => return encode_error_body(id, self.agent_target_error_body(err)),
        };
        let Some((pane, workspace_id)) = self.lookup_runtime(resolved.ws_idx, resolved.pane_id)
        else {
            return agent_not_found(id, &params.target);
        };
        let snapshot = crate::app::api_helpers::read_terminal_snapshot(
            pane,
            params.source,
            params.format,
            params.lines,
        );

        encode_success(
            id,
            ResponseResult::PaneRead {
                read: PaneReadResult {
                    pane_id: self
                        .public_pane_id(resolved.ws_idx, resolved.pane_id)
                        .unwrap_or_else(|| params.target.clone()),
                    workspace_id,
                    tab_id: self
                        .public_tab_id(resolved.ws_idx, resolved.tab_idx)
                        .unwrap(),
                    source: params.source,
                    format: params.format,
                    text: snapshot.text,
                    revision: 0,
                    truncated: snapshot.truncated,
                },
            },
        )
    }

    pub(super) fn handle_agent_explain(&mut self, id: String, target: AgentTarget) -> String {
        let resolved = match self.resolve_agent_target(&target.target) {
            Ok(resolved) => resolved,
            Err(err) => return encode_error_body(id, self.agent_target_error_body(err)),
        };
        let Some((pane, _workspace_id)) = self.lookup_runtime(resolved.ws_idx, resolved.pane_id)
        else {
            return agent_not_found(id, &target.target);
        };
        let Some(terminal_id) = self
            .state
            .workspaces
            .get(resolved.ws_idx)
            .and_then(|workspace| workspace.terminal_id(resolved.pane_id))
        else {
            return agent_not_found(id, &target.target);
        };
        let Some(terminal) = self.state.terminals.get(terminal_id) else {
            return agent_not_found(id, &target.target);
        };
        if terminal.full_lifecycle_hook_authority_active() {
            let explain = serde_json::json!({
                "agent": terminal.effective_agent_label().unwrap_or("unknown"),
                "state": crate::detect::manifest::agent_state_label(terminal.state),
                "manifest_source": null,
                "manifest_version": null,
                "cached_remote_version": null,
                "local_override_shadowing_remote": false,
                "remote_update_status": null,
                "remote_update_error": null,
                "matched_rule": null,
                "visible_idle": false,
                "visible_blocker": false,
                "visible_working": false,
                "screen_detection_skipped": true,
                "screen_detection_skip_reason": "full_lifecycle_hook_authority",
                "skip_state_update": false,
                "skipped_update_reason": null,
                "fallback_reason": null,
                "warning": null,
                "evaluated_rules": [],
            });
            return encode_success(id, ResponseResult::AgentExplain { explain });
        }
        let Some(agent) = terminal.effective_known_agent().or(terminal.detected_agent) else {
            return encode_error(
                id,
                "agent_explain_unavailable",
                format!(
                    "agent target {} does not have a detected agent label",
                    target.target
                ),
            );
        };

        let screen = pane.detection_text();
        let osc_title = pane.agent_osc_title();
        let osc_progress = pane.agent_osc_progress();
        let explain = crate::detect::manifest::explain_with_input(
            agent,
            crate::detect::manifest::DetectionInput {
                screen: &screen,
                osc_title: &osc_title,
                osc_progress: &osc_progress,
            },
        );
        let value = crate::detect::manifest::explain_to_json_value(&explain);

        encode_success(id, ResponseResult::AgentExplain { explain: value })
    }

    pub(super) fn handle_agent_send_keys(
        &mut self,
        id: String,
        params: AgentSendKeysParams,
    ) -> String {
        let resolved = match self.resolve_agent_target(&params.target) {
            Ok(resolved) => resolved,
            Err(err) => return encode_error_body(id, self.agent_target_error_body(err)),
        };
        let Some(terminal_id) = self
            .state
            .workspaces
            .get(resolved.ws_idx)
            .and_then(|workspace| workspace.terminal_id(resolved.pane_id))
        else {
            return agent_not_found(id, &params.target);
        };
        let Some(expected_agent) = self
            .state
            .terminals
            .get(terminal_id)
            .and_then(|terminal| terminal.effective_known_agent())
        else {
            return agent_not_ready(id, &params.target);
        };
        let Some(runtime) = self.lookup_runtime_sender(resolved.ws_idx, resolved.pane_id) else {
            return agent_not_found(id, &params.target);
        };
        if !super::super::agents::runtime_hosts_agent(runtime, expected_agent) {
            return agent_not_ready(id, &params.target);
        }
        let encoded = match super::super::api_helpers::encode_api_keys(runtime, &params.keys) {
            Ok(encoded) => encoded,
            Err(key) => {
                return encode_error(id, "invalid_key", format!("unsupported key {key}"));
            }
        };
        let bytes: Vec<u8> = encoded.into_iter().flatten().collect();
        if let Err(err) = runtime.try_send_bytes(Bytes::from(bytes)) {
            return encode_error(id, "agent_send_keys_failed", err.to_string());
        }

        encode_success(id, ResponseResult::Ok {})
    }
}

struct ValidatedAgentPrompt {
    id: String,
    ws_idx: usize,
    pane_id: crate::layout::PaneId,
    expected_agent: crate::detect::Agent,
    agent: crate::api::schema::AgentInfo,
    /// The app's "blocked" state for this terminal, read again at Enter time.
    app_blocked: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// A prompt handed to the PTY writer, waiting for its Enter to be written or dropped.
struct SubmittedAgentPrompt {
    id: String,
    agent: crate::api::schema::AgentInfo,
    completion: std::sync::mpsc::Receiver<std::io::Result<()>>,
    /// The person's draft lifted out of the input before the prompt was typed.
    draft: Option<String>,
    input_draft: std::sync::Arc<std::sync::Mutex<crate::pane::InputDraft>>,
    prompt_chars: usize,
    /// Characters of an earlier unsubmitted prompt this one erases first.
    leftover_prompt_chars: usize,
}

fn agent_blocked_up_front(id: String, target: &str) -> String {
    // Checked before the person's draft is touched: nothing to hand back.
    agent_blocked_error(
        id,
        format!("agent {target} is blocked and requires interactive input"),
        None,
    )
}

fn agent_blocked_error(id: String, message: String, unsent_draft: Option<String>) -> String {
    encode_error_body(
        id,
        crate::api::schema::ErrorBody {
            code: "agent_blocked".into(),
            message,
            unsent_draft,
        },
    )
}

fn agent_not_ready(id: String, target: &str) -> String {
    encode_error(
        id,
        "agent_not_ready",
        format!("agent {target} is not an active named agent"),
    )
}

fn agent_not_found(id: String, target: &str) -> String {
    encode_error(
        id,
        "agent_not_found",
        format!("agent target {target} not found"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        api::schema::{AgentStatus, SuccessResponse},
        app::Mode,
        config::Config,
        detect::{Agent, AgentState},
        workspace::Workspace,
    };

    fn app_with_agent() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.workspaces = vec![Workspace::test_new("agent")];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        app
    }

    fn start_deferred_agent_prompt(
        app: &mut App,
        id: &str,
        params: AgentPromptParams,
    ) -> std::sync::mpsc::Receiver<String> {
        let (respond_to, response_rx) = std::sync::mpsc::channel();
        assert!(app.handle_deferred_agent_api_request(
            crate::api::schema::Request {
                id: id.into(),
                method: crate::api::schema::Method::AgentPrompt(params),
            },
            respond_to,
        ));
        response_rx
    }

    fn run_deferred_agent_prompt(app: &mut App, id: &str, params: AgentPromptParams) -> String {
        start_deferred_agent_prompt(app, id, params)
            .recv_timeout(Duration::from_secs(1))
            .expect("agent prompt responds after submission")
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_codex_prompt_flushes_paste_burst_before_enter() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(Agent::Codex), AgentState::Idle);
        let (runtime, mut rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(pane_id, runtime);

        let response = run_deferred_agent_prompt(
            &mut app,
            "req",
            AgentPromptParams {
                target: "reviewer".into(),
                text: "A != B".into(),
                wait: None,
            },
        );
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert!(matches!(
            success.result,
            ResponseResult::AgentPrompted { .. }
        ));
        // The non-character key must precede Enter so Codex commits the paste burst first.
        assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"A != B\x1b[C"));
        assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"\r"));
    }

    #[tokio::test]
    async fn a_false_process_exit_makes_a_named_live_agent_unreachable_by_name() {
        // Reproduces the registration loss reported on #3225 by rszrszrsz:
        // a live agent pane with an assigned name stops resolving by that name
        // while its process keeps running, and renaming is the only recovery.
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let observed_at = std::time::Instant::now();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_detected_state(Some(Agent::Pi), AgentState::Working);
        terminal.set_agent_name("reviewer".into());

        let found = app.handle_agent_get(
            "req:before".into(),
            AgentTarget {
                target: "reviewer".into(),
            },
        );
        assert!(
            serde_json::from_str::<SuccessResponse>(&found).is_ok(),
            "the assigned name must resolve while the agent is running: {found}"
        );

        // One process-exit observation, then the same agent is observed alive
        // again on the next probe - the process never actually went away.
        app.handle_internal_event(crate::events::AppEvent::StateChanged {
            pane_id,
            agent: Some(Agent::Pi),
            state: AgentState::Idle,
            visible_blocker: false,
            visible_working: false,
            process_exited: true,
            observed_at,
        });
        app.handle_internal_event(crate::events::AppEvent::AgentProcessDetected {
            pane_id,
            agent: Agent::Pi,
            observed_at: observed_at + std::time::Duration::from_secs(1),
        });

        let terminal = &app.state.terminals[&terminal_id];
        assert_eq!(
            terminal.detected_agent,
            Some(Agent::Pi),
            "the agent process is still there"
        );

        let after = app.handle_agent_get(
            "req:after".into(),
            AgentTarget {
                target: "reviewer".into(),
            },
        );
        assert!(
            serde_json::from_str::<SuccessResponse>(&after).is_ok(),
            "a live agent must stay reachable by its assigned name: {after}"
        );
    }

    #[test]
    fn submit_delay_grows_with_text_and_is_capped() {
        assert_eq!(super::submit_delay(10), AGENT_PROMPT_SUBMIT_DELAY);
        assert_eq!(
            super::submit_delay(1600),
            AGENT_PROMPT_SUBMIT_DELAY + Duration::from_millis(100)
        );
        assert_eq!(
            super::submit_delay(10_000_000),
            AGENT_PROMPT_SUBMIT_DELAY + Duration::from_millis(1200)
        );
    }

    #[tokio::test]
    async fn agent_prompt_sends_text_then_delays_enter() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(Agent::OpenCode), AgentState::Working);
        let (runtime, mut rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                80, 24, 0, b"", 2,
            );
        runtime.test_process_pty_bytes(b"\x1b[?2004h");
        app.state.insert_test_runtime(pane_id, runtime);

        let public_pane_id = app.public_pane_id(0, pane_id).unwrap();
        let bracketed_started = std::time::Instant::now();
        let response_rx = start_deferred_agent_prompt(
            &mut app,
            "req",
            AgentPromptParams {
                target: public_pane_id,
                text: "A != B".into(),
                wait: None,
            },
        );
        assert!(response_rx.try_recv().is_err());
        let response = response_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("agent prompt responds after submission");
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::AgentPrompted { agent, .. } = success.result else {
            panic!("expected prompted response");
        };
        assert_eq!(agent.name.as_deref(), Some("reviewer"));
        assert_eq!(
            rx.try_recv().unwrap(),
            Bytes::from_static(b"\x1b[200~A != B\x1b[201~")
        );
        assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"\r"));
        assert!(bracketed_started.elapsed() >= AGENT_PROMPT_SUBMIT_DELAY);

        app.lookup_runtime_sender(0, pane_id)
            .unwrap()
            .test_process_pty_bytes(b"\x1b[?2004l");
        let raw_started = std::time::Instant::now();
        let raw = run_deferred_agent_prompt(
            &mut app,
            "req-raw",
            AgentPromptParams {
                target: "reviewer".into(),
                text: "A != B".into(),
                wait: None,
            },
        );
        let raw: SuccessResponse = serde_json::from_str(&raw).unwrap();
        assert!(matches!(raw.result, ResponseResult::AgentPrompted { .. }));
        assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"A != B"));
        assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"\r"));
        assert!(raw_started.elapsed() >= AGENT_PROMPT_SUBMIT_DELAY);

        let rejected = run_deferred_agent_prompt(
            &mut app,
            "req-label",
            AgentPromptParams {
                target: "opencode".into(),
                text: "wrong target".into(),
                wait: None,
            },
        );
        let error: crate::api::schema::ErrorResponse = serde_json::from_str(&rejected).unwrap();
        assert_eq!(error.error.code, "agent_not_found");
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn agent_prompt_waiter_spawn_failure_fails_before_submitting() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(Agent::OpenCode), AgentState::Idle);
        let (runtime, mut rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(pane_id, runtime);

        crate::thread_spawn::test_hook::fail_next_spawns(1);
        let response = run_deferred_agent_prompt(
            &mut app,
            "req",
            AgentPromptParams {
                target: "reviewer".into(),
                text: "hello".into(),
                wait: None,
            },
        );

        let error: crate::api::schema::ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "agent_prompt_failed");
        std::thread::sleep(AGENT_PROMPT_SUBMIT_DELAY * 2);
        assert!(rx.try_recv().is_err(), "a failed prompt must not be sent");
    }

    #[tokio::test]
    async fn agent_prompt_validation_errors_win_over_waiter_spawn_failure() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(Agent::OpenCode), AgentState::Idle);
        let (runtime, mut rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(pane_id, runtime);

        for (target, text, code) in [
            ("reviewer", "", "empty_agent_prompt"),
            ("missing", "hello", "agent_not_found"),
        ] {
            crate::thread_spawn::test_hook::fail_next_spawns(1);
            let response = run_deferred_agent_prompt(
                &mut app,
                "req",
                AgentPromptParams {
                    target: target.into(),
                    text: text.into(),
                    wait: None,
                },
            );

            let error: crate::api::schema::ErrorResponse = serde_json::from_str(&response).unwrap();
            assert_eq!(error.error.code, code);
            assert!(
                crate::thread_spawn::spawn_named("probe", || {}).is_err(),
                "a rejected prompt must not start a waiter thread"
            );
        }
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn agent_prompt_rejects_blocked_agent_without_writing() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(Agent::GithubCopilot), AgentState::Blocked);
        let (runtime, mut rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(pane_id, runtime);

        let response = run_deferred_agent_prompt(
            &mut app,
            "req",
            AgentPromptParams {
                target: "reviewer".into(),
                text: "unrelated prompt".into(),
                wait: None,
            },
        );

        let error: crate::api::schema::ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "agent_blocked");
        assert!(
            tokio::time::timeout(
                AGENT_PROMPT_SUBMIT_DELAY + Duration::from_millis(100),
                rx.recv()
            )
            .await
            .is_err(),
            "blocked prompt wrote or scheduled terminal input"
        );
    }

    /// A Claude Code permission dialog, as the screen shows it.
    const CLAUDE_PERMISSION_DIALOG: &[u8] = b"\x1b[2J\x1b[H\
        Bash command\r\n  rm -rf build\r\n\
        \xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80\r\n\
        Do you want to proceed?\r\n\
        \xe2\x9d\xaf 1. Yes\r\n  2. No\r\n\r\n\
        Esc to cancel \xc2\xb7 Enter to confirm\r\n";

    fn app_with_named_agent(
        agent: Agent,
        state: AgentState,
    ) -> (
        App,
        crate::layout::PaneId,
        crate::terminal::TerminalId,
        tokio::sync::mpsc::Receiver<Bytes>,
    ) {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(agent), state);
        let (runtime, rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(pane_id, runtime);
        (app, pane_id, terminal_id, rx)
    }

    fn prompt(text: &str) -> AgentPromptParams {
        AgentPromptParams {
            target: "reviewer".into(),
            text: text.into(),
            wait: None,
        }
    }

    async fn next_write(rx: &mut tokio::sync::mpsc::Receiver<Bytes>) -> Bytes {
        tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("pane write")
            .expect("pane open")
    }

    async fn assert_no_write(rx: &mut tokio::sync::mpsc::Receiver<Bytes>, why: &str) {
        let late = tokio::time::timeout(Duration::from_millis(150), rx.recv()).await;
        assert!(late.is_err(), "{why}: {late:?}");
    }

    #[tokio::test]
    async fn agent_prompt_withholds_enter_when_agent_becomes_blocked_during_submit_delay() {
        let (mut app, pane_id, terminal_id, mut rx) =
            app_with_named_agent(Agent::OpenCode, AgentState::Working);
        // The person has typed an unsent draft into the agent's input.
        app.lookup_runtime_sender(0, pane_id)
            .unwrap()
            .input_draft()
            .note_text("wip");

        // Long enough (submit delay ~0.85 s) that the state flip below lands
        // inside the delay even on a loaded machine.
        let text = "do it ".repeat(1500);
        let response_rx = start_deferred_agent_prompt(&mut app, "req", prompt(&text));
        // The draft is lifted out and the prompt typed: accepted while not blocked.
        assert_eq!(
            next_write(&mut rx).await,
            Bytes::from([b"\x7f\x7f\x7f".as_slice(), text.as_bytes()].concat())
        );
        // Inside the submit delay the agent raises a dialog (hook or detector report).
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_detected_state(Some(Agent::OpenCode), AgentState::Blocked);

        let response = response_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("agent prompt responds");
        let error: crate::api::schema::ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "agent_blocked");
        assert!(
            error.error.message.contains("nothing was submitted"),
            "{}",
            error.error.message
        );
        assert_eq!(error.error.unsent_draft.as_deref(), Some("wip"));
        assert_no_write(&mut rx, "Enter must not reach a blocked agent").await;
        let runtime = app.lookup_runtime_sender(0, pane_id).unwrap();
        assert_eq!(runtime.input_draft().unsubmitted_prompt_chars, text.len());
        assert!(runtime.input_draft().text.is_empty());

        // Up-front refusal while still blocked: nothing typed, no draft to return.
        let refused = run_deferred_agent_prompt(&mut app, "req-blocked", prompt("do it"));
        let refused: crate::api::schema::ErrorResponse = serde_json::from_str(&refused).unwrap();
        assert_eq!(refused.error.code, "agent_blocked");
        assert_eq!(refused.error.unsent_draft, None);
        assert_no_write(&mut rx, "a blocked agent gets no input").await;

        // Once unblocked, the retried prompt first erases the one left behind,
        // which is not reported as the person's draft.
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_detected_state(Some(Agent::OpenCode), AgentState::Working);
        let retried = start_deferred_agent_prompt(&mut app, "req-retry", prompt("do it"))
            .recv_timeout(Duration::from_secs(2))
            .expect("retry responds");
        let retried: SuccessResponse = serde_json::from_str(&retried).unwrap();
        let ResponseResult::AgentPrompted { draft, .. } = retried.result else {
            panic!("expected prompted response");
        };
        assert_eq!(draft, None);
        let mut erase_then_prompt = vec![0x7f_u8; text.len()];
        erase_then_prompt.extend_from_slice(b"do it");
        assert_eq!(next_write(&mut rx).await, Bytes::from(erase_then_prompt));
        assert_eq!(next_write(&mut rx).await, Bytes::from_static(b"\r"));
        assert_eq!(
            app.lookup_runtime_sender(0, pane_id)
                .unwrap()
                .input_draft()
                .unsubmitted_prompt_chars,
            0
        );
    }

    #[tokio::test]
    async fn agent_prompt_enter_check_reads_the_screen_ahead_of_published_state() {
        let (mut app, pane_id, _terminal_id, mut rx) =
            app_with_named_agent(Agent::Claude, AgentState::Working);

        let text = "go ".repeat(3000);
        let response_rx = start_deferred_agent_prompt(&mut app, "req", prompt(&text));
        assert_eq!(next_write(&mut rx).await, Bytes::from(text.clone()));
        // The dialog is on screen, but no detector pass has published it yet.
        let runtime = app.lookup_runtime_sender(0, pane_id).unwrap();
        runtime.test_process_pty_bytes(CLAUDE_PERMISSION_DIALOG);
        assert!(runtime.screen_shows_agent_blocked(Agent::Claude));

        let response = response_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("agent prompt responds");
        let error: crate::api::schema::ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "agent_blocked");
        assert_eq!(error.error.unsent_draft, None);
        assert_no_write(&mut rx, "Enter must not answer the dialog").await;
    }

    #[tokio::test]
    async fn agent_prompt_up_front_check_reads_the_screen_too() {
        let (mut app, pane_id, _terminal_id, mut rx) =
            app_with_named_agent(Agent::Claude, AgentState::Working);
        app.lookup_runtime_sender(0, pane_id)
            .unwrap()
            .test_process_pty_bytes(CLAUDE_PERMISSION_DIALOG);

        let response = run_deferred_agent_prompt(&mut app, "req", prompt("go"));
        let error: crate::api::schema::ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "agent_blocked");
        assert_no_write(&mut rx, "a visibly blocked agent gets no input").await;
    }

    #[tokio::test]
    async fn agent_prompt_focuses_copilot_before_submitting() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(Agent::GithubCopilot), AgentState::Idle);
        let (runtime, mut rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                80, 24, 0, b"", 3,
            );
        runtime.test_process_pty_bytes(b"\x1b[?2004h");
        app.state.insert_test_runtime(pane_id, runtime);

        let response = run_deferred_agent_prompt(
            &mut app,
            "req",
            AgentPromptParams {
                target: "reviewer".into(),
                text: "A != B".into(),
                wait: None,
            },
        );
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert!(matches!(
            success.result,
            ResponseResult::AgentPrompted { .. }
        ));
        assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"\x1b[I"));
        assert_eq!(
            rx.try_recv().unwrap(),
            Bytes::from_static(b"\x1b[200~A != B\x1b[201~")
        );
        assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"\r"));
    }

    #[tokio::test]
    async fn agent_send_keys_validates_every_key_before_writing() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(Agent::Pi), AgentState::Idle);
        let (runtime, mut rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(pane_id, runtime);

        let rejected = app.handle_agent_send_keys(
            "req-invalid".into(),
            AgentSendKeysParams {
                target: "reviewer".into(),
                keys: vec!["enter".into(), "not-a-key".into()],
            },
        );
        let error: crate::api::schema::ErrorResponse = serde_json::from_str(&rejected).unwrap();
        assert_eq!(error.error.code, "invalid_key");
        assert!(rx.try_recv().is_err());

        let sent = app.handle_agent_send_keys(
            "req-valid".into(),
            AgentSendKeysParams {
                target: "reviewer".into(),
                keys: vec!["up".into(), "enter".into()],
            },
        );
        let success: SuccessResponse = serde_json::from_str(&sent).unwrap();
        assert!(matches!(success.result, ResponseResult::Ok {}));
        assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"\x1b[A\r"));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn agent_prompt_rejects_managed_agent_while_startup_is_pending() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        let now = std::time::Instant::now();
        terminal.begin_managed_agent(
            "reviewer".into(),
            Agent::OpenCode,
            now,
            std::time::Duration::from_secs(3),
            std::time::Duration::from_secs(10),
        );
        terminal.set_detected_state(Some(Agent::OpenCode), AgentState::Idle);
        let (runtime, mut rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(pane_id, runtime);

        let response = run_deferred_agent_prompt(
            &mut app,
            "req-pending",
            AgentPromptParams {
                target: "reviewer".into(),
                text: "A != B".into(),
                wait: None,
            },
        );
        let error: crate::api::schema::ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "agent_not_ready");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn agent_focus_marks_already_focused_done_agent_seen() {
        let mut app = app_with_agent();
        app.state.outer_terminal_focus = Some(false);

        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_detected_state(Some(Agent::Pi), AgentState::Idle);
        app.state.workspaces[0].tabs[0]
            .panes
            .get_mut(&pane_id)
            .unwrap()
            .seen = false;
        app.state.workspaces[0].tabs[0].layout.focus_pane(pane_id);

        let response = app.handle_agent_focus(
            "req".into(),
            AgentTarget {
                target: app.public_pane_id(0, pane_id).unwrap(),
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::AgentInfo { agent } = success.result else {
            panic!("expected agent info response");
        };
        assert_eq!(agent.agent_status, AgentStatus::Idle);
    }

    fn closed_log_for_test(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("klark-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("agents-closed.jsonl")
    }

    #[test]
    fn agent_close_closes_the_pane_and_records_the_agent() {
        let path = closed_log_for_test("agent-close");
        let mut app = app_with_agent();
        app.state.closed_agents_path = Some(path.clone());
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_detected_state(Some(Agent::Pi), AgentState::Idle);
        terminal.agent_name = Some("worker".into());
        let target = app.public_pane_id(0, pane_id).unwrap();

        let response = app.handle_agent_close(
            "req".into(),
            crate::api::schema::AgentCloseParams {
                target,
                reason: None,
            },
        );
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert!(matches!(success.result, ResponseResult::AgentInfo { .. }));
        let records = crate::agent_lifecycle::read_closed(&path, crate::agent_lifecycle::now_secs());
        assert_eq!(records.len(), 1, "{response}");
        assert_eq!(records[0].name.as_deref(), Some("worker"));
        assert_eq!(records[0].agent, "pi");
        assert_eq!(records[0].reason, "user");
        assert_eq!(app.state.close_reason, "user");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn agent_close_rejects_an_unknown_reason() {
        let mut app = app_with_agent();
        let response = app.handle_agent_close(
            "req".into(),
            crate::api::schema::AgentCloseParams {
                target: "nope".into(),
                reason: Some("whim".into()),
            },
        );
        assert!(response.contains("invalid_close_reason"), "{response}");
    }

    #[test]
    fn idle_agents_are_closed_on_the_timer_and_busy_ones_are_not() {
        let path = closed_log_for_test("idle-close");
        let mut app = app_with_agent();
        app.state.closed_agents_path = Some(path.clone());
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        {
            let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
            terminal.set_detected_state(Some(Agent::Pi), AgentState::Working);
            terminal.agent_state_since = crate::agent_lifecycle::now_secs() - 5 * 3600;
        }
        let long_ago = std::time::Instant::now() - Duration::from_secs(3600);

        // Off by default.
        app.state.last_idle_close_check = long_ago;
        assert!(!app.close_idle_agents_if_due(std::time::Instant::now()));
        app.state.close_idle_after_secs = Some(3 * 3600);

        // Not due yet.
        app.state.last_idle_close_check = std::time::Instant::now();
        assert!(!app.close_idle_agents_if_due(std::time::Instant::now()));

        // Working for 5h is never closed.
        app.state.last_idle_close_check = long_ago;
        assert!(!app.close_idle_agents_if_due(std::time::Instant::now()));
        assert_eq!(app.state.workspaces.len(), 1);

        // Idle for 5h is.
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_detected_state(Some(Agent::Pi), AgentState::Idle);
        terminal.agent_state_since = crate::agent_lifecycle::now_secs() - 5 * 3600;
        app.state.last_idle_close_check = long_ago;
        assert!(app.close_idle_agents_if_due(std::time::Instant::now()));
        let records = crate::agent_lifecycle::read_closed(&path, crate::agent_lifecycle::now_secs());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].reason, "idle-close");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn agent_rename_does_not_replace_the_pane_label() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_manual_label("shell-pane".into());
        terminal.set_detected_state(Some(Agent::Pi), AgentState::Idle);
        let target = app.public_pane_id(0, pane_id).unwrap();

        for name in [Some("reviewer".to_string()), None] {
            let response = app.handle_agent_rename(
                "req".into(),
                AgentRenameParams {
                    target: target.clone(),
                    name,
                },
            );
            let success: SuccessResponse = serde_json::from_str(&response).unwrap();
            assert!(matches!(success.result, ResponseResult::AgentInfo { .. }));
            assert_eq!(
                app.state.terminals[&terminal_id].manual_label.as_deref(),
                Some("shell-pane")
            );
        }
    }
}
