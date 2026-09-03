//! The editor widget: renders a `&mut Buffer` (mut only to reconcile scroll)
//! with a line-number gutter, Vulpin highlighting, selection, and a
//! matched-bracket highlight. Places the real terminal cursor.

use ratatui::Frame;
use ratatui::layout::{Position as TermPos, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};
use unicode_width::UnicodeWidthChar;

use crate::buffer::movement;
use crate::buffer::{Buffer, Position};
use crate::syntax::{Tokenizer, vulpin::VulpinTokenizer};
use crate::theme::Theme;

const GUTTER_MIN: u16 = 4;

pub fn render(f: &mut Frame, buf: &mut Buffer, theme: &Theme, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    f.render_widget(Block::default().style(Style::default().bg(theme.bg)), area);

    let total = buf.line_count().max(1);
    let gutter_w = ((total.to_string().len() as u16) + 1).max(GUTTER_MIN);
    let text_w = area.width.saturating_sub(gutter_w) as usize;
    let text_h = area.height as usize;
    if text_w == 0 {
        return;
    }

    let cursor = buf.cursor();
    let cursor_disp = movement::display_col(&buf.line_text(cursor.line), cursor.col);
    reconcile_scroll(buf, cursor, cursor_disp, text_w, text_h);

    let selection = buf.selection();
    let bracket_match = buf.matching_bracket();
    let cursor_bracket = bracket_pair_at(buf, cursor);
    let tokenizer = VulpinTokenizer;

    let mut lines: Vec<Line> = Vec::with_capacity(text_h);
    for row in 0..text_h {
        let ln = buf.scroll_top + row;
        if ln >= total {
            lines.push(Line::default());
            continue;
        }
        let is_current = ln == cursor.line;
        let gutter_style = Style::default().bg(theme.bg).fg(if is_current {
            theme.gutter_current_fg
        } else {
            theme.gutter_fg
        });
        let label = format!("{:>w$} ", ln + 1, w = (gutter_w - 1) as usize);

        let mut spans = vec![Span::styled(label, gutter_style)];
        spans.extend(styled_text(
            &buf.line_text(ln),
            ln,
            is_current,
            buf.scroll_left,
            text_w,
            selection,
            bracket_match,
            cursor_bracket,
            theme,
            &tokenizer,
        ));
        lines.push(Line::from(spans));
    }

    f.render_widget(Paragraph::new(lines), area);

    // Real cursor, only when on screen.
    if cursor.line >= buf.scroll_top && cursor.line < buf.scroll_top + text_h {
        let y = area.y + (cursor.line - buf.scroll_top) as u16;
        if cursor_disp >= buf.scroll_left {
            let x = area.x + gutter_w + (cursor_disp - buf.scroll_left) as u16;
            if x < area.x + area.width {
                f.set_cursor_position(TermPos::new(x, y));
            }
        }
    }
}

fn reconcile_scroll(
    buf: &mut Buffer,
    cursor: Position,
    cursor_disp: usize,
    text_w: usize,
    text_h: usize,
) {
    if cursor.line < buf.scroll_top {
        buf.scroll_top = cursor.line;
    } else if text_h > 0 && cursor.line >= buf.scroll_top + text_h {
        buf.scroll_top = cursor.line + 1 - text_h;
    }
    if cursor_disp < buf.scroll_left {
        buf.scroll_left = cursor_disp;
    } else if cursor_disp >= buf.scroll_left + text_w {
        buf.scroll_left = cursor_disp + 1 - text_w;
    }
}

#[allow(clippy::too_many_arguments)]
fn styled_text(
    text: &str,
    line: usize,
    is_current: bool,
    scroll_left: usize,
    text_w: usize,
    selection: Option<(Position, Position)>,
    bracket_match: Option<Position>,
    cursor_bracket: Option<Position>,
    theme: &Theme,
    tokenizer: &VulpinTokenizer,
) -> Vec<Span<'static>> {
    let base_bg = if is_current {
        theme.current_line_bg
    } else {
        theme.bg
    };
    let tokens = tokenizer.tokenize_line(text);

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut group = String::new();
    let mut group_style: Option<Style> = None;
    let mut disp = 0usize;

    for (col, ch) in text.chars().enumerate() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0).max(1);
        // horizontal scroll window
        if disp + w <= scroll_left {
            disp += w;
            continue;
        }
        if disp >= scroll_left + text_w {
            break;
        }

        let mut style = Style::default().bg(base_bg).fg(theme.fg);
        if let Some(kind) = tokens
            .iter()
            .find(|t| col >= t.start && col < t.end)
            .map(|t| t.kind)
            && let Some(fg) = theme.token_color(kind)
        {
            style = style.fg(fg);
        }
        let here = Position { line, col };
        if selection.is_some_and(|(a, b)| a <= here && here < b) {
            style = style.bg(theme.selection_bg);
        }
        if Some(here) == bracket_match || Some(here) == cursor_bracket {
            style = style
                .fg(theme.match_bracket_fg)
                .add_modifier(Modifier::BOLD);
        }

        if group_style == Some(style) {
            group.push(ch);
        } else {
            flush(&mut spans, &mut group, &mut group_style);
            group.push(ch);
            group_style = Some(style);
        }
        disp += w;
    }
    flush(&mut spans, &mut group, &mut group_style);
    spans
}

fn flush(spans: &mut Vec<Span<'static>>, group: &mut String, style: &mut Option<Style>) {
    if !group.is_empty() {
        spans.push(Span::styled(
            std::mem::take(group),
            style.unwrap_or_default(),
        ));
    }
    *style = None;
}

/// If the cursor sits on or just after a bracket, the position of that bracket.
fn bracket_pair_at(buf: &Buffer, cursor: Position) -> Option<Position> {
    let line = buf.line_text(cursor.line);
    let chars: Vec<char> = line.chars().collect();
    for c in [cursor.col, cursor.col.wrapping_sub(1)] {
        if c < chars.len() && "()[]{}".contains(chars[c]) {
            return Some(Position {
                line: cursor.line,
                col: c,
            });
        }
    }
    None
}
