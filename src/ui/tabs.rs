//! The tab strip — one row listing every open buffer, the active one lit.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;

pub fn render(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let strip = Style::default().fg(theme.tab_bg).bg(theme.tab_bg);
    let inactive = Style::default().fg(theme.toolbar_fg).bg(theme.tab_bg);
    let active = Style::default()
        .fg(theme.tab_active)
        .bg(theme.bg)
        .add_modifier(Modifier::BOLD);

    let mut spans: Vec<Span> = Vec::new();
    for (i, buf) in app.buffers.iter().enumerate() {
        let label = format!(" {} ", buf.title());
        spans.push(Span::styled(
            label,
            if i == app.active { active } else { inactive },
        ));
        spans.push(Span::styled("│", strip));
    }

    f.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme.tab_bg)),
        area,
    );
}
