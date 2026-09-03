//! Top-level application state and the event loop.
//!
//! One `App` owns everything the UI reads. Widgets are pure functions of it.
//! Phase 3: `buffers` is a `Vec` behind a tab strip; a `Config` loaded from
//! `~/.config/vulide/config.toml` drives editor behaviour.

use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::buffer::Buffer;
use crate::complete::Completion;
use crate::config::Config;
use crate::event::{AppEvent, EventSource};
use crate::theme::Theme;
use crate::ui;
use crate::ui::overlay::{Overlay, PathPrompt, PromptKind, PromptOutcome, expand_tilde};
use crate::ui::palette::{Cmd, Entry, Palette, PaletteOutcome};

const TICK: Duration = Duration::from_millis(250);

pub struct App {
    pub buffers: Vec<Buffer>,
    pub active: usize,
    pub config: Config,
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
        let (config, warning) = Config::load();
        let mut app = Self::with_config(config);
        if let Some(w) = warning {
            app.set_status(w);
        }
        app
    }

    /// Build an app around an explicit config, skipping the disk read. Tests use
    /// this so a developer's real `~/.config/vulide/config.toml` can't sway them.
    pub fn with_config(config: Config) -> Self {
        let themes = Theme::builtins();
        let theme_idx = themes
            .iter()
            .position(|t| t.name == config.theme)
            .unwrap_or(0);
        let theme = themes[theme_idx].clone();

        let mut app = Self {
            buffers: vec![Buffer::new()],
            active: 0,
            config,
            theme,
            themes,
            theme_idx,
            status: String::new(),
            editor_rows: 20,
            overlay: Overlay::None,
            completion: None,
            should_quit: false,
        };
        app.apply_config();
        app
    }

    // ---- buffer access ----

    pub fn buf(&self) -> &Buffer {
        &self.buffers[self.active]
    }

    pub fn buf_mut(&mut self) -> &mut Buffer {
        &mut self.buffers[self.active]
    }

    /// Push the config's editor settings onto every open buffer.
    fn apply_config(&mut self) {
        let c = &self.config;
        for b in &mut self.buffers {
            b.tab_width = c.tab_width;
            b.auto_close_brackets = c.auto_close_brackets;
            b.auto_indent = c.auto_indent;
        }
    }

    fn save_config(&mut self) {
        // Never write a real config file from a test run.
        if cfg!(test) {
            return;
        }
        if let Err(e) = self.config.save() {
            self.set_status(format!("config not saved: {e}"));
        }
    }

    // ---- theme ----

    /// Advance to the next bundled theme (Ctrl+T).
    pub fn cycle_theme(&mut self) {
        let next = (self.theme_idx + 1) % self.themes.len();
        self.set_theme(next);
        self.set_status(format!("theme: {}", self.theme.name));
    }

    fn set_theme(&mut self, idx: usize) {
        self.theme_idx = idx.min(self.themes.len() - 1);
        self.theme = self.themes[self.theme_idx].clone();
        self.config.theme = self.theme.name.clone();
        self.save_config();
    }

    fn set_theme_by_name(&mut self, name: &str) {
        if let Some(i) = self.themes.iter().position(|t| t.name == name) {
            self.set_theme(i);
            self.set_status(format!("theme: {}", self.theme.name));
        }
    }

    // ---- tabs / files ----

    pub fn open_path(&mut self, path: PathBuf) -> Result<()> {
        self.open_file(path)?;
        Ok(())
    }

    fn open_file(&mut self, path: PathBuf) -> io::Result<()> {
        let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());

        if let Some(i) = self
            .buffers
            .iter()
            .position(|b| b.path().map(|p| p == canonical) == Some(true))
        {
            self.active = i;
            self.set_status(format!("switched to {}", self.buffers[i].title()));
            return Ok(());
        }

        let buf = Buffer::open(&path)?;
        // Replace a pristine lone "untitled" buffer instead of stacking a tab.
        if self.buffers.len() == 1 && self.buf().path().is_none() && !self.buf().is_dirty() {
            self.buffers[0] = buf;
            self.active = 0;
        } else {
            self.buffers.push(buf);
            self.active = self.buffers.len() - 1;
        }
        self.apply_config();
        self.completion = None;
        self.set_status(format!("opened {}", self.buf().title()));

        self.config.push_recent(&canonical);
        self.save_config();
        Ok(())
    }

    fn new_tab(&mut self) {
        self.buffers.push(Buffer::new());
        self.active = self.buffers.len() - 1;
        self.apply_config();
        self.completion = None;
        self.set_status("new buffer");
    }

    fn close_tab(&mut self, discard: bool) {
        if self.buf().is_dirty() && !discard {
            self.set_status("unsaved changes — save (Ctrl+S) or use palette › Close Tab (discard)");
            return;
        }
        self.buffers.remove(self.active);
        if self.buffers.is_empty() {
            self.buffers.push(Buffer::new());
            self.apply_config();
        }
        self.active = self.active.min(self.buffers.len() - 1);
        self.completion = None;
        self.set_status("closed tab");
    }

    fn next_tab(&mut self) {
        self.active = (self.active + 1) % self.buffers.len();
        self.completion = None;
    }

    fn prev_tab(&mut self) {
        self.active = (self.active + self.buffers.len() - 1) % self.buffers.len();
        self.completion = None;
    }

    fn save_active(&mut self) {
        if self.buf().path().is_none() {
            self.overlay = Overlay::Prompt(Box::new(PathPrompt::save(&default_save_seed())));
            return;
        }
        let msg = match self.buf_mut().save() {
            Ok(()) => format!("saved {}", self.buf().title()),
            Err(e) => format!("save failed: {e}"),
        };
        self.set_status(msg);
    }

    // ---- command palette ----

    fn open_palette(&mut self) {
        let mut entries = vec![
            Entry::new("Save", Cmd::Save),
            Entry::new("Save As…", Cmd::SaveAs),
            Entry::new("Open File…", Cmd::OpenFile),
            Entry::new("New Tab", Cmd::NewTab),
            Entry::new("Close Tab", Cmd::CloseTab),
            Entry::new("Close Tab (discard changes)", Cmd::CloseTabDiscard),
            Entry::new("Next Tab", Cmd::NextTab),
            Entry::new("Previous Tab", Cmd::PrevTab),
            Entry::new("Cycle Theme", Cmd::NextTheme),
            Entry::new("Toggle Line Numbers", Cmd::ToggleLineNumbers),
            Entry::new("Toggle Word Wrap", Cmd::ToggleWordWrap),
            Entry::new("Toggle Auto-close Brackets", Cmd::ToggleAutoClose),
            Entry::new("Reload Config", Cmd::ReloadConfig),
            Entry::new("Quit", Cmd::Quit),
        ];
        for t in &self.themes {
            entries.push(Entry::new(
                format!("Theme: {}", t.name),
                Cmd::SetTheme(t.name.clone()),
            ));
        }
        for p in &self.config.recent_files {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string());
            entries.push(Entry::new(
                format!("Open Recent: {name}"),
                Cmd::OpenRecent(p.clone()),
            ));
        }
        self.overlay = Overlay::Palette(Box::new(Palette::new(entries)));
    }

    fn run_command(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Quit => self.should_quit = true,
            Cmd::Save => self.save_active(),
            Cmd::SaveAs => {
                self.overlay = Overlay::Prompt(Box::new(PathPrompt::save(&default_save_seed())));
            }
            Cmd::OpenFile => {
                self.overlay = Overlay::Prompt(Box::new(PathPrompt::open(&default_save_seed())));
            }
            Cmd::NewTab => self.new_tab(),
            Cmd::CloseTab => self.close_tab(false),
            Cmd::CloseTabDiscard => self.close_tab(true),
            Cmd::NextTab => self.next_tab(),
            Cmd::PrevTab => self.prev_tab(),
            Cmd::NextTheme => self.cycle_theme(),
            Cmd::SetTheme(name) => self.set_theme_by_name(&name),
            Cmd::ToggleLineNumbers => {
                self.config.show_line_numbers = !self.config.show_line_numbers;
                self.save_config();
                self.set_status(format!(
                    "line numbers: {}",
                    on_off(self.config.show_line_numbers)
                ));
            }
            Cmd::ToggleWordWrap => {
                self.config.word_wrap = !self.config.word_wrap;
                self.save_config();
                self.set_status(format!("word wrap: {}", on_off(self.config.word_wrap)));
            }
            Cmd::ToggleAutoClose => {
                self.config.auto_close_brackets = !self.config.auto_close_brackets;
                self.apply_config();
                self.save_config();
                self.set_status(format!(
                    "auto-close brackets: {}",
                    on_off(self.config.auto_close_brackets)
                ));
            }
            Cmd::ReloadConfig => {
                let (cfg, warning) = Config::load();
                self.config = cfg;
                self.apply_config();
                if let Some(name) = self.themes.iter().position(|t| t.name == self.config.theme) {
                    self.set_theme(name);
                }
                self.set_status(warning.unwrap_or_else(|| "config reloaded".to_string()));
            }
            Cmd::OpenRecent(path) => {
                if let Err(e) = self.open_file(path) {
                    self.set_status(format!("open failed: {e}"));
                }
            }
        }
    }

    // ---- misc ----

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
                self.buf_mut().insert_str(&text);
                self.clear_status();
            }
            AppEvent::Resize(..) | AppEvent::Tick => {}
        }
    }

    fn clear_status(&mut self) {
        self.status.clear();
    }

    /// Route a key to whatever overlay is open. Returns `true` if the overlay
    /// consumed the key (the editor must not also see it).
    fn handle_overlay_key(&mut self, key: KeyEvent) -> bool {
        match &mut self.overlay {
            Overlay::None => false,
            Overlay::Prompt(prompt) => {
                let outcome = prompt.handle_key(key);
                self.resolve_prompt(outcome);
                true
            }
            Overlay::Palette(palette) => {
                match palette.handle_key(key) {
                    PaletteOutcome::Stay => {}
                    PaletteOutcome::Cancel => self.overlay = Overlay::None,
                    PaletteOutcome::Run(cmd) => {
                        self.overlay = Overlay::None;
                        self.run_command(cmd);
                    }
                }
                true
            }
        }
    }

    fn resolve_prompt(&mut self, outcome: PromptOutcome) {
        let kind = match &self.overlay {
            Overlay::Prompt(p) => p.kind,
            _ => return,
        };
        match outcome {
            PromptOutcome::Stay => {}
            PromptOutcome::Cancel => {
                self.overlay = Overlay::None;
                self.set_status(match kind {
                    PromptKind::Save => "save cancelled",
                    PromptKind::Open => "open cancelled",
                });
            }
            PromptOutcome::Submit(path) if path.is_empty() => {
                if let Overlay::Prompt(prompt) = &mut self.overlay {
                    prompt.error = Some("enter a path".to_string());
                }
            }
            PromptOutcome::Submit(path) => {
                let path = expand_tilde(&path);
                let result: io::Result<String> = match kind {
                    PromptKind::Save => {
                        let r = self.buf_mut().save_as(&path);
                        r.map(|()| format!("saved {}", self.buf().title()))
                    }
                    // `open_file` sets its own status; keep it on success.
                    PromptKind::Open => {
                        self.open_file(PathBuf::from(&path)).map(|()| String::new())
                    }
                };
                match result {
                    Ok(msg) => {
                        self.overlay = Overlay::None;
                        if !msg.is_empty() {
                            self.set_status(msg);
                        }
                    }
                    Err(e) => {
                        if let Overlay::Prompt(prompt) = &mut self.overlay {
                            prompt.error = Some(e.to_string());
                        }
                    }
                }
            }
        }
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
        self.completion = if self.config.show_autocomplete {
            Completion::detect(self.buf())
        } else {
            None
        };
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
                self.buf_mut().insert_str(&tail);
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

        // ---- app-level shortcuts (must not hold a &mut buffer) ----
        if ctrl {
            match key.code {
                // Ctrl+C and Ctrl+Q both quit for now. A dirty-buffer guard and a
                // modal `:` command line (BatScript wants Vim-like) land in Phase 1.5.
                KeyCode::Char('q') | KeyCode::Char('c') => {
                    self.should_quit = true;
                    return;
                }
                KeyCode::Char('p') => {
                    self.open_palette();
                    return;
                }
                KeyCode::Char('o') => {
                    self.overlay =
                        Overlay::Prompt(Box::new(PathPrompt::open(&default_save_seed())));
                    return;
                }
                KeyCode::Char('n') => {
                    self.new_tab();
                    return;
                }
                KeyCode::Char('w') => {
                    self.close_tab(false);
                    return;
                }
                KeyCode::PageDown | KeyCode::Tab => {
                    self.next_tab();
                    return;
                }
                KeyCode::PageUp | KeyCode::BackTab => {
                    self.prev_tab();
                    return;
                }
                KeyCode::Char('t') => {
                    self.cycle_theme();
                    return;
                }
                KeyCode::Char('s') => {
                    self.save_active();
                    return;
                }
                KeyCode::Char('z') if !shift => {
                    let ok = self.buf_mut().undo();
                    self.set_status(if ok { "" } else { "nothing to undo" });
                    return;
                }
                KeyCode::Char('z') if shift => {
                    self.buf_mut().redo();
                    self.clear_status();
                    return;
                }
                KeyCode::Char('y') => {
                    let ok = self.buf_mut().redo();
                    self.set_status(if ok { "" } else { "nothing to redo" });
                    return;
                }
                KeyCode::Char('a') => {
                    self.buf_mut().select_all();
                    return;
                }
                _ => {}
            }
        }

        let b = &mut self.buffers[self.active];
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

fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

/// Prefill for the path field: the current working directory with a trailing
/// separator, so the user only types a filename.
fn default_save_seed() -> String {
    match std::env::current_dir() {
        Ok(dir) => format!("{}/", dir.display()),
        Err(_) => "~/".to_string(),
    }
}
