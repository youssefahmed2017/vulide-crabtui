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
}
