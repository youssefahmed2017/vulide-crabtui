//! The modal overlay layer. Phase 1.5 has one member — the **Save As** prompt,
//! opened when `Ctrl+S` hits an untitled buffer. Phase 3's command palette and
//! file picker reuse `centered_rect` and the same "captures all input while
//! open" contract.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Position as TermPos, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};

use crate::buffer::Buffer;
use crate::theme::Theme;

pub enum Overlay {
    None,
    SaveAs(Box<SaveAs>),
}

impl Overlay {
    pub fn is_open(&self) -> bool {
        !matches!(self, Overlay::None)
    }
}

/// What the app should do with a key the Save As prompt just consumed.
pub enum SaveAsOutcome {
    /// Stay open, keep editing the path.
    Stay,
    /// User cancelled.
    Cancel,
    /// User confirmed this (already whitespace-trimmed) path.
    Submit(String),
}

pub struct SaveAs {
    /// Single-line path editor — a one-line `Buffer` so it reuses the tested
    /// insert/delete/movement code.
    input: Buffer,
    pub error: Option<String>,
}

impl SaveAs {
    pub fn new(seed: &str) -> Self {
        let mut input = Buffer::from_str(seed);
        input.move_doc_end(false);
        Self { input, error: None }
    }

    pub fn path(&self) -> String {
        self.input.rope().to_string()
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> SaveAsOutcome {
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => SaveAsOutcome::Cancel,
            KeyCode::Enter => SaveAsOutcome::Submit(self.path().trim().to_string()),
            KeyCode::Backspace => {
                self.input.delete_backward();
                SaveAsOutcome::Stay
            }
            KeyCode::Delete => {
                self.input.delete_forward();
                SaveAsOutcome::Stay
            }
            KeyCode::Left => {
                self.input.move_left(false);
                SaveAsOutcome::Stay
            }
            KeyCode::Right => {
                self.input.move_right(false);
                SaveAsOutcome::Stay
            }
            KeyCode::Home => {
                self.input.move_home(false);
                SaveAsOutcome::Stay
            }
            KeyCode::End => {
                self.input.move_end(false);
                SaveAsOutcome::Stay
            }
            KeyCode::Char(c) if plain && c != '\n' => {
                self.input.insert_char(c);
                self.error = None;
                SaveAsOutcome::Stay
            }
            _ => SaveAsOutcome::Stay,
        }
    }
}

/// Expand a leading `~` / `~/` to `$HOME` (Linux target — good enough for now).
pub fn expand_tilde(path: &str) -> String {
    if (path == "~" || path.starts_with("~/"))
        && let Ok(home) = std::env::var("HOME")
    {
        return format!("{home}{}", &path[1..]);
    }
    path.to_string()
}

pub fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    }
}

pub fn render_save_as(f: &mut Frame, prompt: &SaveAs, theme: &Theme, area: Rect) {
    let extra = if prompt.error.is_some() { 2 } else { 0 };
    let rect = centered_rect(64, 7 + extra, area);
    f.render_widget(Clear, rect);

    let panel = Style::default().fg(theme.fg).bg(theme.statusbar_bg);
    let muted = Style::default()
        .fg(theme.statusbar_fg)
        .bg(theme.statusbar_bg);
    let block = Block::default()
        .borders(Borders::ALL)
        .padding(Padding::symmetric(2, 1))
        .border_style(Style::default().fg(theme.accent).bg(theme.statusbar_bg))
        .title(Span::styled(
            " Save As ",
            Style::default()
                .fg(theme.accent)
                .bg(theme.statusbar_bg)
                .add_modifier(Modifier::BOLD),
        ))
        .style(panel);
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let label = "Path: ";
    let mut lines = vec![
        Line::from(vec![
            Span::styled(label, muted),
            Span::styled(prompt.path(), panel),
        ]),
        Line::default(),
    ];
    if let Some(err) = &prompt.error {
        lines.push(Line::from(Span::styled(
            err.clone(),
            Style::default()
                .fg(theme.output_err)
                .bg(theme.statusbar_bg)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::default());
    }
    lines.push(Line::from(vec![
        Span::styled("[ Cancel ]", muted),
        Span::styled("   ", panel),
        Span::styled(
            "[ Save ]",
            Style::default()
                .fg(theme.accent)
                .bg(theme.statusbar_bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("      Enter save · Esc cancel", muted),
    ]));
    f.render_widget(Paragraph::new(lines).style(panel), inner);

    // caret in the path field (first inner row)
    let cx = inner.x + label.len() as u16 + prompt.input.cursor().col as u16;
    if cx < inner.x + inner.width {
        f.set_cursor_position(TermPos::new(cx, inner.y));
    }
}
