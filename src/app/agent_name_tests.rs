//! Automatic agent names: every agent pane gets a name from its folder so
//! `klark msg ls` addresses say which agent is which.

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::api::schema::{AgentRenameParams, Method, Request};
use crate::app::{App, Mode};
use crate::config::Config;
use crate::detect::{Agent, AgentState};
use crate::events::AppEvent;
use crate::layout::PaneId;
use crate::terminal::TerminalId;
use crate::workspace::Workspace;

fn app_with_panes(count: usize) -> (App, Vec<PaneId>) {
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(
        &Config::default(),
        crate::app::AppPolicy::TEST,
        None,
        api_rx,
        crate::api::EventHub::default(),
    );
    let mut workspace = Workspace::test_new("agent-names");
    let mut panes = vec![workspace.tabs[0].root_pane];
    for _ in 1..count {
        panes.push(workspace.test_split(ratatui::layout::Direction::Horizontal));
    }
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    app.state.active = Some(0);
    app.state.selected = 0;
    app.state.mode = Mode::Terminal;
    (app, panes)
}

fn terminal_id(app: &App, pane: PaneId) -> TerminalId {
    app.state.workspaces[0]
        .pane_state(pane)
        .unwrap()
        .attached_terminal_id
        .clone()
}

fn set_cwd(app: &mut App, pane: PaneId, cwd: &Path) {
    let id = terminal_id(app, pane);
    app.state.terminals.get_mut(&id).unwrap().cwd = cwd.to_path_buf();
}

fn detect(app: &mut App, pane: PaneId, agent: Agent) {
    app.handle_internal_event(AppEvent::AgentProcessDetected {
        pane_id: pane,
        agent,
        observed_at: Instant::now(),
    });
}

fn name(app: &App, pane: PaneId) -> Option<String> {
    app.state.terminals[&terminal_id(app, pane)]
        .agent_name
        .clone()
}

fn rename(app: &mut App, target: &str, name: Option<&str>) -> serde_json::Value {
    let response = app.handle_api_request(Request {
        id: "req_rename".into(),
        method: Method::AgentRename(AgentRenameParams {
            target: target.into(),
            name: name.map(str::to_string),
        }),
    });
    serde_json::from_str(&response).unwrap()
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "klark-agent-names-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_detected_agent_is_named_after_its_folder() {
    let base = scratch("folder");
    let folder = base.join("Portfolio Plan");
    std::fs::create_dir_all(&folder).unwrap();
    let (mut app, panes) = app_with_panes(1);
    set_cwd(&mut app, panes[0], &folder);

    detect(&mut app, panes[0], Agent::Claude);

    assert_eq!(name(&app, panes[0]).as_deref(), Some("portfolio-plan"));
    let terminal = &app.state.terminals[&terminal_id(&app, panes[0])];
    assert!(terminal.agent_name_auto);
    let info = app.agent_info(0, panes[0]).unwrap();
    assert_eq!(info.name.as_deref(), Some("portfolio-plan"));
    assert!(app.state.session_dirty, "the name is saved like any other");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn agents_in_the_same_folder_get_numbered_names() {
    let base = scratch("same");
    let folder = base.join("klark");
    std::fs::create_dir_all(&folder).unwrap();
    let (mut app, panes) = app_with_panes(3);
    for pane in &panes {
        set_cwd(&mut app, *pane, &folder);
    }

    detect(&mut app, panes[0], Agent::Claude);
    detect(&mut app, panes[1], Agent::Codex);
    detect(&mut app, panes[2], Agent::Claude);

    assert_eq!(name(&app, panes[0]).as_deref(), Some("klark"));
    assert_eq!(name(&app, panes[1]).as_deref(), Some("klark-2"));
    assert_eq!(name(&app, panes[2]).as_deref(), Some("klark-3"));
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn an_explicit_name_is_never_replaced() {
    let base = scratch("explicit");
    let (mut app, panes) = app_with_panes(1);
    set_cwd(&mut app, panes[0], &base.join("repo"));
    let id = terminal_id(&app, panes[0]);
    let terminal = app.state.terminals.get_mut(&id).unwrap();
    terminal.set_detected_state(Some(Agent::Claude), AgentState::Idle);
    terminal.set_agent_name("reviewer".into());

    detect(&mut app, panes[0], Agent::Claude);
    app.assign_automatic_agent_names();

    assert_eq!(name(&app, panes[0]).as_deref(), Some("reviewer"));
    assert!(!app.state.terminals[&id].agent_name_auto);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn renaming_an_automatic_name_makes_it_explicit_and_clearing_keeps_it_cleared() {
    let base = scratch("rename");
    let (mut app, panes) = app_with_panes(1);
    set_cwd(&mut app, panes[0], &base.join("repo"));
    detect(&mut app, panes[0], Agent::Claude);
    assert_eq!(name(&app, panes[0]).as_deref(), Some("repo"));
    let pane_id = app.public_pane_id(0, panes[0]).unwrap();

    let renamed = rename(&mut app, "repo", Some("reviewer"));
    assert_eq!(renamed["result"]["agent"]["name"], "reviewer", "{renamed}");
    assert!(!app.state.terminals[&terminal_id(&app, panes[0])].agent_name_auto);

    let cleared = rename(&mut app, &pane_id, None);
    assert!(cleared["result"]["agent"]["name"].is_null(), "{cleared}");
    detect(&mut app, panes[0], Agent::Claude);
    app.assign_automatic_agent_names();
    assert_eq!(name(&app, panes[0]), None, "a cleared name stays cleared");

    // Once that agent leaves the pane (its process exits, the shell is back),
    // the next agent is named again.
    let exit_at = Instant::now();
    for (agent, process_exited, at) in [
        (Some(Agent::Claude), true, exit_at),
        (None, false, exit_at + std::time::Duration::from_millis(1)),
    ] {
        app.handle_internal_event(AppEvent::StateChanged {
            pane_id: panes[0],
            agent,
            state: AgentState::Idle,
            visible_blocker: false,
            visible_working: false,
            process_exited,
            observed_at: at,
        });
    }
    app.handle_internal_event(AppEvent::AgentProcessDetected {
        pane_id: panes[0],
        agent: Agent::Codex,
        observed_at: exit_at + std::time::Duration::from_secs(1),
    });
    assert_eq!(name(&app, panes[0]).as_deref(), Some("repo"));
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn restored_running_agents_without_a_name_are_named() {
    // A restore sets the detected agent on the terminal directly, no event;
    // App construction then runs the same pass as below.
    let base = scratch("restore");
    let (mut app, panes) = app_with_panes(2);
    set_cwd(&mut app, panes[0], &base.join("api"));
    set_cwd(&mut app, panes[1], &base.join("api"));
    for pane in &panes {
        let id = terminal_id(&app, *pane);
        app.state
            .terminals
            .get_mut(&id)
            .unwrap()
            .set_detected_state(Some(Agent::Claude), AgentState::Idle);
    }
    app.state.session_dirty = false;

    app.assign_automatic_agent_names();

    assert_eq!(name(&app, panes[0]).as_deref(), Some("api"));
    assert_eq!(name(&app, panes[1]).as_deref(), Some("api-2"));
    assert!(app.state.session_dirty);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn pane_ids_and_automatic_names_both_resolve() {
    let base = scratch("targets");
    let (mut app, panes) = app_with_panes(2);
    set_cwd(&mut app, panes[0], &base.join("web"));
    set_cwd(&mut app, panes[1], &base.join("web"));
    detect(&mut app, panes[0], Agent::Claude);
    detect(&mut app, panes[1], Agent::Claude);

    for (pane, auto) in [(panes[0], "web"), (panes[1], "web-2")] {
        let pane_id = app.public_pane_id(0, pane).unwrap();
        assert_eq!(app.resolve_agent_target(&pane_id).unwrap().pane_id, pane);
        assert_eq!(app.resolve_terminal_target(&pane_id).unwrap().pane_id, pane);
        assert_eq!(app.resolve_agent_target(auto).unwrap().pane_id, pane);
    }
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn an_automatic_name_does_not_keep_a_pane_listed_once_its_agent_is_gone() {
    let base = scratch("listed");
    let (mut app, panes) = app_with_panes(1);
    set_cwd(&mut app, panes[0], &base.join("repo"));
    let id = terminal_id(&app, panes[0]);
    app.state
        .terminals
        .get_mut(&id)
        .unwrap()
        .set_detected_state(Some(Agent::Claude), AgentState::Idle);
    app.assign_automatic_agent_names();
    assert!(app.agent_info(0, panes[0]).is_some());

    // Detection no longer sees an agent (e.g. suspended behind the shell).
    // An explicit name would keep the pane addressable; an automatic one not.
    app.state
        .terminals
        .get_mut(&id)
        .unwrap()
        .set_detected_state(None, AgentState::Unknown);
    let terminal = &app.state.terminals[&id];
    assert_eq!(terminal.effective_agent_label(), None);
    assert_eq!(terminal.agent_name.as_deref(), Some("repo"));
    assert!(
        app.agent_info(0, panes[0]).is_none(),
        "a plain shell must not be listed as an agent through its automatic name"
    );
    assert!(
        app.resolve_agent_target("repo").is_err(),
        "nor reached through it"
    );
    assert!(app.resolve_terminal_target("repo").is_err());
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn an_agent_in_a_linked_worktree_is_named_after_the_worktree_folder() {
    let base = scratch("worktree");
    let main = base.join("klark");
    let wt = base.join("klark-agent-names");
    std::fs::create_dir_all(main.join(".git/worktrees/klark-agent-names")).unwrap();
    std::fs::create_dir_all(wt.join("src/app")).unwrap();
    std::fs::create_dir_all(main.join("src")).unwrap();
    std::fs::write(
        wt.join(".git"),
        "gitdir: ../klark/.git/worktrees/klark-agent-names\n",
    )
    .unwrap();
    let (mut app, panes) = app_with_panes(2);
    set_cwd(&mut app, panes[0], &wt.join("src/app"));
    set_cwd(&mut app, panes[1], &main.join("src"));

    detect(&mut app, panes[0], Agent::Claude);
    detect(&mut app, panes[1], Agent::Claude);

    assert_eq!(name(&app, panes[0]).as_deref(), Some("klark-agent-names"));
    // A main checkout's subfolder names it as it is.
    assert_eq!(name(&app, panes[1]).as_deref(), Some("src"));
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn shell_panes_are_not_named() {
    let (mut app, panes) = app_with_panes(1);
    app.assign_automatic_agent_names();
    assert_eq!(name(&app, panes[0]), None);
}
