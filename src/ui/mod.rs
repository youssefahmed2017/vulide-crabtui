//! Screen layout and draw dispatch.
//!
//! Phase 3: a tab strip above the editor, a one-row status bar below. Panels
//! (output console, algorithm viewer) slot in around this in Phases 4–5.

pub mod editor;
pub mod overlay;
pub mod palette;
pub mod status;
pub mod tabs;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};

use crate::app::App;
use crate::complete;
use overlay::Overlay;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let show_tabs = app.buffers.len() > 1;
    let constraints = if show_tabs {
        vec![
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ]
    } else {
        vec![Constraint::Min(1), Constraint::Length(1)]
    };
    let chunks = Layout::vertical(constraints).split(area);
    let (editor_area, status_area) = if show_tabs {
        tabs::render(f, app, chunks[0]);
        (chunks[1], chunks[2])
    } else {
        (chunks[0], chunks[1])
    };

    app.editor_rows = editor_area.height as usize;
    let show_numbers = app.config.show_line_numbers;
    let cursor_screen = editor::render(
        f,
        &mut app.buffers[app.active],
        &app.theme,
        show_numbers,
        editor_area,
    );
    status::render(f, app, status_area);

    // Autocomplete popup floats over the editor, anchored to the cursor. It is
    // non-modal, so it never draws while an overlay owns the screen.
    if !app.overlay.is_open()
        && let (Some(c), Some(pos)) = (&app.completion, cursor_screen)
    {
        complete::render_popup(f, c, pos, &app.theme, editor_area);
    }

    // Overlays draw last, over everything, and own the cursor while open.
    match &app.overlay {
        Overlay::Prompt(prompt) => overlay::render_prompt(f, prompt, &app.theme, area),
        Overlay::Palette(palette) => palette::render(f, palette, &app.theme, area),
        Overlay::None => {}
    }
}
