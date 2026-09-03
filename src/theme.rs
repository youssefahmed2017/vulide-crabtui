//! Colour roles used by the UI.
//!
//! Phase 1 ships one hard-coded theme (Catppuccin Mocha, VulIDE's default).
//! Phase 2 expands this to the full ~40 roles from the Python `THEME_KEYS` and
//! loads them from `themes/*.toml` with live switching.

use ratatui::style::Color;

use crate::syntax::TokenKind;

#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    pub bg: Color,
    pub fg: Color,
    pub gutter_fg: Color,
    pub gutter_current_fg: Color,
    pub current_line_bg: Color,
    pub selection_bg: Color,
    pub match_bracket_fg: Color,
    pub status_fg: Color,
    pub status_bg: Color,
    pub accent: Color,

    pub comment: Color,
    pub string: Color,
    pub number: Color,
    pub variable: Color,
    pub function: Color,
    pub command: Color,
    pub control: Color,
    pub operator: Color,
    pub bracket: Color,
}

impl Theme {
    pub fn mocha() -> Self {
        Self {
            name: "Dark (Catppuccin Mocha)".into(),
            bg: Color::Rgb(0x1e, 0x1e, 0x2e),
            fg: Color::Rgb(0xcd, 0xd6, 0xf4),
            gutter_fg: Color::Rgb(0x6c, 0x70, 0x86),
            gutter_current_fg: Color::Rgb(0xf5, 0xe0, 0xdc),
            current_line_bg: Color::Rgb(0x2a, 0x2a, 0x3c),
            selection_bg: Color::Rgb(0x45, 0x47, 0x5a),
            match_bracket_fg: Color::Rgb(0x89, 0xb4, 0xfa),
            status_fg: Color::Rgb(0xa6, 0xad, 0xc8),
            status_bg: Color::Rgb(0x18, 0x18, 0x25),
            accent: Color::Rgb(0x89, 0xb4, 0xfa),

            comment: Color::Rgb(0x6c, 0x70, 0x86),
            string: Color::Rgb(0xa6, 0xe3, 0xa1),
            number: Color::Rgb(0xfa, 0xb3, 0x87),
            variable: Color::Rgb(0xcb, 0xa6, 0xf7),
            function: Color::Rgb(0xf5, 0xc2, 0xe7),
            command: Color::Rgb(0x89, 0xb4, 0xfa),
            control: Color::Rgb(0x89, 0xb4, 0xfa),
            operator: Color::Rgb(0x89, 0xdc, 0xeb),
            bracket: Color::Rgb(0x94, 0xe2, 0xd5),
        }
    }

    pub fn token_color(&self, kind: TokenKind) -> Option<Color> {
        Some(match kind {
            TokenKind::Comment => self.comment,
            TokenKind::String => self.string,
            TokenKind::Number => self.number,
            TokenKind::Variable => self.variable,
            TokenKind::Function => self.function,
            TokenKind::Command => self.command,
            TokenKind::Control => self.control,
            TokenKind::Operator => self.operator,
            TokenKind::Bracket => self.bracket,
            TokenKind::Text => return None,
        })
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::mocha()
    }
}
