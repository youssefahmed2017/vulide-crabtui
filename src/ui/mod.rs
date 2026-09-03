//! Screen layout and draw dispatch.
//!
//! Phase 1: editor fills the screen with a one-row status bar. Phase 3 adds a
//! tab strip, side panel, and bottom panel around this.

pub mod editor;
pub mod overlay;
pub mod status;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};

use crate::app::App;
use crate::complete;
use overlay::Overlay;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(area);

    app.editor_rows = chunks[0].height as usize;
    let cursor_screen = editor::render(f, &mut app.buffer, &app.theme, chunks[0]);
    status::render(f, app, chunks[1]);

    // Autocomplete popup floats over the editor, anchored to the cursor. It is
    // non-modal, so it never draws while an overlay owns the screen.
    if !app.overlay.is_open()
        && let (Some(c), Some(pos)) = (&app.completion, cursor_screen)
    {
        complete::render_popup(f, c, pos, &app.theme, chunks[0]);
    }

    // Overlays draw last, over everything, and own the cursor while open.
    if let Overlay::SaveAs(prompt) = &app.overlay {
        overlay::render_save_as(f, prompt, &app.theme, area);
    }
}
