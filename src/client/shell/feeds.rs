//! The global menu's `feeds` overlay (ADR 0015, M5). The list comes from the
//! endpoint (`feed.list`), never from local files: the client may be attached
//! to another machine than the server running the feeds.

use super::*;

impl ClientShellState {
    pub(super) fn open_feeds_overlay(&mut self, outcome: &mut ClientShellInput) {
        self.overlay = Some(ClientShellOverlay::Feeds(ClientFeedsOverlay::default()));
        self.queue_feed_list(outcome);
        outcome.repaint = true;
    }

    fn queue_feed_list(&mut self, outcome: &mut ClientShellInput) {
        if let Some(ClientShellOverlay::Feeds(feeds)) = self.overlay.as_mut() {
            // One list in flight at a time: its answer clears `loading`.
            if feeds.loading {
                return;
            }
            feeds.loading = true;
        }
        if !self.push_endpoint_method_with_kind(
            crate::api::schema::Method::FeedList(crate::api::schema::EmptyParams::default()),
            PendingEndpointKind::FeedList,
            outcome,
        ) {
            if let Some(ClientShellOverlay::Feeds(feeds)) = self.overlay.as_mut() {
                feeds.loading = false;
            }
        }
    }

    fn highlighted_feed_id(&self) -> Option<String> {
        match self.overlay.as_ref() {
            Some(ClientShellOverlay::Feeds(feeds)) => {
                feeds.feeds.get(feeds.selected).map(|feed| feed.id.clone())
            }
            _ => None,
        }
    }

    /// Stop (`stop == true`) or start the highlighted feed. Stop ends a feed
    /// for good, as `klark feed stop` does, so it asks for a second press.
    fn act_on_highlighted_feed(&mut self, stop: bool, outcome: &mut ClientShellInput) {
        let Some(id) = self.highlighted_feed_id() else {
            return;
        };
        let Some(ClientShellOverlay::Feeds(feeds)) = self.overlay.as_mut() else {
            return;
        };
        if feeds.busy || feeds.loading {
            return;
        }
        outcome.repaint = true;
        if stop && feeds.confirm_stop.as_deref() != Some(id.as_str()) {
            feeds.message = Some(format!(
                "stop {id}? it is removed for every agent; press x again"
            ));
            feeds.confirm_stop = Some(id);
            return;
        }
        feeds.confirm_stop = None;
        feeds.busy = true;
        feeds.message = Some(if stop {
            format!("stopping {id}…")
        } else {
            format!("starting {id}…")
        });
        let target = crate::api::schema::FeedTarget { id };
        let (method, kind) = if stop {
            (
                crate::api::schema::Method::FeedStop(target),
                PendingEndpointKind::FeedStop,
            )
        } else {
            (
                crate::api::schema::Method::FeedStart(target),
                PendingEndpointKind::FeedStart,
            )
        };
        if !self.push_endpoint_method_with_kind(method, kind, outcome) {
            if let Some(ClientShellOverlay::Feeds(feeds)) = self.overlay.as_mut() {
                feeds.busy = false;
                feeds.message = None;
            }
        }
    }

    pub(super) fn select_feed_row(&mut self, index: usize) {
        if let Some(ClientShellOverlay::Feeds(jobs)) = self.overlay.as_mut() {
            if jobs.tab == JobsTab::Hooks {
                if index < jobs.hooks.len() {
                    jobs.hook_selected = index;
                }
                return;
            }
        }
        if let Some(ClientShellOverlay::Feeds(feeds)) = self.overlay.as_mut() {
            if index < feeds.feeds.len() && index != feeds.selected {
                feeds.selected = index;
                // The "press x again" was about the feed left behind.
                if feeds.confirm_stop.take().is_some() {
                    feeds.message = None;
                }
            }
        }
    }

    fn move_feed_selection(&mut self, delta: isize) {
        let Some(ClientShellOverlay::Feeds(feeds)) = self.overlay.as_ref() else {
            return;
        };
        let last = feeds.feeds.len().saturating_sub(1) as isize;
        let next = (feeds.selected as isize + delta).clamp(0, last) as usize;
        self.select_feed_row(next);
    }

    pub(super) fn handle_feeds_endpoint_result(
        &mut self,
        kind: PendingEndpointKind,
        result: Result<crate::api::schema::ResponseResult, ClientShellEndpointError>,
    ) -> (bool, Vec<ClientShellAction>) {
        use crate::api::schema::ResponseResult;
        let Some(ClientShellOverlay::Feeds(feeds)) = self.overlay.as_mut() else {
            return (false, Vec::new());
        };
        let refresh = match (kind, result) {
            (PendingEndpointKind::FeedList, result) => {
                feeds.loading = false;
                match result {
                    Ok(ResponseResult::FeedList { feeds: list }) => {
                        let keep = feeds.feeds.get(feeds.selected).map(|f| f.id.clone());
                        feeds.feeds = list;
                        feeds.selected = keep
                            .and_then(|id| feeds.feeds.iter().position(|f| f.id == id))
                            .unwrap_or(feeds.selected)
                            .min(feeds.feeds.len().saturating_sub(1));
                        // A pending "press x again" is for one feed only.
                        let highlighted = feeds.feeds.get(feeds.selected).map(|f| &f.id);
                        if feeds.confirm_stop.is_some()
                            && feeds.confirm_stop.as_ref() != highlighted
                        {
                            feeds.confirm_stop = None;
                            feeds.message = None;
                        }
                    }
                    Ok(_) => {
                        feeds.message = Some("endpoint returned an unexpected feed list".into())
                    }
                    Err(error) => feeds.message = Some(error.message),
                }
                false
            }
            (_, result) => {
                feeds.busy = false;
                feeds.message = Some(match result {
                    Ok(ResponseResult::FeedStopped { id }) => format!("{id}: stopped and removed"),
                    Ok(ResponseResult::FeedStarted { id, state }) => format!("{id}: {state}"),
                    Ok(_) => "endpoint returned an unexpected feed result".into(),
                    Err(error) => error.message,
                });
                true
            }
        };
        let mut deferred = ClientShellInput::default();
        if refresh {
            self.queue_feed_list(&mut deferred);
        }
        (true, deferred.actions)
    }

    /// Keys of the feeds overlay: ↑↓/jk select, s start, x stop (twice),
    /// o the settings' `feeds` tab, r refresh, esc close.
    pub(super) fn route_feeds_key(
        &mut self,
        key: &crate::input::TerminalKey,
        outcome: &mut ClientShellInput,
    ) -> bool {
        if !matches!(self.overlay, Some(ClientShellOverlay::Feeds(_))) {
            return false;
        }
        let (code, modifiers) = crate::config::normalize_key_combo((key.code, key.modifiers));
        if !modifiers
            .difference(crossterm::event::KeyModifiers::SHIFT)
            .is_empty()
        {
            return true;
        }
        match code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.overlay = None;
                outcome.repaint = true;
                return true;
            }
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right => {
                self.switch_jobs_tab();
                outcome.repaint = true;
                return true;
            }
            _ => {}
        }
        if matches!(
            self.overlay,
            Some(ClientShellOverlay::Feeds(ClientFeedsOverlay {
                tab: JobsTab::Hooks,
                ..
            }))
        ) {
            self.route_hooks_key(code);
            outcome.repaint = true;
            return true;
        }
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_feed_selection(-1);
                outcome.repaint = true;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_feed_selection(1);
                outcome.repaint = true;
            }
            KeyCode::Char('x') => self.act_on_highlighted_feed(true, outcome),
            KeyCode::Char('s') => self.act_on_highlighted_feed(false, outcome),
            KeyCode::Char('r') => {
                self.queue_feed_list(outcome);
                outcome.repaint = true;
            }
            KeyCode::Char('o') => self.open_feed_settings(outcome),
            _ => {}
        }
        true
    }

    fn switch_jobs_tab(&mut self) {
        let Some(ClientShellOverlay::Feeds(jobs)) = self.overlay.as_mut() else {
            return;
        };
        jobs.tab = match jobs.tab {
            JobsTab::Feeds => JobsTab::Hooks,
            JobsTab::Hooks => JobsTab::Feeds,
        };
        jobs.message = None;
        jobs.confirm_stop = None;
        if jobs.tab == JobsTab::Hooks {
            refresh_hooks(jobs);
        }
    }

    /// Keys of the hooks tab: ↑↓/jk select, s start, x stop, e restart,
    /// r refresh. A hook stopped here stays stopped until started again.
    fn route_hooks_key(&mut self, code: KeyCode) {
        let Some(ClientShellOverlay::Feeds(jobs)) = self.overlay.as_mut() else {
            return;
        };
        let last = jobs.hooks.len().saturating_sub(1);
        let name = jobs
            .hooks
            .get(jobs.hook_selected)
            .map(|h| (h.name.clone(), h.kind == "start"));
        match code {
            KeyCode::Up | KeyCode::Char('k') => {
                jobs.hook_selected = jobs.hook_selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                jobs.hook_selected = (jobs.hook_selected + 1).min(last);
            }
            KeyCode::Char('r') => refresh_hooks(jobs),
            KeyCode::Char(c @ ('s' | 'x' | 'e')) => {
                let Some((name, start_hook)) = name else {
                    return;
                };
                if !start_hook {
                    jobs.message = Some(format!(
                        "{name} is a stop hook: it runs when the hooks stop"
                    ));
                    return;
                }
                let result = if !jobs.hooks_running {
                    if c == 's' || c == 'e' {
                        crate::hooks::start_pinned()
                            .map(|()| "starting the hooks supervisor (all start hooks)".to_string())
                    } else {
                        Ok("hooks are not running".to_string())
                    }
                } else {
                    let action = match c {
                        's' => crate::hooks::Action::Start,
                        'x' => crate::hooks::Action::Stop,
                        _ => crate::hooks::Action::Restart,
                    };
                    let word = match c {
                        's' => "starting",
                        'x' => "stopping",
                        _ => "restarting",
                    };
                    crate::hooks::send(action, Some(&name)).map(|path| {
                        // The supervisor handles it within 200 ms.
                        crate::hooks::wait_handled(&path, std::time::Duration::from_millis(600));
                        format!("{word} {name}")
                    })
                };
                jobs.message = Some(result.unwrap_or_else(|e| format!("hooks: {e}")));
                refresh_hooks(jobs);
            }
            _ => {}
        }
    }
}

/// Reads this machine's hooks and their state.
fn refresh_hooks(jobs: &mut ClientFeedsOverlay) {
    let (supervisor, hooks) = crate::hooks::snapshot();
    jobs.hooks_running = supervisor.is_some();
    jobs.hooks = hooks;
    jobs.hook_selected = jobs.hook_selected.min(jobs.hooks.len().saturating_sub(1));
}
