//! Top-level application state and the event loop.
//!
//! One `App` owns everything the UI reads. Widgets are pure functions of it.
//! Phase 3: `buffers` is a `Vec` behind a tab strip; a `Config` loaded from
//! `~/.config/vulide/config.toml` drives editor behaviour.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::Duration;

use anyhow::Result;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;

use crate::buffer::Buffer;
use crate::complete::Completion;
use crate::config::Config;
use crate::event::{AppEvent, EventSource};
use crate::run::{self, RunConsole};
use crate::theme::Theme;
use crate::ui;
use crate::ui::help::HelpOutcome;
use crate::ui::overlay::{Overlay, PathPrompt, PromptKind, PromptOutcome, expand_tilde};
use crate::ui::palette::{Cmd, Entry, Palette, PaletteOutcome};
use crate::ui::tabs::TabHit;
use crate::ui::theme_picker::{ThemePicker, ThemePickerOutcome};

/// Input poll granularity for the reader thread (not an output-latency bound).
const TICK: Duration = Duration::from_millis(100);

/// Which pane keystrokes go to when no overlay is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Editor,
    Output,
}

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
    /// The run-output console, present once the file has been run at least once
    /// this session (until explicitly closed).
    pub run: Option<RunConsole>,
    pub focus: Focus,
    /// User-dragged output-panel height (rows); `None` = the default third.
    pub panel_height: Option<u16>,
    pub dragging_splitter: bool,

    // ---- mouse hit rects, refreshed every draw ----
    /// Screen rect of the status-bar ▶/■ button.
    pub run_button: Option<Rect>,
    pub editor_rect: Rect,
    pub status_rect: Rect,
    pub splitter_rect: Option<Rect>,
    /// Screen rect of the output panel (when shown).
    pub panel_rect: Option<Rect>,
    /// Screen rect of the panel's close-✕ button.
    pub panel_close_rect: Option<Rect>,
    /// Per-tab hit rects (index, tab, close-✕).
    pub tab_hits: Vec<TabHit>,
    /// Screen rect of the open overlay's box (for click-away dismiss).
    pub overlay_rect: Option<Rect>,

    // ---- hover state (mouse-move driven) ----
    pub hovered_tab: Option<usize>,
    pub hover_splitter: bool,
    pub hover_panel_close: bool,
    /// Channel the run console's reader threads push onto; set while `run()` owns
    /// the loop. `None` outside it (e.g. in tests, unless injected).
    run_tx: Option<Sender<AppEvent>>,
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
            run: None,
            focus: Focus::Editor,
            panel_height: None,
            dragging_splitter: false,
            run_button: None,
            editor_rect: Rect::default(),
            status_rect: Rect::default(),
            splitter_rect: None,
            panel_rect: None,
            panel_close_rect: None,
            tab_hits: Vec::new(),
            overlay_rect: None,
            hovered_tab: None,
            hover_splitter: false,
            hover_panel_close: false,
            run_tx: None,
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

    /// Open the theme picker (Ctrl+T), selection starting on the active theme.
    pub fn open_theme_picker(&mut self) {
        let names = self.themes.iter().map(|t| t.name.clone()).collect();
        self.overlay = Overlay::ThemePicker(Box::new(ThemePicker::new(names, self.theme_idx)));
    }

    /// Swap the active theme without touching the config (live preview).
    fn preview_theme(&mut self, idx: usize) {
        self.theme_idx = idx.min(self.themes.len() - 1);
        self.theme = self.themes[self.theme_idx].clone();
    }

    /// Preview + persist to config.
    fn set_theme(&mut self, idx: usize) {
        self.preview_theme(idx);
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
        self.close_tab_at(self.active, discard);
    }

    fn close_tab_at(&mut self, index: usize, discard: bool) {
        if index >= self.buffers.len() {
            return;
        }
        if self.buffers[index].is_dirty() && !discard {
            self.set_status("unsaved changes — save (Ctrl+S) or use palette › Close Tab (discard)");
            return;
        }
        self.buffers.remove(index);
        if self.buffers.is_empty() {
            self.buffers.push(Buffer::new());
            self.apply_config();
        }
        // Keep `active` pointing at the same buffer (or the nearest one).
        if self.active > index || self.active >= self.buffers.len() {
            self.active = self.active.saturating_sub(1).min(self.buffers.len() - 1);
        }
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
            Entry::new("Choose Theme…", Cmd::ChooseTheme),
            Entry::new("Toggle Line Numbers", Cmd::ToggleLineNumbers),
            Entry::new("Toggle Word Wrap", Cmd::ToggleWordWrap),
            Entry::new("Toggle Auto-close Brackets", Cmd::ToggleAutoClose),
            Entry::new(
                "Toggle Mouse (for terminal text selection)",
                Cmd::ToggleMouse,
            ),
            Entry::new("Run File (F5)", Cmd::RunFile),
            Entry::new("Stop Run", Cmd::StopRun),
            Entry::new("Close Output Panel", Cmd::CloseOutput),
            Entry::new("Help — Keys & Shortcuts (F1)", Cmd::Help),
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
            Cmd::ChooseTheme => self.open_theme_picker(),
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
            Cmd::ToggleMouse => {
                self.config.mouse = !self.config.mouse;
                self.save_config();
                self.apply_mouse_capture();
                self.set_status(format!(
                    "mouse: {} (Shift bypasses for selection)",
                    on_off(self.config.mouse)
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
            Cmd::RunFile => self.start_run(),
            Cmd::StopRun => self.stop_run(),
            Cmd::CloseOutput => self.close_output(),
            Cmd::Help => self.open_help(),
        }
    }

    // ---- run console ----

    /// F5: run the active buffer through the Vulpin interpreter.
    pub fn start_run(&mut self) {
        let Some(tx) = self.run_tx.clone() else {
            self.set_status("run needs the interactive event loop");
            return;
        };
        let Some(interp) = run::resolve_interpreter(&self.config.vulpin_path) else {
            self.set_status("vulpin interpreter not found — set vulpin_path in config");
            return;
        };
        let (file, workdir, temp) = match self.run_target() {
            Ok(t) => t,
            Err(e) => {
                self.set_status(format!("run: {e}"));
                return;
            }
        };
        let mut argv = interp;
        argv.push(file.to_string_lossy().into_owned());

        // Replacing `self.run` drops the previous console, which kills any child
        // still running and clears its temp file.
        match RunConsole::start(argv, &workdir, temp, &tx) {
            Ok(console) => {
                self.run = Some(console);
                self.focus = Focus::Output;
                self.set_status(format!("running {}", file.display()));
            }
            Err(e) => self.set_status(format!("run failed: {e}")),
        }
    }

    /// The file to hand the interpreter: the buffer's own path when saved and
    /// clean, otherwise a temp `.vul` written from the current contents.
    fn run_target(&self) -> io::Result<(PathBuf, PathBuf, Option<PathBuf>)> {
        let b = self.buf();
        if let Some(path) = b.path()
            && !b.is_dirty()
        {
            let workdir = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."));
            return Ok((path.to_path_buf(), workdir, None));
        }

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut tmp = std::env::temp_dir();
        tmp.push(format!("vulide-run-{}-{nanos}.vul", std::process::id()));
        let mut text = b.rope().to_string();
        if !text.ends_with('\n') {
            text.push('\n');
        }
        std::fs::write(&tmp, text)?;
        let workdir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Ok((tmp.clone(), workdir, Some(tmp)))
    }

    pub fn stop_run(&mut self) {
        match &mut self.run {
            Some(r) if r.is_running() => {
                r.stop();
                self.set_status("run stopped");
            }
            Some(_) => self.set_status("run already finished"),
            None => self.set_status("nothing running"),
        }
    }

    fn close_output(&mut self) {
        if self.run.is_some() {
            self.run = None; // Drop kills any child + removes the temp file
            self.focus = Focus::Editor;
            self.set_status("output panel closed");
        }
    }

    fn toggle_output_focus(&mut self) {
        if self.run.is_none() {
            self.start_run();
            return;
        }
        self.focus = match self.focus {
            Focus::Editor => Focus::Output,
            Focus::Output => Focus::Editor,
        };
    }

    /// Label for the status-bar button; `is_running` also decides its colour.
    pub fn run_button_label(&self) -> &'static str {
        if self.run.as_ref().is_some_and(RunConsole::is_running) {
            " ■ Stop "
        } else {
            " ▶ Run "
        }
    }

    /// The ▶/■ button: run when idle, stop when a run is in progress.
    fn toggle_run(&mut self) {
        if self.run.as_ref().is_some_and(RunConsole::is_running) {
            self.stop_run();
        } else {
            self.start_run();
        }
    }

    fn open_help(&mut self) {
        self.overlay = Overlay::Help(Box::default());
    }

    /// Push `config.mouse` to the terminal (live toggle). No-op under tests.
    fn apply_mouse_capture(&self) {
        if cfg!(test) {
            return;
        }
        use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
        let _ = if self.config.mouse {
            ratatui::crossterm::execute!(std::io::stdout(), EnableMouseCapture)
        } else {
            ratatui::crossterm::execute!(std::io::stdout(), DisableMouseCapture)
        };
    }

    /// Returns whether the screen needs a redraw (so idle mouse motion is free).
    fn handle_mouse(&mut self, ev: MouseEvent) -> bool {
        let (col, row) = (ev.column, ev.row);
        match ev.kind {
            MouseEventKind::Moved => return self.update_hover(col, row),
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.dragging_splitter {
                    self.resize_panel_to(row);
                    return true;
                }
                return false;
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let was = self.dragging_splitter;
                self.dragging_splitter = false;
                return was;
            }
            MouseEventKind::ScrollUp if self.focus == Focus::Output => {
                if let Some(r) = &mut self.run {
                    r.scroll_up(3);
                }
                return true;
            }
            MouseEventKind::ScrollDown if self.focus == Focus::Output => {
                if let Some(r) = &mut self.run {
                    r.scroll_down(3);
                }
                return true;
            }
            MouseEventKind::Down(MouseButton::Left) => {}
            _ => return false,
        }

        // ---- left click ----

        // A click outside an open overlay dismisses it (like Esc).
        if self.overlay.is_open() {
            if !self.overlay_rect.is_some_and(|r| hit(r, col, row)) {
                self.dismiss_overlay();
            }
            return true;
        }

        // Grab the splitter.
        if self.splitter_rect.is_some_and(|r| hit(r, col, row)) {
            self.dragging_splitter = true;
            return true;
        }

        // The panel's ✕ (checked before the panel body so the corner works).
        if self.panel_close_rect.is_some_and(|r| hit(r, col, row)) {
            self.close_output();
            return true;
        }

        // The status-bar ▶/■ button.
        if self.run_button.is_some_and(|r| hit(r, col, row)) {
            self.toggle_run();
            return true;
        }

        // Tabs: a click on a tab switches to it; on its ✕ closes it.
        let tab_action = self.tab_hits.iter().find_map(|t| {
            if hit(t.close, col, row) {
                Some((t.index, true))
            } else if hit(t.rect, col, row) {
                Some((t.index, false))
            } else {
                None
            }
        });
        if let Some((index, close)) = tab_action {
            if close {
                self.close_tab_at(index, false);
            } else {
                self.active = index;
                self.completion = None;
            }
            self.focus = Focus::Editor;
            return true;
        }

        // Otherwise a click just moves focus between the two panes, so the
        // keyboard always goes where you're looking.
        if self.panel_rect.is_some_and(|r| hit(r, col, row)) && self.run.is_some() {
            self.focus = Focus::Output;
        } else if self.editor_rect.height > 0 && hit(self.editor_rect, col, row) {
            self.focus = Focus::Editor;
        }
        true
    }

    /// Recompute hover flags; returns whether any of them changed.
    fn update_hover(&mut self, col: u16, row: u16) -> bool {
        let tab = self
            .tab_hits
            .iter()
            .find(|t| hit(t.rect, col, row))
            .map(|t| t.index);
        let splitter = self.splitter_rect.is_some_and(|r| hit(r, col, row));
        let close = self.panel_close_rect.is_some_and(|r| hit(r, col, row));
        let changed = tab != self.hovered_tab
            || splitter != self.hover_splitter
            || close != self.hover_panel_close;
        self.hovered_tab = tab;
        self.hover_splitter = splitter;
        self.hover_panel_close = close;
        changed
    }

    /// Drag the editor/panel divider: the splitter row follows the cursor,
    /// keeping the editor at least [`ui::MIN_EDITOR_ROWS`] tall.
    fn resize_panel_to(&mut self, row: u16) {
        let status_y = self.status_rect.y;
        let editor_top = self.editor_rect.y;
        if status_y == 0 {
            return;
        }
        // panel occupies (row+1 ..= status_y-1)  →  height = status_y - row - 1
        let want = status_y.saturating_sub(row).saturating_sub(1);
        let max = status_y.saturating_sub(editor_top + crate::ui::MIN_EDITOR_ROWS + 1);
        self.panel_height = Some(want.clamp(3, max.max(3)));
    }

    /// Close the overlay the way its own Esc would (reverting a theme preview).
    fn dismiss_overlay(&mut self) {
        if let Overlay::ThemePicker(p) = &self.overlay {
            let original = p.original;
            self.preview_theme(original);
        }
        self.overlay = Overlay::None;
    }

    #[cfg(test)]
    pub fn inject_run_tx(&mut self, tx: Sender<AppEvent>) {
        self.run_tx = Some(tx);
    }

    /// Test hook: start a run from an explicit argv, skipping interpreter
    /// resolution and temp-file handling (workdir = cwd).
    #[cfg(test)]
    pub fn start_run_argv(&mut self, argv: Vec<String>) {
        let tx = self.run_tx.clone().expect("inject_run_tx first");
        let workdir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        match RunConsole::start(argv, &workdir, None, &tx) {
            Ok(console) => {
                self.run = Some(console);
                self.focus = Focus::Output;
            }
            Err(e) => self.set_status(format!("run failed: {e}")),
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
        self.run_tx = Some(events.sender());
        terminal.draw(|f| ui::draw(f, self))?;

        while !self.should_quit {
            let Some(first) = events.next()? else {
                break; // every sender dropped — shouldn't happen, but exit cleanly
            };
            let mut dirty = self.handle_event(first);
            // Coalesce a burst (e.g. a flood of output lines, or mouse motion)
            // into a single redraw — and skip it entirely if nothing changed.
            while let Some(ev) = events.try_next() {
                if self.should_quit {
                    break;
                }
                dirty |= self.handle_event(ev);
            }
            if dirty {
                terminal.draw(|f| ui::draw(f, self))?;
            }
        }
        self.run_tx = None;
        Ok(())
    }

    /// Dispatch one event; returns whether the screen needs to be redrawn.
    pub fn handle_event(&mut self, ev: AppEvent) -> bool {
        match ev {
            AppEvent::Key(key) => self.handle_key(key),
            AppEvent::Mouse(m) => return self.handle_mouse(m),
            AppEvent::Paste(text) if self.focus == Focus::Output => {
                if let Some(r) = &mut self.run {
                    r.input.insert_str(&text);
                }
            }
            AppEvent::Paste(text) => {
                self.buf_mut().insert_str(&text);
                self.clear_status();
            }
            AppEvent::Output { stream, line } => {
                if let Some(r) = &mut self.run {
                    r.on_output(stream, line);
                }
            }
            AppEvent::StreamClosed(_) => {
                let done = if let Some(r) = &mut self.run {
                    if r.on_stream_closed() {
                        r.reap();
                        r.close_stdin();
                        Some((r.stopped, r.exit_code))
                    } else {
                        None
                    }
                } else {
                    None
                };
                if let Some((stopped, code)) = done {
                    self.set_status(match (stopped, code) {
                        (true, _) => "run stopped".to_string(),
                        (_, Some(0)) => "run finished (exit 0)".to_string(),
                        (_, Some(c)) => format!("run finished (exit {c})"),
                        (_, None) => "run finished".to_string(),
                    });
                    // The program is done — hand the keyboard back to the editor
                    // unless the user is scrolled up reading the output.
                    if self.focus == Focus::Output
                        && self.run.as_ref().is_some_and(|r| r.scroll == 0)
                    {
                        self.focus = Focus::Editor;
                    }
                }
            }
            AppEvent::InputClosed => {
                self.set_status("terminal input closed");
                self.should_quit = true;
            }
            AppEvent::Resize(..) => {}
            AppEvent::Tick => return false,
        }
        true
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
            Overlay::Help(h) => {
                if let HelpOutcome::Close = h.handle_key(key) {
                    self.overlay = Overlay::None;
                }
                true
            }
            Overlay::ThemePicker(picker) => {
                match picker.handle_key(key) {
                    ThemePickerOutcome::Preview(i) => self.preview_theme(i),
                    ThemePickerOutcome::Commit(i) => {
                        self.set_theme(i);
                        self.overlay = Overlay::None;
                        self.set_status(format!("theme: {}", self.theme.name));
                    }
                    ThemePickerOutcome::Cancel => {
                        let original = match &self.overlay {
                            Overlay::ThemePicker(p) => p.original,
                            _ => self.theme_idx,
                        };
                        self.preview_theme(original);
                        self.overlay = Overlay::None;
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
        // These work from either pane.
        match key.code {
            KeyCode::F(5) => return self.start_run(),
            KeyCode::F(6) => return self.toggle_output_focus(),
            KeyCode::F(1) => return self.open_help(),
            _ => {}
        }
        if self.focus == Focus::Output {
            self.handle_output_key(key);
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

    /// Keystrokes while the output panel has focus: scrollback nav, a stdin line,
    /// Esc back to the editor. `Ctrl+C` stops a running child (else quits).
    fn handle_output_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        if ctrl {
            match key.code {
                KeyCode::Char('q') => {
                    self.should_quit = true;
                    return;
                }
                KeyCode::Char('c') => {
                    if self.run.as_ref().is_some_and(RunConsole::is_running) {
                        self.stop_run();
                    } else {
                        self.should_quit = true;
                    }
                    return;
                }
                KeyCode::Char('p') => return self.open_palette(),
                KeyCode::Char('d') => {
                    if let Some(r) = &mut self.run {
                        r.close_stdin();
                        self.set_status("stdin closed (EOF)");
                    }
                    return;
                }
                _ => {}
            }
        }
        let Some(r) = &mut self.run else {
            self.focus = Focus::Editor;
            return;
        };
        let running = r.is_running();
        match key.code {
            KeyCode::Esc => self.focus = Focus::Editor,
            KeyCode::Up => r.scroll_up(1),
            KeyCode::Down => r.scroll_down(1),
            KeyCode::PageUp => r.scroll_up(10),
            KeyCode::PageDown => r.scroll_down(10),
            KeyCode::Home => r.scroll_up(usize::MAX),
            KeyCode::End => r.scroll_to_bottom(),
            KeyCode::Enter if running => {
                let line = r.input.rope().to_string();
                r.input = Buffer::new();
                r.send_stdin(&line);
                r.scroll_to_bottom();
            }
            KeyCode::Backspace if running => r.input.delete_backward(),
            KeyCode::Char(c) if running && !ctrl && !alt => r.input.insert_char(c),
            _ => {}
        }
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
                // Many terminals send Ctrl+H as Backspace; where it arrives as a
                // real Ctrl+H it opens the help card (F1 is the reliable key).
                KeyCode::Char('h') => {
                    self.open_help();
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
                    self.open_theme_picker();
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

/// Is the cell `(col, row)` inside `r`?
fn hit(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

/// Prefill for the path field: the current working directory with a trailing
/// separator, so the user only types a filename.
fn default_save_seed() -> String {
    match std::env::current_dir() {
        Ok(dir) => format!("{}/", dir.display()),
        Err(_) => "~/".to_string(),
    }
}
