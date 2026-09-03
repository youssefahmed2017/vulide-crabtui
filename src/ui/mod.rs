//! Screen layout and draw dispatch.
//!
//! Rows: an optional tab strip, the editor, an optional run-output panel, and a
//! one-row status bar. The algorithm viewer slots in beside the editor in
//! Phase 5.

pub mod editor;
pub mod help;
pub mod overlay;
pub mod palette;
pub mod panel;
pub mod status;
pub mod tabs;
pub mod theme_picker;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};

use crate::app::{App, Focus};
use crate::complete;
use overlay::Overlay;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let show_tabs = app.buffers.len() > 1;

    let mut rows = Vec::new();
    if show_tabs {
        rows.push(Constraint::Length(1));
    }
    rows.push(Constraint::Min(1)); // editor
    let show_panel = app.run.is_some();
    if show_panel {
        let h = ((area.height as usize) / 3).clamp(6, 16) as u16;
        rows.push(Constraint::Length(h));
    }
    rows.push(Constraint::Length(1)); // status
    let chunks = Layout::vertical(rows).split(area);

    let mut i = 0;
    if show_tabs {
        tabs::render(f, app, chunks[i]);
        i += 1;
    }
    let editor_area = chunks[i];
    i += 1;
    let panel_area = if show_panel {
        let a = chunks[i];
        i += 1;
        Some(a)
    } else {
        None
    };
    let status_area = chunks[i];

    app.editor_rows = editor_area.height as usize;
    let show_numbers = app.config.show_line_numbers;
    let cursor_screen = editor::render(
        f,
        &mut app.buffers[app.active],
        &app.theme,
        show_numbers,
        editor_area,
    );

    app.panel_rect = panel_area;
    if let (Some(panel_area), Some(console)) = (panel_area, &app.run) {
        panel::render(
            f,
            console,
            &app.theme,
            app.focus == Focus::Output,
            panel_area,
        );
    }

    // Record the run/stop button's hit rect (leftmost cells of the status bar).
    let btn_w = (app.run_button_label().chars().count() as u16).min(status_area.width);
    app.run_button = Some(ratatui::layout::Rect {
        x: status_area.x,
        y: status_area.y,
        width: btn_w,
        height: 1,
    });
    status::render(f, app, status_area);

    // Autocomplete popup floats over the editor, anchored to the cursor. It is
    // non-modal, so it never draws while an overlay owns the screen or the
    // output panel has focus.
    if !app.overlay.is_open()
        && app.focus == Focus::Editor
        && let (Some(c), Some(pos)) = (&app.completion, cursor_screen)
    {
        complete::render_popup(f, c, pos, &app.theme, editor_area);
    }

    // Overlays draw last, over everything, and own the cursor while open. Record
    // the outer rect so a click outside it can dismiss the overlay.
    app.overlay_rect = match &app.overlay {
        Overlay::Prompt(prompt) => Some(overlay::render_prompt(f, prompt, &app.theme, area)),
        Overlay::Palette(palette) => Some(palette::render(f, palette, &app.theme, area)),
        Overlay::ThemePicker(picker) => Some(theme_picker::render(f, picker, &app.theme, area)),
        Overlay::Help(h) => Some(help::render(f, h, &app.theme, area)),
        Overlay::None => None,
    };
}
