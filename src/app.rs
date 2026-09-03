//! Top-level application state and the event loop.
//!
//! One `App` owns everything the UI reads. Widgets are pure functions of it.
//! Phase 3 turns `buffer` into a `Vec<Buffer>` behind a tab bar.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::buffer::Buffer;
use crate::complete::Completion;
use crate::event::{AppEvent, EventSource};
use crate::theme::Theme;
use crate::ui;
use crate::ui::overlay::{Overlay, SaveAs, SaveAsOutcome, expand_tilde};

const TICK: Duration = Duration::from_millis(250);

pub struct App {
    pub buffer: Buffer,
    pub theme: Theme,
    pub themes: Vec<Theme>,
    pub theme_idx: usize,
    pub status: String,
    pub editor_rows: usize,
    pub overlay: Overlay,
    /// The live autocomplete popup, recomputed after every editing key.
    pub completion: Option<Completion>,
    should_quit: bool,
}

impl App {
    pub fn new() -> Self {
        let themes = Theme::builtins();
        Self {
            buffer: Buffer::new(),
            theme: themes[0].clone(),
            themes,
            theme_idx: 0,
            status: String::new(),
            editor_rows: 20,
            overlay: Overlay::None,
            completion: None,
            should_quit: false,
        }
    }

    /// Advance to the next bundled theme (Ctrl+T).
    pub fn cycle_theme(&mut self) {
        self.theme_idx = (self.theme_idx + 1) % self.themes.len();
        self.theme = self.themes[self.theme_idx].clone();
        let name = self.theme.name.clone();
        self.set_status(format!("theme: {name}"));
    }

    pub fn open_path(&mut self, path: PathBuf) -> Result<()> {
        self.buffer = Buffer::open(&path)?;
        self.set_status(format!("opened {}", path.display()));
        Ok(())
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let events = EventSource::new(TICK);
        terminal.draw(|f| ui::draw(f, self))?;
        // If `poll` keeps reporting the input fd ready but nothing usable comes
        // through, the terminal has gone away (closed / EOF). Bail rather than
        // spin at 100% CPU.
        let mut dead_reads = 0u32;
        while !self.should_quit {
            match events.next()? {
                Some(AppEvent::Tick) => dead_reads = 0,
                // Redraw only on a real event — a dropped event (focus in/out,
                // key-release) returns `None` and must not trigger a redraw.
                Some(ev) => {
                    dead_reads = 0;
                    self.handle_event(ev);
                    terminal.draw(|f| ui::draw(f, self))?;
                }
                None => {
                    dead_reads += 1;
                    if dead_reads > 256 {
                        anyhow::bail!("terminal input stream closed");
                    }
                }
            }
        }
        Ok(())
    }

    pub fn handle_event(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Key(key) => self.handle_key(key),
            AppEvent::Paste(text) => {
                self.buffer.insert_str(&text);
                self.clear_status();
            }
            AppEvent::Resize(..) | AppEvent::Tick => {}
        }
    }

    fn clear_status(&mut self) {
        self.status.clear();
    }

    /// Route a key to the Save As overlay while it is open. Returns `true` if
    /// the overlay consumed the key (the editor must not also see it).
    fn handle_overlay_key(&mut self, key: KeyEvent) -> bool {
        let outcome = match &mut self.overlay {
            Overlay::SaveAs(prompt) => prompt.handle_key(key),
            Overlay::None => return false,
        };
        match outcome {
            SaveAsOutcome::Stay => {}
            SaveAsOutcome::Cancel => {
                self.overlay = Overlay::None;
                self.set_status("save cancelled");
            }
            SaveAsOutcome::Submit(path) => {
                if path.is_empty() {
                    if let Overlay::SaveAs(prompt) = &mut self.overlay {
                        prompt.error = Some("enter a path".to_string());
                    }
                } else {
                    match self.buffer.save_as(expand_tilde(&path)) {
                        Ok(()) => {
                            self.overlay = Overlay::None;
                            self.set_status(format!("saved {}", self.buffer.title()));
                        }
                        Err(e) => {
                            if let Overlay::SaveAs(prompt) = &mut self.overlay {
                                prompt.error = Some(e.to_string());
                            }
                        }
                    }
                }
            }
        }
        true
    }

    fn handle_key(&mut self, key: KeyEvent) {
        if self.handle_overlay_key(key) {
            return;
        }
        if self.completion.is_some() && self.handle_completion_key(key) {
            return;
        }
        self.handle_key_inner(key);
        // The `$word` context under the cursor may have changed — re-scan.
        self.completion = Completion::detect(&self.buffer);
    }

    /// Route a key to the autocomplete popup. Returns `true` if it was consumed
    /// (navigation / accept / dismiss); `false` lets the key edit as normal and
    /// the popup refreshes afterwards.
    fn handle_completion_key(&mut self, key: KeyEvent) -> bool {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return false;
        }
        let Some(c) = &mut self.completion else {
            return false;
        };
        match key.code {
            KeyCode::Up => c.move_up(),
            KeyCode::Down => c.move_down(),
            KeyCode::Esc => self.completion = None,
            KeyCode::Tab => {
                let tail = c.completion_tail().to_string();
                self.buffer.insert_str(&tail);
                self.completion = None;
            }
            // Enter still inserts a newline; it just closes the popup first so it
            // can't silently swap in a half-typed name.
            KeyCode::Enter => {
                self.completion = None;
                return false;
            }
            _ => return false,
        }
        true
    }

    fn handle_key_inner(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let rows = self.editor_rows.max(1);

        // ---- app-level shortcuts (must not hold a &mut self.buffer) ----
        if ctrl {
            match key.code {
                // Ctrl+C and Ctrl+Q both quit for now. A dirty-buffer guard and a
                // modal `:` command line (BatScript wants Vim-like) land in Phase 1.5.
                KeyCode::Char('q') | KeyCode::Char('c') => {
                    self.should_quit = true;
                    return;
                }
                KeyCode::Char('t') => {
                    self.cycle_theme();
                    return;
                }
                KeyCode::Char('s') => {
                    if self.buffer.path().is_none() {
                        self.overlay = Overlay::SaveAs(Box::new(SaveAs::new(&default_save_seed())));
                    } else {
                        let msg = match self.buffer.save() {
                            Ok(()) => format!("saved {}", self.buffer.title()),
                            Err(e) => format!("save failed: {e}"),
                        };
                        self.set_status(msg);
                    }
                    return;
                }
                KeyCode::Char('z') if !shift => {
                    let ok = self.buffer.undo();
                    self.set_status(if ok { "" } else { "nothing to undo" });
                    return;
                }
                KeyCode::Char('z') if shift => {
                    self.buffer.redo();
                    self.clear_status();
                    return;
                }
                KeyCode::Char('y') => {
                    let ok = self.buffer.redo();
                    self.set_status(if ok { "" } else { "nothing to redo" });
                    return;
                }
                KeyCode::Char('a') => {
                    self.buffer.select_all();
                    return;
                }
                _ => {}
            }
        }

        let b = &mut self.buffer;
        match key.code {
            // ---- motion ----
            KeyCode::Left if ctrl => b.move_word_left(shift),
            KeyCode::Right if ctrl => b.move_word_right(shift),
            KeyCode::Left => b.move_left(shift),
            KeyCode::Right => b.move_right(shift),
            KeyCode::Up => b.move_up(shift),
            KeyCode::Down => b.move_down(shift),
            KeyCode::Home if ctrl => b.move_doc_start(shift),
            KeyCode::End if ctrl => b.move_doc_end(shift),
            KeyCode::Home => b.move_home(shift),
            KeyCode::End => b.move_end(shift),
            KeyCode::PageUp => b.move_page_up(rows, shift),
            KeyCode::PageDown => b.move_page_down(rows, shift),
            KeyCode::Esc => {
                let c = b.cursor();
                b.set_cursor(c, false);
            }

            // ---- edits ----
            KeyCode::Char(c) if !ctrl && !alt => b.insert_char(c),
            KeyCode::Enter => b.insert_char('\n'),
            KeyCode::Backspace => b.delete_backward(),
            KeyCode::Delete => b.delete_forward(),
            KeyCode::Tab => {
                if b.selection().is_some() {
                    b.indent(false);
                } else {
                    let pad: String = " ".repeat(b.tab_width);
                    b.insert_str(&pad);
                }
            }
            KeyCode::BackTab => b.indent(true),

            _ => {}
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience for tests / callers wanting a buffer preloaded from disk.
pub fn app_with_file(path: &Path) -> Result<App> {
    let mut app = App::new();
    app.open_path(path.to_path_buf())?;
    Ok(app)
}

/// Prefill for the Save As path field: the current working directory with a
/// trailing separator, so the user only types a filename.
fn default_save_seed() -> String {
    match std::env::current_dir() {
        Ok(dir) => format!("{}/", dir.display()),
        Err(_) => "~/".to_string(),
    }
}
