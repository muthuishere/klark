use super::*;

fn ago(now: u64, then: Option<u64>) -> String {
    let Some(then) = then else {
        return "-".into();
    };
    let d = now.saturating_sub(then);
    match d {
        0..=59 => format!("{d}s ago"),
        60..=3599 => format!("{}m ago", d / 60),
        3600..=86_399 => format!("{}h ago", d / 3600),
        _ => format!("{}d ago", d / 86_400),
    }
}

fn cell(text: &str, width: usize) -> String {
    let mut out: String = text.chars().take(width).collect();
    let used = out.chars().count();
    out.extend(std::iter::repeat_n(' ', width - used + 1));
    out
}

/// The feeds overlay: one row per feed (id, label, kind, agents, state, last
/// delivery). Row hits go to `settings_choices`, the popup to
/// `settings_popup`.
pub(super) fn render_feeds_overlay(
    b: &mut Buffer,
    overlay: &ClientFeedsOverlay,
    p: &Palette,
) -> Option<OverlayRender> {
    let rows = match overlay.tab {
        JobsTab::Feeds => overlay.feeds.len(),
        JobsTab::Hooks => overlay.hooks.len(),
    }
    .max(1) as u16;
    let popup = popup(b.area, 110, rows.saturating_add(8).max(10))?;
    let inner = panel(b, popup, p.accent, p.panel_bg)?;
    let bold = Style::default()
        .fg(p.text)
        .bg(p.panel_bg)
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(p.overlay1).bg(p.panel_bg);
    // " jobs   feeds  hooks ": the open tab in the accent colour.
    put_text(b, inner.x, inner.y, inner.width, " jobs", bold);
    let mut x = inner.x + 8;
    for (label, tab) in [("feeds", JobsTab::Feeds), ("hooks", JobsTab::Hooks)] {
        let style = if overlay.tab == tab {
            Style::default()
                .fg(contrast(p))
                .bg(p.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            dim
        };
        let text = format!(" {label} ");
        put_text(b, x, inner.y, inner.right().saturating_sub(x), &text, style);
        x += text.len() as u16 + 1;
    }
    if overlay.tab == JobsTab::Hooks {
        return render_hooks_tab(b, overlay, p, popup, inner, dim);
    }
    let widths = [14usize, 14, 6, 30, 22, 12];
    let header = ["ID", "LABEL", "KIND", "AGENTS", "STATE", "LAST SENT"];
    let line = |cells: [&str; 6]| -> String {
        let mut out = String::from(" ");
        for (text, width) in cells.iter().zip(widths) {
            out.push_str(&cell(text, width));
        }
        out
    };
    put_text(b, inner.x, inner.y + 2, inner.width, &line(header), dim);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut hits = Vec::new();
    let body_y = inner.y + 3;
    if overlay.feeds.is_empty() {
        let text = if overlay.loading {
            " loading feeds…"
        } else {
            " no feeds (klark feed add --help)"
        };
        put_text(b, inner.x, body_y, inner.width, text, dim);
    }
    for (index, feed) in overlay.feeds.iter().enumerate() {
        let y = body_y + index as u16;
        if y >= inner.bottom().saturating_sub(3) {
            break;
        }
        let rect = Rect::new(inner.x, y, inner.width, 1);
        let selected = index == overlay.selected;
        let style = if selected {
            Style::default()
                .fg(contrast(p))
                .bg(p.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(p.text).bg(p.panel_bg)
        };
        b.set_style(rect, style);
        let agents = feed.agents.join(", ");
        let last = ago(now, feed.last_delivery);
        let text = line([
            &feed.id,
            &feed.label,
            &feed.kind,
            &agents,
            &feed.state,
            &last,
        ]);
        put_text(b, rect.x, rect.y, rect.width, &text, style);
        hits.push((rect, index));
    }
    if let Some(message) = overlay.message.as_deref() {
        let color = if overlay.confirm_stop.is_some() {
            p.red
        } else {
            p.accent
        };
        put_text(
            b,
            inner.x,
            inner.bottom().saturating_sub(2),
            inner.width,
            &format!(" {message}"),
            Style::default().fg(color).bg(p.panel_bg),
        );
    }
    put_text(
        b,
        inner.x,
        inner.bottom().saturating_sub(1),
        inner.width,
        " ↑↓ select  s start  x stop  o settings  r refresh  tab hooks  esc close",
        dim,
    );
    Some(OverlayRender {
        area: popup,
        settings_popup: popup,
        settings_choices: hits,
        ..OverlayRender::default()
    })
}

/// The hooks tab: one row per configured hook (name, kind, state, pid,
/// restarts, last exit, uptime, restart policy).
fn render_hooks_tab(
    b: &mut Buffer,
    overlay: &ClientFeedsOverlay,
    p: &Palette,
    popup_area: Rect,
    inner: Rect,
    dim: Style,
) -> Option<OverlayRender> {
    let widths = [22usize, 6, 12, 8, 9, 5, 6, 11];
    let header = [
        "NAME", "KIND", "STATE", "PID", "RESTARTS", "EXIT", "UP", "RESTART",
    ];
    let line = |cells: [&str; 8]| -> String {
        let mut out = String::from(" ");
        for (text, width) in cells.iter().zip(widths) {
            out.push_str(&cell(text, width));
        }
        out
    };
    let supervisor = if overlay.hooks_running {
        "hooks running on this machine"
    } else {
        "hooks not running (they start with the first klark server; s starts them)"
    };
    put_text(
        b,
        inner.x,
        inner.y + 1,
        inner.width,
        &format!(" {supervisor}"),
        dim,
    );
    put_text(b, inner.x, inner.y + 2, inner.width, &line(header), dim);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let body_y = inner.y + 3;
    if overlay.hooks.is_empty() {
        put_text(
            b,
            inner.x,
            body_y,
            inner.width,
            " no hooks: add [[hooks.start]] to config.toml (klark hooks --help)",
            dim,
        );
    }
    let mut hits = Vec::new();
    for (index, hook) in overlay.hooks.iter().enumerate() {
        let y = body_y + index as u16;
        if y >= inner.bottom().saturating_sub(3) {
            break;
        }
        let rect = Rect::new(inner.x, y, inner.width, 1);
        let state_color = match hook.state.as_str() {
            "running" | "done" => p.green,
            "failed" | "backoff" => p.red,
            "stopped" | "not running" => p.overlay1,
            _ => p.text,
        };
        let style = if index == overlay.hook_selected {
            Style::default()
                .fg(contrast(p))
                .bg(p.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(state_color).bg(p.panel_bg)
        };
        b.set_style(rect, style);
        let pid = hook.pid.map_or("-".into(), |p| p.to_string());
        let restarts = hook.restarts.to_string();
        let exit = hook.last_exit.map_or("-".into(), |c| c.to_string());
        let up = match (hook.state.as_str(), hook.started_at) {
            ("running", Some(t)) => ago(now, Some(t)).trim_end_matches(" ago").to_string(),
            _ => "-".into(),
        };
        let text = line([
            &hook.name,
            &hook.kind,
            &hook.state,
            &pid,
            &restarts,
            &exit,
            &up,
            &hook.restart,
        ]);
        put_text(b, rect.x, rect.y, rect.width, &text, style);
        hits.push((rect, index));
    }
    if let Some(message) = overlay.message.as_deref() {
        put_text(
            b,
            inner.x,
            inner.bottom().saturating_sub(2),
            inner.width,
            &format!(" {message}"),
            Style::default().fg(p.accent).bg(p.panel_bg),
        );
    }
    put_text(
        b,
        inner.x,
        inner.bottom().saturating_sub(1),
        inner.width,
        " ↑↓ select  s start  x stop  e restart  r refresh  tab feeds  esc close",
        dim,
    );
    Some(OverlayRender {
        area: popup_area,
        settings_popup: popup_area,
        settings_choices: hits,
        ..OverlayRender::default()
    })
}
