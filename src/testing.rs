//! `Harness` — drive the real `App` headlessly against a `ratatui` `TestBackend`.
//!
//! Tests feed parsed events through `App::handle_event` (the same path the live
//! loop uses) and read back the rendered screen. Keystroke tests touch no real
//! terminal and never sleep. The run-console tests do spawn real short-lived
//! child processes (`printf`, `cat`, …) and `pump` their output off the channel.

#![cfg(test)]

use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::App;
use crate::buffer::Buffer;
use crate::config::Config;
use crate::event::AppEvent;

pub struct Harness {
    pub app: App,
    terminal: Terminal<TestBackend>,
    run_rx: Receiver<AppEvent>,
}

impl Harness {
    pub fn new(width: u16, height: u16) -> Self {
        let terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let (tx, run_rx) = mpsc::channel();
        let mut app = App::with_config(Config::default());
        app.inject_run_tx(tx);
        let mut h = Self {
            app,
            terminal,
            run_rx,
        };
        h.draw();
        h
    }

    /// Drain any background (run-console) events, then redraw.
    pub fn pump(&mut self) -> &mut Self {
        while let Ok(ev) = self.run_rx.try_recv() {
            self.app.handle_event(ev);
        }
        self.draw()
    }

    /// Pump until `pred` holds or `timeout` elapses. Returns whether it held.
    pub fn pump_until(&mut self, timeout: Duration, mut pred: impl FnMut(&App) -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            self.pump();
            if pred(&self.app) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub fn with_text(text: &str, width: u16, height: u16) -> Self {
        let mut h = Self::new(width, height);
        h.app.buffers[0] = Buffer::from_str(text);
        h.draw();
        h
    }

    pub fn draw(&mut self) -> &mut Self {
        let app = &mut self.app;
        self.terminal.draw(|f| crate::ui::draw(f, app)).unwrap();
        self
    }

    pub fn key(&mut self, code: KeyCode) -> &mut Self {
        self.key_mods(code, KeyModifiers::NONE)
    }

    pub fn key_mods(&mut self, code: KeyCode, mods: KeyModifiers) -> &mut Self {
        self.app
            .handle_event(AppEvent::Key(KeyEvent::new(code, mods)));
        self.draw()
    }

    pub fn ctrl(&mut self, ch: char) -> &mut Self {
        self.key_mods(KeyCode::Char(ch), KeyModifiers::CONTROL)
    }

    fn mouse(
        &mut self,
        kind: ratatui::crossterm::event::MouseEventKind,
        col: u16,
        row: u16,
    ) -> &mut Self {
        use ratatui::crossterm::event::MouseEvent;
        self.app.handle_event(AppEvent::Mouse(MouseEvent {
            kind,
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }));
        self.draw()
    }

    /// A left-button press at `(col, row)`.
    pub fn click(&mut self, col: u16, row: u16) -> &mut Self {
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};
        self.mouse(MouseEventKind::Down(MouseButton::Left), col, row)
    }

    pub fn mouse_move(&mut self, col: u16, row: u16) -> &mut Self {
        self.mouse(ratatui::crossterm::event::MouseEventKind::Moved, col, row)
    }

    /// A full press → drag → release from `(x0,y0)` to `(x1,y1)`.
    pub fn drag(&mut self, x0: u16, y0: u16, x1: u16, y1: u16) -> &mut Self {
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};
        self.mouse(MouseEventKind::Down(MouseButton::Left), x0, y0);
        self.mouse(MouseEventKind::Drag(MouseButton::Left), x1, y1);
        self.mouse(MouseEventKind::Up(MouseButton::Left), x1, y1)
    }

    pub fn type_str(&mut self, s: &str) -> &mut Self {
        for ch in s.chars() {
            let code = if ch == '\n' {
                KeyCode::Enter
            } else {
                KeyCode::Char(ch)
            };
            self.app
                .handle_event(AppEvent::Key(KeyEvent::new(code, KeyModifiers::NONE)));
        }
        self.draw()
    }

    pub fn screen(&self) -> String {
        let buf = self.terminal.backend().buffer();
        let area = *buf.area();
        let mut out = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                if let Some(cell) = buf.cell((x, y)) {
                    out.push_str(cell.symbol());
                }
            }
            out.push('\n');
        }
        out
    }

    pub fn line(&self, row: u16) -> String {
        self.screen()
            .lines()
            .nth(row as usize)
            .unwrap_or_default()
            .trim_end()
            .to_string()
    }

    pub fn contains(&self, needle: &str) -> bool {
        self.screen().contains(needle)
    }

    /// The rendered cell at `(x, y)` — for asserting on colour/style.
    pub fn cell(&self, x: u16, y: u16) -> ratatui::buffer::Cell {
        self.terminal
            .backend()
            .buffer()
            .cell((x, y))
            .cloned()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::buffer::Position;

    #[test]
    fn status_bar_renders() {
        let h = Harness::new(40, 6);
        assert!(h.contains("untitled"));
        assert!(h.contains("Ln 1, Col 1"));
    }

    #[test]
    fn typing_shows_on_screen() {
        let mut h = Harness::new(40, 6);
        h.type_str("G\"hello\"");
        assert!(h.contains("G\"hello\""));
        assert!(h.contains("Ln 1, Col 9"));
    }

    #[test]
    fn ctrl_t_theme_picker_previews_commits_and_reverts() {
        use ratatui::style::Color;

        let mut h = Harness::with_text("G\"hi\"", 44, 12);
        let g_mocha = h.cell(4, 0).fg; // the 'G', coloured by theme.command
        assert_eq!(h.app.theme.name, "Dark (Catppuccin Mocha)");

        h.ctrl('t');
        assert!(h.contains("Theme"));
        assert_eq!(h.app.theme.name, "Dark (Catppuccin Mocha)"); // not changed yet

        // Down previews the next theme live on the editor behind the overlay.
        h.key(KeyCode::Down);
        assert_eq!(h.app.theme.name, "Light (Catppuccin Latte)");
        assert_ne!(h.cell(4, 0).fg, g_mocha, "'G' recoloured on preview");
        assert_ne!(h.cell(4, 0).fg, Color::Reset);

        // Esc reverts to the theme that was active when the picker opened.
        h.key(KeyCode::Esc);
        assert!(!h.app.overlay.is_open());
        assert_eq!(h.app.theme.name, "Dark (Catppuccin Mocha)");
        assert_eq!(h.cell(4, 0).fg, g_mocha);

        // This time keep the previewed theme.
        h.ctrl('t');
        h.key(KeyCode::Down);
        h.key(KeyCode::Enter);
        assert!(!h.app.overlay.is_open());
        assert_eq!(h.app.theme.name, "Light (Catppuccin Latte)");
        assert_eq!(h.app.config.theme, "Light (Catppuccin Latte)");
        assert!(h.contains("theme: Light"));
    }

    #[test]
    fn default_theme_is_dark() {
        let h = Harness::new(40, 6);
        assert_eq!(h.app.theme.name, "Dark (Catppuccin Mocha)");
        assert_eq!(
            crate::theme::Theme::default().name,
            "Dark (Catppuccin Mocha)"
        );
    }

    #[test]
    fn autocomplete_popup_appears_and_accepts() {
        let mut h = Harness::with_text("counter = 0\n", 40, 10);
        h.app
            .buf_mut()
            .set_cursor(Position { line: 1, col: 0 }, false);
        h.draw();

        h.type_str("G $c");
        assert!(h.app.completion.is_some(), "popup should be open on `$c`");
        assert!(h.contains("counter"), "candidate shown:\n{}", h.screen());

        h.key(KeyCode::Tab);
        assert!(h.app.completion.is_none(), "popup closes on accept");
        assert_eq!(h.app.buf().line_text(1), "G $counter");
    }

    #[test]
    fn autocomplete_dismisses_on_esc_without_editing() {
        let mut h = Harness::with_text("value = 1\n", 40, 10);
        h.app
            .buf_mut()
            .set_cursor(Position { line: 1, col: 0 }, false);
        h.draw();

        h.type_str("$va");
        assert!(h.app.completion.is_some());
        h.key(KeyCode::Esc);
        assert!(h.app.completion.is_none());
        assert_eq!(h.app.buf().line_text(1), "$va");
        assert!(
            !h.contains("value = 1\nvalue"),
            "no completion was inserted"
        );
    }

    #[test]
    fn enter_and_autoindent_render() {
        let mut h = Harness::new(40, 8);
        h.type_str("? $x > 1\n");
        assert_eq!(h.app.buf().cursor(), Position { line: 1, col: 4 });
        h.type_str("G\"big\"");
        assert!(h.line(1).ends_with("    G\"big\""));
    }

    #[test]
    fn backspace_and_undo_via_keys() {
        let mut h = Harness::new(40, 6);
        h.type_str("abcd");
        h.key(KeyCode::Backspace);
        assert_eq!(h.app.buf().rope().to_string(), "abc");
        h.ctrl('z'); // undo the backspace
        assert_eq!(h.app.buf().rope().to_string(), "abcd");
        h.ctrl('z'); // undo the typing group
        assert_eq!(h.app.buf().rope().to_string(), "");
        h.ctrl('y');
        assert_eq!(h.app.buf().rope().to_string(), "abcd");
    }

    #[test]
    fn shift_arrow_builds_selection() {
        let mut h = Harness::with_text("hello world", 40, 6);
        h.key(KeyCode::End);
        for _ in 0..5 {
            h.key_mods(KeyCode::Left, KeyModifiers::SHIFT);
        }
        assert_eq!(
            h.app.buf().selection(),
            Some((Position { line: 0, col: 6 }, Position { line: 0, col: 11 }))
        );
        h.key(KeyCode::Backspace);
        assert_eq!(h.app.buf().rope().to_string(), "hello ");
    }

    #[test]
    fn cursor_stays_visible_when_scrolling_down() {
        let mut h = Harness::new(40, 6); // ~4 text rows + status
        for i in 0..20 {
            h.type_str(&format!("line{i}\n"));
        }
        h.type_str("LAST");
        assert!(
            h.contains("LAST"),
            "cursor line must be on screen:\n{}",
            h.screen()
        );
        assert!(!h.contains("line0"), "top should have scrolled away");
    }

    #[test]
    fn horizontal_scroll_follows_cursor() {
        let mut h = Harness::new(24, 5);
        h.type_str(&"x".repeat(60));
        assert_eq!(h.app.buf().cursor().col, 60);
        // the line is far wider than the viewport, so it must be clipped
        assert!(h.screen().matches('x').count() < 60);
        // and the cursor end of the line stays visible
        assert!(h.line(0).ends_with('x'));
    }

    #[test]
    fn ctrl_s_on_untitled_opens_save_as() {
        let mut h = Harness::new(70, 12);
        h.type_str("G\"hi\"");
        h.ctrl('s');
        assert!(h.contains("Save As"));
        assert!(matches!(
            h.app.overlay,
            crate::ui::overlay::Overlay::Prompt(_)
        ));
        // editor keystrokes are captured by the overlay now
        h.type_str("abc");
        assert_eq!(h.app.buf().rope().to_string(), "G\"hi\"");
        h.key(KeyCode::Esc);
        assert!(!h.app.overlay.is_open());
        assert!(h.contains("save cancelled"));
    }

    #[test]
    fn save_as_writes_file_and_closes() {
        let path = std::env::temp_dir().join(format!("vulide_saveas_{}.vp", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let mut h = Harness::new(80, 12);
        h.type_str("G\"saved\"");
        h.ctrl('s');
        // clear the prefilled path, type our own
        for _ in 0..200 {
            h.key(KeyCode::Backspace);
        }
        h.type_str(path.to_str().unwrap());
        h.key(KeyCode::Enter);

        assert!(!h.app.overlay.is_open());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "G\"saved\"\n");
        assert!(!h.app.buf().is_dirty());
        assert_eq!(h.app.buf().path(), Some(path.as_path()));
        std::fs::remove_file(&path).ok();

        // a second Ctrl+S now writes straight through, no overlay
        h.type_str("!");
        h.ctrl('s');
        assert!(!h.app.overlay.is_open());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "G\"saved\"!\n");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn tabs_open_switch_and_close() {
        let mut h = Harness::new(60, 12);
        h.type_str("first");
        h.ctrl('n'); // new tab
        assert_eq!(h.app.buffers.len(), 2);
        assert_eq!(h.app.active, 1);
        h.type_str("second");
        assert!(h.contains("[2/2]")); // status bar shows the tab count

        h.key_mods(KeyCode::PageUp, KeyModifiers::CONTROL); // prev tab
        assert_eq!(h.app.active, 0);
        assert_eq!(h.app.buf().rope().to_string(), "first");

        h.key_mods(KeyCode::PageDown, KeyModifiers::CONTROL); // next tab
        assert_eq!(h.app.active, 1);

        // dirty tab won't close without discard
        h.ctrl('w');
        assert_eq!(h.app.buffers.len(), 2);
        assert!(h.contains("unsaved changes"));
    }

    #[test]
    fn command_palette_filters_and_runs() {
        let mut h = Harness::new(80, 16);
        assert_eq!(h.app.buffers.len(), 1);

        h.ctrl('p');
        assert!(h.contains("Commands"));
        h.type_str("new tab"); // fuzzy filter
        h.key(KeyCode::Enter);

        assert!(!h.app.overlay.is_open());
        assert_eq!(h.app.buffers.len(), 2, "palette ran 'New Tab'");
    }

    #[test]
    fn palette_sets_theme_directly() {
        let mut h = Harness::new(80, 16);
        assert_eq!(h.app.theme.name, "Dark (Catppuccin Mocha)");
        h.ctrl('p');
        h.type_str("theme nord"); // matches the "Theme: Nord" entry
        h.key(KeyCode::Enter);
        assert_eq!(h.app.theme.name, "Nord");
        assert_eq!(h.app.config.theme, "Nord");
    }

    #[test]
    fn ctrl_o_opens_a_file_in_a_new_tab() {
        let path = std::env::temp_dir().join(format!("vulide_open_{}.vul", std::process::id()));
        std::fs::write(&path, "G\"from disk\"\n").unwrap();

        let mut h = Harness::new(80, 14);
        h.type_str("scratch");
        h.ctrl('o');
        for _ in 0..300 {
            h.key(KeyCode::Backspace);
        }
        h.type_str(path.to_str().unwrap());
        h.key(KeyCode::Enter);

        assert!(!h.app.overlay.is_open());
        assert_eq!(h.app.buffers.len(), 2);
        assert_eq!(h.app.buf().rope().to_string(), "G\"from disk\"");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn toggle_line_numbers_via_palette() {
        let mut h = Harness::with_text("G\"x\"", 40, 8);
        assert!(
            h.line(0).starts_with("  1 "),
            "gutter present: {:?}",
            h.line(0)
        );
        h.ctrl('p');
        h.type_str("line numbers");
        h.key(KeyCode::Enter);
        assert!(!h.app.config.show_line_numbers);
        assert_eq!(h.line(0), "G\"x\"", "gutter gone:\n{}", h.screen());
    }

    // ---- Phase 4: run + output console ----

    fn wait_for_exit(h: &mut Harness) {
        let done = h.pump_until(Duration::from_secs(5), |a| {
            a.run.as_ref().is_some_and(|r| !r.is_running())
        });
        assert!(done, "process did not finish:\n{}", h.screen());
    }

    #[test]
    fn run_streams_stdout_into_the_panel() {
        let mut h = Harness::new(60, 20);
        h.app
            .start_run_argv(vec!["printf".into(), "line one\nline two\n".into()]);
        h.draw();
        assert!(h.contains("printf"), "panel title missing:\n{}", h.screen()); // panel title shows the command
        wait_for_exit(&mut h);

        assert!(h.contains("line one"), "stdout not shown:\n{}", h.screen());
        assert!(h.contains("line two"));
        assert_eq!(h.app.run.as_ref().unwrap().exit_code, Some(0));
        assert!(h.contains("exit 0"));
    }

    #[test]
    fn run_shows_stderr_and_nonzero_exit() {
        let mut h = Harness::new(60, 20);
        h.app.start_run_argv(vec![
            "sh".into(),
            "-c".into(),
            "echo good; echo bad 1>&2; exit 3".into(),
        ]);
        wait_for_exit(&mut h);

        let console = h.app.run.as_ref().unwrap();
        assert_eq!(console.exit_code, Some(3));
        assert!(console.rows.iter().any(|r| r.text == "good"));
        assert!(
            console
                .rows
                .iter()
                .any(|r| { r.text == "bad" && r.stream == crate::event::OutputStream::Stderr })
        );
        assert!(h.contains("exit 3"));
    }

    #[test]
    fn stdin_round_trips_through_cat() {
        let mut h = Harness::new(60, 20);
        h.app.start_run_argv(vec!["cat".into()]);
        assert_eq!(h.app.focus, crate::app::Focus::Output);

        h.type_str("ping"); // goes to the panel's stdin line
        h.key(KeyCode::Enter);

        let echoed = h.pump_until(Duration::from_secs(5), |a| {
            a.run
                .as_ref()
                .unwrap()
                .rows
                .iter()
                .any(|r| r.text == "ping" && r.stream == crate::event::OutputStream::Stdout)
        });
        assert!(echoed, "cat did not echo stdin:\n{}", h.screen());

        h.ctrl('d'); // close stdin → cat exits
        wait_for_exit(&mut h);
        assert_eq!(h.app.run.as_ref().unwrap().exit_code, Some(0));
    }

    #[test]
    fn stop_terminates_a_running_process() {
        let mut h = Harness::new(60, 20);
        h.app.start_run_argv(vec!["sleep".into(), "30".into()]);
        h.pump();
        assert!(h.app.run.as_ref().unwrap().is_running());

        h.app.stop_run();
        assert!(h.app.run.as_ref().unwrap().stopped);
        assert!(!h.app.run.as_ref().unwrap().is_running());
        assert_eq!(h.app.run.as_ref().unwrap().exit_code, None);

        // drain the StreamClosed events from the killed pipes; state stays stopped
        for _ in 0..20 {
            h.pump();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(h.app.run.as_ref().unwrap().stopped);
        assert!(h.contains("stopped"));
    }

    #[test]
    fn f5_runs_the_saved_file() {
        // A real interpreter probably isn't on PATH in CI; point vulpin_path at
        // a stub that echoes its file argument.
        let dir = std::env::temp_dir();
        let stub = dir.join(format!("vulide_stub_{}.sh", std::process::id()));
        std::fs::write(&stub, "#!/bin/sh\necho \"ran: $1\"\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let src = dir.join(format!("vulide_prog_{}.vul", std::process::id()));
        std::fs::write(&src, "G\"hi\"\n").unwrap();

        let mut h = Harness::new(70, 20);
        h.app.config.vulpin_path = stub.to_string_lossy().into_owned();
        h.app.open_path(src.clone()).unwrap();
        h.draw();

        h.key(KeyCode::F(5));
        wait_for_exit(&mut h);
        assert!(
            h.contains(&format!("ran: {}", src.display())),
            "stub output missing:\n{}",
            h.screen()
        );

        std::fs::remove_file(&stub).ok();
        std::fs::remove_file(&src).ok();
    }

    #[test]
    fn run_button_is_visible_and_clickable() {
        let dir = std::env::temp_dir();
        let stub = dir.join(format!("vulide_btn_stub_{}.sh", std::process::id()));
        std::fs::write(&stub, "#!/bin/sh\necho clicked-run\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let src = dir.join(format!("vulide_btn_prog_{}.vul", std::process::id()));
        std::fs::write(&src, "G\"x\"\n").unwrap();

        let mut h = Harness::new(70, 16);
        h.app.config.vulpin_path = stub.to_string_lossy().into_owned();
        h.app.open_path(src.clone()).unwrap();
        h.draw();

        assert!(h.contains("▶ Run"), "button missing:\n{}", h.screen());
        let btn = h.app.run_button.expect("button rect recorded");
        h.click(btn.x + 2, btn.y); // click on the "▶"
        wait_for_exit(&mut h);
        assert!(
            h.contains("clicked-run"),
            "click didn't run:\n{}",
            h.screen()
        );

        std::fs::remove_file(&stub).ok();
        std::fs::remove_file(&src).ok();
    }

    #[test]
    fn f1_opens_help_and_esc_closes() {
        let mut h = Harness::new(90, 44);
        h.key(KeyCode::F(1));
        assert!(h.contains("Keys & Shortcuts"));
        assert!(h.contains("save (Save As if untitled)"));
        assert!(h.contains("command palette"));
        assert!(h.contains("run the current file"));
        h.key(KeyCode::Esc);
        assert!(!h.app.overlay.is_open());

        // short terminal: scrolls instead of overflowing
        let mut h = Harness::new(90, 14);
        h.key(KeyCode::F(1));
        assert!(h.contains("Keys & Shortcuts"));
        for _ in 0..40 {
            h.key(KeyCode::Down);
        }
        assert!(h.contains("this help"), "scroll reached the last section");
    }

    #[test]
    fn palette_has_a_help_entry() {
        let mut h = Harness::new(80, 24);
        h.ctrl('p');
        h.type_str("help");
        h.key(KeyCode::Enter);
        assert!(matches!(
            h.app.overlay,
            crate::ui::overlay::Overlay::Help(_)
        ));
        assert!(h.contains("Keys & Shortcuts"));
    }

    #[test]
    fn click_outside_an_overlay_dismisses_it() {
        let mut h = Harness::new(90, 30);
        h.key(KeyCode::F(1));
        assert!(h.app.overlay.is_open());
        let r = h.app.overlay_rect.expect("overlay rect recorded");

        // a click inside the box does not close it
        h.click(r.x + 1, r.y + 1);
        assert!(h.app.overlay.is_open());

        // a click in the corner (outside) does
        h.click(0, 0);
        assert!(!h.app.overlay.is_open());
    }

    #[test]
    fn click_outside_theme_picker_reverts_preview() {
        let mut h = Harness::new(90, 30);
        h.ctrl('t');
        h.key(KeyCode::Down); // preview Latte
        assert_eq!(h.app.theme.name, "Light (Catppuccin Latte)");
        h.click(0, 0); // click away
        assert!(!h.app.overlay.is_open());
        assert_eq!(h.app.theme.name, "Dark (Catppuccin Mocha)");
    }

    #[test]
    fn clicking_between_panes_moves_focus() {
        let mut h = Harness::new(70, 20);
        h.app.start_run_argv(vec!["sleep".into(), "30".into()]);
        h.pump();
        assert_eq!(h.app.focus, crate::app::Focus::Output);

        let editor_row = 2;
        h.click(5, editor_row);
        assert_eq!(h.app.focus, crate::app::Focus::Editor, "click in editor");

        let pr = h.app.panel_rect.expect("panel rect");
        h.click(pr.x + 2, pr.y + 1);
        assert_eq!(h.app.focus, crate::app::Focus::Output, "click in panel");

        h.app.stop_run();
    }

    #[test]
    fn focus_returns_to_editor_when_the_run_finishes() {
        let mut h = Harness::new(70, 20);
        h.app.start_run_argv(vec!["printf".into(), "done\n".into()]);
        assert_eq!(h.app.focus, crate::app::Focus::Output);
        wait_for_exit(&mut h);
        assert_eq!(
            h.app.focus,
            crate::app::Focus::Editor,
            "keyboard should be back in the editor after the run"
        );
    }

    #[test]
    fn splitter_drag_resizes_the_panel() {
        let mut h = Harness::new(80, 30);
        h.app.start_run_argv(vec!["sleep".into(), "30".into()]);
        h.pump();
        let start_h = h.app.panel_rect.expect("panel").height;
        let sp = h.app.splitter_rect.expect("splitter rect");

        // drag the splitter up by 5 rows → panel grows
        h.drag(sp.x + sp.width / 2, sp.y, sp.x + sp.width / 2, sp.y - 5);
        let new_h = h.app.panel_rect.expect("panel").height;
        assert!(new_h > start_h, "panel {start_h} -> {new_h}");
        assert!(!h.app.dragging_splitter, "drag released");

        h.app.stop_run();
    }

    #[test]
    fn panel_close_button_closes_the_output() {
        let mut h = Harness::new(70, 20);
        h.app.start_run_argv(vec!["sleep".into(), "30".into()]);
        h.pump();
        let x = h.app.panel_close_rect.expect("close rect");
        h.click(x.x, x.y);
        assert!(h.app.run.is_none(), "output panel closed");
        assert_eq!(h.app.focus, crate::app::Focus::Editor);
    }

    #[test]
    fn clicking_a_tab_switches_and_its_x_closes_it() {
        let mut h = Harness::new(70, 14);
        h.ctrl('n');
        h.ctrl('n'); // three tabs, active = 2
        assert_eq!(h.app.buffers.len(), 3);
        assert_eq!(h.app.active, 2);

        let first = h.app.tab_hits[0];
        h.click(first.rect.x + 1, first.rect.y);
        assert_eq!(h.app.active, 0, "clicked tab 0");

        // close the (now) middle tab via its ✕
        let mid = h.app.tab_hits[1];
        h.click(mid.close.x, mid.close.y);
        assert_eq!(h.app.buffers.len(), 2);
    }

    // ---- Phase 5: find / replace ----

    #[test]
    fn find_bar_highlights_and_navigates() {
        let mut h = Harness::with_text("alpha beta alpha gamma alpha", 60, 12);
        h.ctrl('f');
        assert!(h.app.search.is_some());
        h.type_str("alpha");
        assert_eq!(h.app.search_matches.len(), 3);
        assert!(h.contains("1/3"), "counter shown:\n{}", h.screen());
        // incremental: cursor jumped to (the end of) the first match
        assert_eq!(h.app.buf().cursor(), Position { line: 0, col: 5 });

        h.key(KeyCode::Enter); // next
        assert_eq!(h.app.search_idx, 1);
        assert_eq!(h.app.buf().cursor(), Position { line: 0, col: 16 });

        h.key(KeyCode::Enter);
        h.key(KeyCode::Enter); // wraps 2 -> 0
        assert_eq!(h.app.search_idx, 0);

        h.key_mods(KeyCode::Enter, KeyModifiers::SHIFT); // prev, wraps 0 -> 2
        assert_eq!(h.app.search_idx, 2);

        h.key(KeyCode::Esc);
        assert!(h.app.search.is_none());
        assert!(h.app.search_matches.is_empty());
    }

    #[test]
    fn find_is_case_insensitive_until_toggled() {
        let mut h = Harness::with_text("Foo foo FOO", 50, 10);
        h.ctrl('f');
        h.type_str("foo");
        assert_eq!(h.app.search_matches.len(), 3);
        h.key_mods(KeyCode::Char('c'), KeyModifiers::ALT); // Alt+C
        assert_eq!(h.app.search_matches.len(), 1);
        assert!(h.contains("case: on"));
    }

    #[test]
    fn replace_one_then_replace_all() {
        let mut h = Harness::with_text("foo foo foo", 60, 12);
        h.ctrl('f');
        h.type_str("foo");
        assert_eq!(h.app.search_matches.len(), 3);

        h.key(KeyCode::Tab); // -> Replace field
        h.type_str("bar");
        h.ctrl('r'); // replace current + advance
        assert_eq!(h.app.buf().line_text(0), "bar foo foo");
        assert_eq!(h.app.search_matches.len(), 2);

        h.key_mods(KeyCode::Char('a'), KeyModifiers::ALT); // Alt+A replace all
        assert_eq!(h.app.buf().line_text(0), "bar bar bar");
        assert!(h.contains("replaced 2"));

        // one undo step per operation
        h.key(KeyCode::Esc);
        h.ctrl('z');
        assert_eq!(h.app.buf().line_text(0), "bar foo foo");
        h.ctrl('z');
        assert_eq!(h.app.buf().line_text(0), "foo foo foo");
    }

    #[test]
    fn find_seeds_from_selection() {
        let mut h = Harness::with_text("needle here and needle there", 60, 12);
        h.key(KeyCode::End);
        for _ in 0..5 {
            h.key_mods(KeyCode::Left, KeyModifiers::SHIFT); // select "there"
        }
        h.ctrl('f');
        assert_eq!(h.app.search.as_ref().unwrap().query(), "there");
    }

    #[test]
    fn clicking_another_tab_dismisses_the_find_bar() {
        // The bar captures the keyboard, so a tab switch only reaches here via
        // the mouse or the palette — either way its per-buffer matches must go.
        let mut h = Harness::new(70, 14);
        h.type_str("alpha alpha");
        h.ctrl('n');
        h.type_str("alpha");
        h.ctrl('f');
        h.type_str("alpha");
        assert!(h.app.search.is_some());

        let first = h.app.tab_hits[0];
        h.click(first.rect.x + 1, first.rect.y);
        assert_eq!(h.app.active, 0);
        assert!(h.app.search.is_none(), "find bar dropped on tab switch");
        assert!(h.app.search_matches.is_empty());
    }

    #[test]
    fn find_bar_shares_the_screen_with_the_output_panel() {
        let mut h = Harness::new(80, 24);
        h.app.start_run_argv(vec!["sleep".into(), "30".into()]);
        h.pump();
        h.app.focus = crate::app::Focus::Editor;
        h.ctrl('f');
        h.draw();
        // editor keeps at least the minimum height with tabs off + panel + bar
        assert!(h.app.editor_rect.height >= crate::ui::MIN_EDITOR_ROWS);
        assert!(h.app.panel_rect.is_some());
        assert!(h.app.search_rect.is_some());
        h.app.stop_run();
    }

    #[test]
    fn hovering_a_tab_marks_it() {
        let mut h = Harness::new(70, 14);
        h.ctrl('n'); // two tabs
        assert_eq!(h.app.hovered_tab, None);
        let t0 = h.app.tab_hits[0];
        h.mouse_move(t0.rect.x + 1, t0.rect.y);
        assert_eq!(h.app.hovered_tab, Some(0));
        h.mouse_move(0, 10); // move away (into the editor)
        assert_eq!(h.app.hovered_tab, None);
    }
}
