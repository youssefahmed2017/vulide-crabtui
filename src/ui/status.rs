//! The one-row status bar: file, cursor position, transient messages.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::App;

pub fn render(f: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let base = Style::default()
        .fg(theme.statusbar_fg)
        .bg(theme.statusbar_bg);
    let accent = Style::default().fg(theme.accent).bg(theme.statusbar_bg);

    let cursor = app.buf().cursor();
    let tab_of = if app.buffers.len() > 1 {
        format!("[{}/{}] ", app.active + 1, app.buffers.len())
    } else {
        String::new()
    };
    let pos = format!(" {tab_of}Ln {}, Col {} ", cursor.line + 1, cursor.col + 1);

    let left = if app.status.is_empty() {
        format!(" {} ", app.buf().title())
    } else {
        format!(" {} ", app.status)
    };

    let used = left.chars().count() + pos.chars().count();
    let gap = (area.width as usize).saturating_sub(used);
    let line = Line::from(vec![
        Span::styled(left, accent),
        Span::styled(" ".repeat(gap), base),
        Span::styled(pos, base),
    ]);

    f.render_widget(Paragraph::new(line).style(base), area);
}
