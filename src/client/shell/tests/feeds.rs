//! ADR 0015 (M5): the settings dialog's `feeds` tab (A2) and the global
//! menu's `feeds` overlay (A3).

use super::*;
use crate::api::schema::{FeedInfo, Method, ResponseResult};

fn endpoint_requests(actions: &[ClientShellAction]) -> Vec<crate::api::schema::Request> {
    actions
        .iter()
        .filter_map(|action| match action {
            ClientShellAction::Endpoint { request, .. } => Some((**request).clone()),
            _ => None,
        })
        .collect()
}

fn frame_text(frame: &crate::client::frame_output::ComposedFrame) -> String {
    frame
        .cells
        .chunks(frame.width as usize)
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn feed(id: &str) -> FeedInfo {
    FeedInfo {
        id: id.into(),
        label: id.into(),
        kind: "stream".into(),
        agents: vec!["default/claude".into()],
        state: "stopped: too many updates".into(),
        running: false,
        last_delivery: None,
    }
}

/// A2: Enter on each row of the `feeds` tab cycles its value, and the value
/// is in `config.toml` afterwards; a value that is not a preset is shown as
/// is and moves to the first preset.
#[test]
fn feeds_settings_tab_cycles_each_setting_into_config_toml() {
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    let path = std::env::temp_dir().join(format!(
        "klark-feeds-settings-{}-{}.toml",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::write(
        &path,
        "# mine\n[feeds]\nmax_bytes = 3000\nidle_stop = \"168h\"\n",
    )
    .unwrap();
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

    let mut config =
        ClientShellConfig::from_config(&crate::config::load_live_config().unwrap().config);
    config.local_config_path = path.clone();
    let mut state = ClientShellState::new(config);
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    state.open_settings_overlay();
    let mut outcome = ClientShellInput::default();
    state.select_settings_section(ClientSettingsSection::Feeds, &mut outcome);
    // `168h` is the 7 days preset; 3000 is not a preset and shows as is.
    assert_eq!(
        crate::client::shell::settings::feed_setting_display(&state.config.feeds, 5),
        "7 days"
    );
    assert_eq!(
        crate::client::shell::settings::feed_setting_display(&state.config.feeds, 6),
        "3000"
    );
    let frame = state.compose(120, 30).expect("feeds tab frame");
    let text = frame_text(&frame);
    assert!(
        text.contains("max bytes") && text.contains("3000"),
        "{text}"
    );

    // (row, the `[feeds]` key, its value after one Enter)
    let expected = [
        (0, "restore", toml::Value::Boolean(false)),
        (1, "agents_may_create", toml::Value::Boolean(false)),
        (2, "message_gap_seconds", toml::Value::Integer(60)),
        (3, "poll_interval_seconds", toml::Value::Integer(60)),
        (4, "max_per_hour", toml::Value::Integer(60)),
        (5, "idle_stop", toml::Value::String("30d".into())),
        (6, "max_bytes", toml::Value::Integer(2048)),
        (7, "use_small_subagent", toml::Value::Boolean(false)),
    ];
    for (row, key, value) in expected {
        state.select_settings_choice(row);
        state.handle_input_bytes(b"\r");
        let written: toml::Table =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["feeds"][key], value, "row {row} ({key})");
    }
    // Cycling wraps: max per hour 60 -> 120 -> no limit -> 10.
    state.select_settings_choice(4);
    for want in [120, 0, 10] {
        state.handle_input_bytes(b"\r");
        let written: toml::Table =
            toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["feeds"]["max_per_hour"].as_integer(), Some(want));
    }
    assert_eq!(
        crate::client::shell::settings::feed_setting_display(&state.config.feeds, 4),
        "10"
    );
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("# mine"), "{content}");
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Settings(ClientSettingsOverlay {
            section: ClientSettingsSection::Feeds,
            ..
        }))
    ));

    std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
    let _ = std::fs::remove_file(&path);
}

/// A3: the global menu's `feeds` entry opens the overlay, which asks the
/// endpoint for the list; stop and start act on the highlighted feed, stop
/// only after a second press; `o` opens the settings' `feeds` tab.
#[test]
fn feeds_menu_entry_opens_overlay_and_acts_on_highlighted_feed() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let index = super::super::global_menu::global_menu_items(&snapshot())
        .iter()
        .position(|(label, _)| *label == "feeds")
        .expect("feeds menu entry");
    let settings = super::super::global_menu::global_menu_items(&snapshot())
        .iter()
        .position(|(label, _)| *label == "settings")
        .unwrap();
    assert_eq!(index, settings + 1, "feeds sits beside settings");
    state.toggle_global_menu();
    let mut outcome = ClientShellInput::default();
    state.activate_global_menu_item(index, &mut outcome);
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Feeds(ClientFeedsOverlay {
            loading: true,
            ..
        }))
    ));
    let requests = endpoint_requests(&outcome.actions);
    assert!(matches!(&requests[..], [r] if matches!(r.method, Method::FeedList(_))));

    let (repaint, _) = state.handle_endpoint_result(
        "boot-1",
        &requests[0].id,
        Ok(ResponseResult::FeedList {
            feeds: vec![feed("first"), feed("second")],
        }),
    );
    assert!(repaint);
    let frame = state.compose(120, 30).expect("feeds overlay frame");
    let text = frame_text(&frame);
    assert!(
        text.contains("second") && text.contains("LAST SENT"),
        "{text}"
    );

    state.handle_input_bytes(b"j");
    let start = state.handle_input_bytes(b"s");
    let requests = endpoint_requests(&start.actions);
    let [request] = &requests[..] else {
        panic!("one start request: {requests:?}");
    };
    assert!(matches!(&request.method, Method::FeedStart(t) if t.id == "second"));
    let (_, refresh) = state.handle_endpoint_result(
        "boot-1",
        &request.id,
        Ok(ResponseResult::FeedStarted {
            id: "second".into(),
            state: "running".into(),
        }),
    );
    // A start refreshes the list.
    let refresh = endpoint_requests(&refresh);
    assert!(matches!(&refresh[..], [r] if matches!(r.method, Method::FeedList(_))));
    state.handle_endpoint_result(
        "boot-1",
        &refresh[0].id,
        Ok(ResponseResult::FeedList {
            feeds: vec![feed("first"), feed("second")],
        }),
    );

    // The first stop key only asks.
    let ask = state.handle_input_bytes(b"x");
    assert!(endpoint_requests(&ask.actions).is_empty());
    let stop = state.handle_input_bytes(b"x");
    let requests = endpoint_requests(&stop.actions);
    assert!(
        matches!(&requests[..], [r] if matches!(&r.method, Method::FeedStop(t) if t.id == "second")),
        "{requests:?}"
    );
    state.handle_endpoint_result(
        "boot-1",
        &requests[0].id,
        Ok(ResponseResult::FeedStopped {
            id: "second".into(),
        }),
    );

    state.handle_input_bytes(b"o");
    assert!(matches!(
        state.overlay,
        Some(ClientShellOverlay::Settings(ClientSettingsOverlay {
            section: ClientSettingsSection::Feeds,
            ..
        }))
    ));
}

/// A4: from a client whose active machine is a remote (SSH) endpoint, the
/// overlay asks that endpoint (its id and boot), and stop and start go there
/// too; nothing is read from this machine's feed store.
#[test]
fn feeds_overlay_works_against_a_remote_endpoint() {
    use crate::client::endpoint::{ClientEndpointId, ClientEndpointStatus, ProfileId};
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let profile = crate::client::endpoint::SavedSshEndpoint {
        id: ProfileId::parse("0123456789abcdef0123456789abcdef").unwrap(),
        label: "Build".into(),
        target: "dev@build.example".into(),
        session: "agents".into(),
        enabled: true,
    };
    let remote = ClientEndpointId::Ssh(profile.id.clone());
    state.set_endpoint_catalog(&[profile]);
    state.set_endpoint_status(&remote, ClientEndpointStatus::Online);
    let mut projection = snapshot();
    projection.boot_id = "remote-boot".into();
    state.set_endpoint_snapshot(&remote, Box::new(projection));
    assert!(state.activate_endpoint_projection(&remote));

    let mut outcome = ClientShellInput::default();
    state.open_feeds_overlay(&mut outcome);
    let [ClientShellAction::Endpoint {
        endpoint_id,
        boot_id,
        request,
    }] = &outcome.actions[..]
    else {
        panic!("one endpoint request: {:?}", outcome.actions);
    };
    assert_eq!(endpoint_id, &remote);
    assert_eq!(boot_id, "remote-boot");
    assert!(matches!(request.method, Method::FeedList(_)));
    let list_id = request.id.clone();

    // A reply carrying the local boot id is not this request's.
    assert!(
        !state
            .handle_endpoint_result(
                "boot-1",
                &list_id,
                Ok(ResponseResult::FeedList { feeds: Vec::new() })
            )
            .0
    );
    let mut outcome = ClientShellInput::default();
    state.overlay = None;
    state.open_feeds_overlay(&mut outcome);
    let list_id = endpoint_requests(&outcome.actions)[0].id.clone();
    let (repaint, _) = state.handle_endpoint_result(
        "remote-boot",
        &list_id,
        Ok(ResponseResult::FeedList {
            feeds: vec![feed("remote-feed")],
        }),
    );
    assert!(repaint);
    assert!(matches!(
        &state.overlay,
        Some(ClientShellOverlay::Feeds(o)) if o.feeds.len() == 1 && o.feeds[0].id == "remote-feed"
    ));

    let start = state.handle_input_bytes(b"s");
    let [ClientShellAction::Endpoint {
        endpoint_id,
        request,
        ..
    }] = &start.actions[..]
    else {
        panic!("one start request: {:?}", start.actions);
    };
    assert_eq!(endpoint_id, &remote);
    assert!(matches!(&request.method, Method::FeedStart(t) if t.id == "remote-feed"));
}

fn open_feeds_with(state: &mut ClientShellState, ids: &[&str]) {
    let mut outcome = ClientShellInput::default();
    state.open_feeds_overlay(&mut outcome);
    let requests = endpoint_requests(&outcome.actions);
    state.handle_endpoint_result(
        "boot-1",
        &requests[0].id,
        Ok(ResponseResult::FeedList {
            feeds: ids.iter().map(|id| feed(id)).collect(),
        }),
    );
}

/// A pending "press x again" for a feed that vanished from a refreshed list
/// is dropped: it must not stay on screen for the feed now highlighted.
#[test]
fn feeds_overlay_drops_stop_confirmation_when_its_feed_vanishes() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    open_feeds_with(&mut state, &["first", "second"]);
    state.handle_input_bytes(b"j");
    state.handle_input_bytes(b"x");
    let refresh = state.handle_input_bytes(b"r");
    let requests = endpoint_requests(&refresh.actions);
    state.handle_endpoint_result(
        "boot-1",
        &requests[0].id,
        Ok(ResponseResult::FeedList {
            feeds: vec![feed("first")],
        }),
    );
    let Some(ClientShellOverlay::Feeds(feeds)) = &state.overlay else {
        panic!("feeds overlay");
    };
    assert_eq!(feeds.selected, 0);
    assert_eq!(feeds.confirm_stop, None);
    assert_eq!(feeds.message, None);
    // One x only asks again.
    let ask = state.handle_input_bytes(b"x");
    assert!(endpoint_requests(&ask.actions).is_empty());
}

/// `r` while a list is already in flight sends no second request.
#[test]
fn feeds_overlay_refresh_while_loading_sends_one_request() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    let mut outcome = ClientShellInput::default();
    state.open_feeds_overlay(&mut outcome);
    assert_eq!(endpoint_requests(&outcome.actions).len(), 1);
    let again = state.handle_input_bytes(b"r");
    assert!(endpoint_requests(&again.actions).is_empty());
}

/// Moving off a feed after the first stop key drops its "press x again".
#[test]
fn feeds_overlay_moving_selection_drops_stop_confirmation() {
    let mut state = ClientShellState::new(ClientShellConfig::from_config(&Config::default()));
    state.set_snapshot(Box::new(snapshot()));
    state.set_pane_surface(surface());
    open_feeds_with(&mut state, &["first", "second"]);
    state.handle_input_bytes(b"x");
    state.handle_input_bytes(b"j");
    let Some(ClientShellOverlay::Feeds(feeds)) = &state.overlay else {
        panic!("feeds overlay");
    };
    assert_eq!(feeds.confirm_stop, None);
    assert_eq!(feeds.message, None);
}
