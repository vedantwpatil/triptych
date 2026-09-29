use triptych::email::smtp::{base64_encode, dot_stuff, parse_recipients};

#[test]
fn base64_matches_known_vectors() {
    assert_eq!(base64_encode(b""), "");
    assert_eq!(base64_encode(b"f"), "Zg==");
    assert_eq!(base64_encode(b"fo"), "Zm8=");
    assert_eq!(base64_encode(b"foo"), "Zm9v");
    assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
}

#[test]
fn dot_stuff_escapes_leading_dot() {
    assert_eq!(dot_stuff("hello\r\n.\r\nworld"), "hello\r\n..\r\nworld");
    assert_eq!(dot_stuff("no dots here"), "no dots here");
    assert_eq!(dot_stuff(".leading"), "..leading");
}

#[test]
fn parse_recipients_strips_display_names() {
    let got = parse_recipients("Jane Doe <jane@example.com>, bob@example.com", "");
    assert_eq!(got, vec!["jane@example.com", "bob@example.com"]);
}

#[test]
fn parse_recipients_merges_to_and_cc() {
    let got = parse_recipients("a@example.com", "b@example.com; c@example.com");
    assert_eq!(got, vec!["a@example.com", "b@example.com", "c@example.com"]);
}
