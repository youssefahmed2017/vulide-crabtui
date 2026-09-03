//! Screen layout and draw dispatch.
//!
//! Rows: an optional tab strip, the editor (optionally sharing its row with the
//! structure-outline sidebar), an optional draggable splitter + run-output
//! panel, an optional find bar, and a one-row status bar.

pub mod algo;
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
/// Below this total width the outline sidebar hides itself rather than starve
/// the editor of columns.
pub const ALGO_MIN_TOTAL_WIDTH: u16 = 56;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let show_tabs = app.buffers.len() > 1;
    let show_panel = app.run.is_some();
    let show_search = app.search.is_some();
    let show_algo = app.show_algo && area.width >= ALGO_MIN_TOTAL_WIDTH;

    let mut rows = Vec::new();
    if show_tabs {
        rows.push(Constraint::Length(1));
    }
    rows.push(Constraint::Min(MIN_EDITOR_ROWS));
    if show_panel {
        rows.push(Constraint::Length(1)); // splitter
        rows.push(Constraint::Length(panel_height(app, area)));
    }
    if show_search {
        rows.push(Constraint::Length(crate::search::SEARCH_ROWS));
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
    let editor_row = chunks[i];
    i += 1;
    let (algo_area, editor_area) = if show_algo {
        let w = (area.width / 4).clamp(18, 32);
        let cols =
            Layout::horizontal([Constraint::Length(w), Constraint::Min(20)]).split(editor_row);
        (Some(cols[0]), cols[1])
    } else {
        (None, editor_row)
    };
    let (splitter_area, panel_area) = if show_panel {
        let s = chunks[i];
        let p = chunks[i + 1];
        i += 2;
        (Some(s), Some(p))
    } else {
        (None, None)
    };
    let search_area = if show_search {
        let s = chunks[i];
        i += 1;
        Some(s)
    } else {
        None
    };
    let status_area = chunks[i];

    app.editor_rect = editor_area;
    app.status_rect = status_area;
    app.splitter_rect = splitter_area;
    app.panel_rect = panel_area;
    app.search_rect = search_area;
    app.algo_rect = algo_area;

    // Rebuild the outline (cheap) and keep the selection/scroll consistent.
    if show_algo {
        app.algo_items = crate::algo::outline(&app.buffers[app.active]);
        let n = app.algo_items.len();
        if app.algo_selected >= n {
            app.algo_selected = n.saturating_sub(1);
        }
        let body_h = algo_area
            .map(|a| a.height.saturating_sub(2) as usize)
            .unwrap_or(0);
        app.algo_scroll = scroll_into_view(app.algo_selected, app.algo_scroll, body_h);
    } else {
        app.algo_items.clear();
        app.algo_scroll = 0;
    }

    app.diagnostics = crate::lint::check(&app.buffers[app.active]);

    app.editor_rows = editor_area.height as usize;
    let show_numbers = app.config.show_line_numbers;
    let search_matches: &[(crate::buffer::Position, crate::buffer::Position)] = if show_search {
        &app.search_matches
    } else {
        &[]
    };
    let diag_ranges: Vec<(crate::buffer::Position, crate::buffer::Position)> =
        app.diagnostics.iter().map(|d| (d.start, d.end)).collect();
    let cursor_screen = editor::render(
        f,
        &mut app.buffers[app.active],
        &app.theme,
        show_numbers,
        search_matches,
        app.search_idx,
        &diag_ranges,
        editor_area,
    );

    if let Some(aa) = algo_area {
        algo::render(
            f,
            &app.algo_items,
            app.algo_selected,
            app.algo_scroll,
            &app.theme,
            app.focus == Focus::Algo,
            aa,
        );
    }

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

    if let (Some(sa), Some(s)) = (search_area, &app.search) {
        let cur = if app.search_matches.is_empty() {
            0
        } else {
            app.search_idx + 1
        };
        crate::search::render(f, s, &app.theme, (cur, app.search_matches.len()), sa);
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
        && app.search.is_none()
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

/// Smallest scroll offset that keeps row `sel` within a `h`-tall viewport.
fn scroll_into_view(sel: usize, cur: usize, h: usize) -> usize {
    if h == 0 {
        0
    } else if sel < cur {
        sel
    } else if sel >= cur + h {
        sel + 1 - h
    } else {
        cur
    }
}

/// Panel height: the user's dragged value, else a third of the screen, always
/// leaving the editor at least `MIN_EDITOR_ROWS` (plus the splitter and status).
pub fn panel_height(app: &App, area: Rect) -> u16 {
    let tabs = if app.buffers.len() > 1 { 1 } else { 0 };
    let search = if app.search.is_some() {
        crate::search::SEARCH_ROWS
    } else {
        0
    };
    let reserved = tabs + 1 /* splitter */ + search + 1 /* status */ + MIN_EDITOR_ROWS;
    let max = area.height.saturating_sub(reserved).max(1);
    let want = app
        .panel_height
        .unwrap_or_else(|| ((area.height as usize) / 3).clamp(6, 16) as u16);
    want.clamp(3, max)
}
