use triptych::app::{Edit, apply_edit, before_cursor, cursor_col};

fn run(text: &str, cursor: Option<usize>, edits: &[Edit]) -> (String, Option<usize>) {
    let mut text = text.to_string();
    let mut cursor = cursor;
    for e in edits {
        apply_edit(&mut text, &mut cursor, *e);
    }
    (text, cursor)
}

#[test]
fn word_moves_and_inserts_mid_text() {
    let (t, c) = run("one two three", None, &[Edit::WordLeft, Edit::Insert('X')]);
    assert_eq!((t.as_str(), c), ("one two Xthree", Some(9)));
    let (t, _) = run(
        "one two three",
        None,
        &[
            Edit::WordLeft,
            Edit::WordLeft,
            Edit::WordRight,
            Edit::Insert('!'),
        ],
    );
    assert_eq!(t, "one two! three");
}

#[test]
fn caret_returns_to_none_at_the_end() {
    let (_, c) = run("ab", None, &[Edit::Left, Edit::Right]);
    assert_eq!(c, None);
}

#[test]
fn delete_word_back_and_char_edits() {
    assert_eq!(run("one two  ", None, &[Edit::DeleteWordBack]).0, "one ");
    assert_eq!(run("abc", Some(1), &[Edit::Backspace]).0, "bc");
    assert_eq!(run("abc", Some(1), &[Edit::Delete]).0, "ac");
    assert_eq!(
        run("", None, &[Edit::Backspace, Edit::Delete, Edit::WordLeft]).0,
        ""
    );
}

#[test]
fn home_and_end_stay_on_the_current_line() {
    let (_, c) = run("ab\ncd", Some(4), &[Edit::Home]);
    assert_eq!(c, Some(3));
    let (_, c) = run("ab\ncd", Some(0), &[Edit::End]);
    assert_eq!(c, Some(2));
}

#[test]
fn multibyte_text_and_stale_cursors_never_panic() {
    let (t, _) = run(
        "héllo wörld",
        None,
        &[
            Edit::WordLeft,
            Edit::Backspace,
            Edit::Left,
            Edit::Insert('é'),
        ],
    );
    assert_eq!(t, "hélléowörld");
    // Cursor inside a multi-byte char and past the end are clamped, not fatal.
    assert_eq!(run("é", Some(1), &[Edit::Insert('x')]).0, "xé");
    assert_eq!(run("ab", Some(99), &[Edit::Insert('x')]).0, "abx");
    assert_eq!(cursor_col("héllo", Some(3)), 2);
    assert_eq!(before_cursor("héllo", Some(3)), "hé");
}
