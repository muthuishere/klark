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
    let rows = overlay.feeds.len().max(1) as u16;
    let popup = popup(b.area, 110, rows.saturating_add(8).max(10))?;
    let inner = panel(b, popup, p.accent, p.panel_bg)?;
    let bold = Style::default()
        .fg(p.text)
        .bg(p.panel_bg)
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(p.overlay1).bg(p.panel_bg);
    put_text(b, inner.x, inner.y, inner.width, " feeds", bold);
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
        " ↑↓ select  s start  x stop  o settings  r refresh  esc close",
        dim,
    );
    Some(OverlayRender {
        area: popup,
        settings_popup: popup,
        settings_choices: hits,
        ..OverlayRender::default()
    })
}
