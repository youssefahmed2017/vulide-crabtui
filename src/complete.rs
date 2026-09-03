//! `$identifier` autocomplete.
//!
//! Vulpin has no multi-character keywords — the useful completion is variable
//! references. When the cursor sits right after `$` + word characters, suggest
//! every identifier defined or referenced elsewhere in the buffer.
//!
//! The popup is non-modal: Up/Down move the selection, Tab accepts, Esc
//! dismisses, and any other key edits as normal and refreshes the list. Enter
//! is left to insert its newline (it just closes the popup first).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::buffer::Buffer;
use crate::theme::Theme;

const MAX_VISIBLE: usize = 8;

pub struct Completion {
    /// Char index of the `$` on the cursor's line.
    pub anchor_col: usize,
    /// Text typed after the `$` so far.
    pub prefix: String,
    /// Candidate names (without the `$`), sorted, none equal to `prefix`.
    pub items: Vec<String>,
    pub selected: usize,
}

impl Completion {
    /// Build a completion if the cursor is in a `$word` context, else `None`.
    pub fn detect(buffer: &Buffer) -> Option<Completion> {
        let cur = buffer.cursor();
        let line = buffer.line_text(cur.line);
        // `Position::col` is a char index — walk the line as chars, not bytes,
        // so a non-ASCII glyph earlier on the line can't misalign the slice.
        let chars: Vec<char> = line.chars().collect();
        let col = cur.col.min(chars.len());

        let mut start = col;
        while start > 0 && is_word_char(chars[start - 1]) {
            start -= 1;
        }
        if start == 0 || chars[start - 1] != '$' {
            return None;
        }
        let prefix: String = chars[start..col].iter().collect();

        let mut names: Vec<String> = Vec::new();
        for i in 0..buffer.line_count() {
            collect_names(&buffer.line_text(i), &mut names);
        }
        names.sort();
        names.dedup();
        names.retain(|n| n.starts_with(&prefix) && *n != prefix);
        if names.is_empty() {
            return None;
        }

        Some(Completion {
            anchor_col: start - 1,
            prefix,
            items: names,
            selected: 0,
        })
    }

    pub fn current(&self) -> &str {
        &self.items[self.selected]
    }

    /// The text to insert when accepted (the part not already typed).
    pub fn completion_tail(&self) -> &str {
        &self.current()[self.prefix.len()..]
    }

    pub fn move_up(&mut self) {
        self.selected = self.selected.checked_sub(1).unwrap_or(self.items.len() - 1);
    }

    pub fn move_down(&mut self) {
        self.selected = (self.selected + 1) % self.items.len();
    }
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Push every identifier this line defines or references:
///   - a leading `ident =` assignment (Vulpin's `parseStatement` default case)
///   - every `$ident` reference
fn collect_names(line: &str, out: &mut Vec<String>) {
    let bytes = line.as_bytes();

    // leading assignment target
    let mut i = 0;
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    if i < bytes.len() && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
        let s = i;
        while i < bytes.len() && is_word(bytes[i]) {
            i += 1;
        }
        let mut j = i;
        while j < bytes.len() && bytes[j] == b' ' {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == b'=' && bytes.get(j + 1) != Some(&b'=') {
            out.push(line[s..i].to_string());
        }
    }

    // `$ident` references
    let mut k = 0;
    while k + 1 < bytes.len() {
        if bytes[k] == b'$' && (bytes[k + 1].is_ascii_alphabetic() || bytes[k + 1] == b'_') {
            let s = k + 1;
            let mut e = s;
            while e < bytes.len() && is_word(bytes[e]) {
                e += 1;
            }
            out.push(line[s..e].to_string());
            k = e;
        } else {
            k += 1;
        }
    }
}

/// Draw the popup anchored under the `$` (falls back to above if no room).
pub fn render_popup(
    f: &mut Frame,
    c: &Completion,
    cursor_screen: (u16, u16),
    theme: &Theme,
    area: Rect,
) {
    let (cx, cy) = cursor_screen;
    let width = (c.items.iter().map(|s| s.len()).max().unwrap_or(1) + 2).clamp(6, 40) as u16;
    let visible = c.items.len().min(MAX_VISIBLE) as u16;

    // scroll the list so the selection is in view
    let top = if c.selected as u16 >= visible {
        c.selected as u16 + 1 - visible
    } else {
        0
    };

    let anchor_x = cx.saturating_sub(c.prefix.chars().count() as u16 + 1);
    let x = anchor_x.min(area.x + area.width.saturating_sub(width));
    let below = cy + 1 + visible <= area.y + area.height;
    let y = if below {
        cy + 1
    } else {
        cy.saturating_sub(visible)
    };

    let rect = Rect {
        x,
        y,
        width,
        height: visible,
    };
    f.render_widget(Clear, rect);

    let rows: Vec<Line> = (top..top + visible)
        .filter_map(|i| c.items.get(i as usize).map(|item| (i, item)))
        .map(|(i, item)| {
            let selected = i as usize == c.selected;
            let style = if selected {
                Style::default()
                    .fg(theme.autocomplete_fg)
                    .bg(theme.autocomplete_sel)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(theme.autocomplete_fg)
                    .bg(theme.autocomplete_bg)
            };
            Line::from(Span::styled(
                format!(" {item:<w$}", w = (width - 1) as usize),
                style,
            ))
        })
        .collect();

    f.render_widget(
        Paragraph::new(rows).style(Style::default().bg(theme.autocomplete_bg)),
        rect,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Position;

    #[test]
    fn suggests_referenced_identifiers() {
        let mut b = Buffer::from_str("G $name + $number\nG $na");
        b.set_cursor(Position { line: 1, col: 5 }, false); // right after `$na`
        let c = Completion::detect(&b).expect("in a $word context");
        assert_eq!(c.prefix, "na");
        assert_eq!(c.items, vec!["name".to_string()]);
        assert_eq!(c.completion_tail(), "me");
    }

    #[test]
    fn suggests_assignment_targets() {
        let mut b = Buffer::from_str("counter = 0\nG $c");
        b.set_cursor(Position { line: 1, col: 4 }, false);
        let c = Completion::detect(&b).unwrap();
        assert_eq!(c.items, vec!["counter".to_string()]);
    }

    #[test]
    fn none_outside_dollar_context() {
        let mut b = Buffer::from_str("name = 1\nG na");
        b.set_cursor(Position { line: 1, col: 4 }, false);
        assert!(Completion::detect(&b).is_none());
    }

    #[test]
    fn none_when_only_match_is_what_was_typed() {
        let mut b = Buffer::from_str("$name\n$name");
        b.set_cursor(Position { line: 1, col: 5 }, false);
        assert!(Completion::detect(&b).is_none());
    }
}
