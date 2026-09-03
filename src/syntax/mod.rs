//! Syntax tokenizing. One trait, one implementation (Vulpin).
//!
//! Tokenizers are single-line: no cross-line state. That is enough for Vulpin —
//! the Python `VulpinHighlighter` is line-based too.

pub mod vulpin;

/// A token's character range is a half-open `[start, end)` interval of
/// **character** indices within its line (not bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub start: usize,
    pub end: usize,
    pub kind: TokenKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Comment,
    String,
    Number,
    Variable,
    Function,
    Command,
    Control,
    Operator,
    Bracket,
    Text,
}

pub trait Tokenizer {
    /// Tokens for a single line, ordered by `start`, non-overlapping. Gaps
    /// between tokens render as `TokenKind::Text`.
    fn tokenize_line(&self, line: &str) -> Vec<Token>;
}
