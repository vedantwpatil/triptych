//! `apply`: the sidecar protocol's event parsing, which is also its trust boundary (field labels
//! come from web pages and end up on the terminal).

use triptych::app::{ApplyEvent, parse_event};

#[test]
fn every_event_parses() {
    assert_eq!(
        parse_event(
            r#"{"event":"filled","ats":"greenhouse","filled":["Email"],"unanswered":["Why us?"],"screenshot":"/tmp/a.png"}"#
        ),
        Ok(ApplyEvent::Filled {
            ats: "greenhouse".into(),
            filled: vec!["Email".into()],
            unanswered: vec!["Why us?".into()],
            screenshot: Some("/tmp/a.png".into()),
        })
    );
    assert_eq!(
        parse_event(r#"{"event":"filled"}"#),
        Ok(ApplyEvent::Filled {
            ats: String::new(),
            filled: vec![],
            unanswered: vec![],
            screenshot: None,
        })
    );
    assert_eq!(
        parse_event(r#"{"event":"submitted"}"#),
        Ok(ApplyEvent::Submitted)
    );
    assert_eq!(
        parse_event(r#"{"event":"unsupported","ats":"workday"}"#),
        Ok(ApplyEvent::Unsupported {
            ats: "workday".into()
        })
    );
    assert_eq!(
        parse_event(r#"{"event":"error","message":"no form"}"#),
        Ok(ApplyEvent::Error {
            message: "no form".into()
        })
    );
}

#[test]
fn unknown_or_malformed_lines_are_errors() {
    for line in [
        "",
        "not json",
        r#"{"event":"explode"}"#,
        r#"{"cmd":"fill"}"#,
    ] {
        assert!(parse_event(line).is_err(), "{line:?} should not parse");
    }
}

#[test]
fn page_text_is_one_clean_capped_line() {
    let long = "x".repeat(500);
    let event = parse_event(
        &serde_json::json!({
            "event": "filled",
            "ats": "green\u{1b}[31mhouse",
            "filled": ["First\nname", "", long],
            "unanswered": (0..50).map(|i| format!("q{i}")).collect::<Vec<_>>(),
        })
        .to_string(),
    );
    let Ok(ApplyEvent::Filled {
        ats,
        filled,
        unanswered,
        ..
    }) = event
    else {
        panic!("expected a filled event, got {event:?}");
    };
    assert_eq!(ats, "green[31mhouse", "control characters are dropped");
    assert_eq!(filled[0], "First name", "line breaks collapse to a space");
    assert_eq!(filled.len(), 2, "an empty item is dropped");
    assert_eq!(filled[1].chars().count(), 120, "long text is cut");
    assert_eq!(unanswered.len(), 20, "the list is capped");
}
