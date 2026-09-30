//! Cursor editing for the app's single-line and body text inputs, terminal-free so tests can reach it.
//!
//! The cursor is a byte offset into the edited `String`; `None` means "at the end", the state every
//! input starts in. Offsets are clamped and snapped to a char boundary on use, so a stale cursor
//! after the text changed elsewhere can never panic.

/// One editing action, mapped from a key by `tui/keys.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    Insert(char),
    Backspace,
    Delete,
    Left,
    Right,
    WordLeft,
    WordRight,
    /// Start of the current line.
    Home,
    /// End of the current line.
    End,
    DeleteWordBack,
}

fn resolve(text: &str, cursor: Option<usize>) -> usize {
    let mut pos = cursor.map_or(text.len(), |c| c.min(text.len()));
    while !text.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

fn word_left(text: &str, pos: usize) -> usize {
    let head = text[..pos].trim_end_matches(char::is_whitespace);
    head.trim_end_matches(|c: char| !c.is_whitespace()).len()
}

fn word_right(text: &str, pos: usize) -> usize {
    let tail = &text[pos..];
    let rest = tail.trim_start_matches(char::is_whitespace);
    let rest = rest.trim_start_matches(|c: char| !c.is_whitespace());
    text.len() - rest.len()
}

/// Applies `edit` to `text` at `cursor`, leaving `cursor` where the caret should now be.
pub fn apply(text: &mut String, cursor: &mut Option<usize>, edit: Edit) {
    let pos = resolve(text, *cursor);
    let new = match edit {
        Edit::Insert(c) => {
            text.insert(pos, c);
            pos + c.len_utf8()
        }
        Edit::Backspace => text[..pos].chars().next_back().map_or(pos, |c| {
            let start = pos - c.len_utf8();
            text.remove(start);
            start
        }),
        Edit::Delete => {
            if pos < text.len() {
                text.remove(pos);
            }
            pos
        }
        Edit::Left => text[..pos]
            .chars()
            .next_back()
            .map_or(pos, |c| pos - c.len_utf8()),
        Edit::Right => text[pos..]
            .chars()
            .next()
            .map_or(pos, |c| pos + c.len_utf8()),
        Edit::WordLeft => word_left(text, pos),
        Edit::WordRight => word_right(text, pos),
        Edit::Home => text[..pos].rfind('\n').map_or(0, |i| i + 1),
        Edit::End => text[pos..].find('\n').map_or(text.len(), |i| pos + i),
        Edit::DeleteWordBack => {
            let start = word_left(text, pos);
            text.replace_range(start..pos, "");
            start
        }
    };
    *cursor = (new < text.len()).then_some(new);
}

/// Char count before the cursor: the column a single-line input's caret is drawn at.
#[must_use]
pub fn cursor_col(text: &str, cursor: Option<usize>) -> usize {
    text[..resolve(text, cursor)].chars().count()
}

/// The text before the cursor, for placing the caret in a multi-line body.
#[must_use]
pub fn before_cursor(text: &str, cursor: Option<usize>) -> &str {
    &text[..resolve(text, cursor)]
}
