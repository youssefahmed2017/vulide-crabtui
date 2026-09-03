//! Screen layout and draw dispatch.
//!
//! Rows: an optional tab strip, the editor, an optional draggable splitter +
//! run-output panel, and a one-row status bar. The algorithm viewer slots in
//! beside the editor in Phase 5.

pub mod editor;
pub mod help;
pub mod overlay;
pub mod palette;
pub mod panel;
pub mod splitter;
pub mod status;
pub mod tabs;
pub mod theme_picker;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};

use crate::app::{App, Focus};
use crate::complete;
use overlay::Overlay;

/// Minimum rows the editor keeps when the panel is open / being resized.
pub const MIN_EDITOR_ROWS: u16 = 3;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let show_tabs = app.buffers.len() > 1;
    let show_panel = app.run.is_some();

    let mut rows = Vec::new();
    if show_tabs {
        rows.push(Constraint::Length(1));
    }
    rows.push(Constraint::Min(MIN_EDITOR_ROWS));
    if show_panel {
        rows.push(Constraint::Length(1)); // splitter
        rows.push(Constraint::Length(panel_height(app, area)));
    }
    rows.push(Constraint::Length(1)); // status
    let chunks = Layout::vertical(rows).split(area);

    let mut i = 0;
    if show_tabs {
        app.tab_hits = tabs::render(f, app, chunks[i]);
        i += 1;
    } else {
        app.tab_hits.clear();
    }
    let editor_area = chunks[i];
    i += 1;
    let (splitter_area, panel_area) = if show_panel {
        let s = chunks[i];
        let p = chunks[i + 1];
        i += 2;
        (Some(s), Some(p))
    } else {
        (None, None)
    };
    let status_area = chunks[i];

    app.editor_rect = editor_area;
    app.status_rect = status_area;
    app.splitter_rect = splitter_area;
    app.panel_rect = panel_area;

    app.editor_rows = editor_area.height as usize;
    let show_numbers = app.config.show_line_numbers;
    let cursor_screen = editor::render(
        f,
        &mut app.buffers[app.active],
        &app.theme,
        show_numbers,
        editor_area,
    );

    if let Some(s) = splitter_area {
        splitter::render(
            f,
            &app.theme,
            app.dragging_splitter || app.hover_splitter,
            s,
        );
    }

    if let (Some(panel_area), Some(console)) = (panel_area, &app.run) {
        let close = panel::render(
            f,
            console,
            &app.theme,
            app.focus == Focus::Output,
            app.hover_panel_close,
            panel_area,
        );
        app.panel_close_rect = Some(close);
    } else {
        app.panel_close_rect = None;
    }

    // Record the run/stop button's hit rect (leftmost cells of the status bar).
    let btn_w = (app.run_button_label().chars().count() as u16).min(status_area.width);
    app.run_button = Some(Rect {
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

/// Panel height: the user's dragged value, else a third of the screen, always
/// leaving the editor at least `MIN_EDITOR_ROWS` (plus the splitter and status).
pub fn panel_height(app: &App, area: Rect) -> u16 {
    let tabs = if app.buffers.len() > 1 { 1 } else { 0 };
    let reserved = tabs + 1 /* splitter */ + 1 /* status */ + MIN_EDITOR_ROWS;
    let max = area.height.saturating_sub(reserved).max(1);
    let want = app
        .panel_height
        .unwrap_or_else(|| ((area.height as usize) / 3).clamp(6, 16) as u16);
    want.clamp(3, max)
}
