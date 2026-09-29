use chrono::Utc;
use triptych::app::{match_rule, parse_rule_spec};
use triptych::email::EmailRule;

fn rule(match_field: &str, pattern: &str) -> EmailRule {
    EmailRule {
        id: 1,
        match_field: match_field.into(),
        pattern: pattern.into(),
        action: "star".into(),
        created_at: Utc::now(),
    }
}

#[test]
fn parse_rule_spec_reads_subject_field() {
    assert_eq!(
        parse_rule_spec("subject newsletter star"),
        Some((
            "subject".to_string(),
            "newsletter".to_string(),
            "star".to_string()
        ))
    );
}

#[test]
fn parse_rule_spec_maps_from_and_sender_to_from_addr() {
    assert_eq!(
        parse_rule_spec("from noreply read"),
        Some((
            "from_addr".to_string(),
            "noreply".to_string(),
            "read".to_string()
        ))
    );
    assert_eq!(
        parse_rule_spec("sender boss@work.com star"),
        Some((
            "from_addr".to_string(),
            "boss@work.com".to_string(),
            "star".to_string()
        ))
    );
}

#[test]
fn parse_rule_spec_keeps_a_multi_word_pattern_intact() {
    assert_eq!(
        parse_rule_spec("subject weekly team sync star"),
        Some((
            "subject".to_string(),
            "weekly team sync".to_string(),
            "star".to_string()
        ))
    );
}

#[test]
fn parse_rule_spec_rejects_an_unknown_field() {
    assert_eq!(parse_rule_spec("body newsletter star"), None);
}

#[test]
fn parse_rule_spec_rejects_an_unknown_action() {
    assert_eq!(parse_rule_spec("subject newsletter snooze"), None);
}

#[test]
fn parse_rule_spec_reads_archive_and_delete_actions() {
    assert_eq!(
        parse_rule_spec("subject newsletter archive"),
        Some((
            "subject".to_string(),
            "newsletter".to_string(),
            "archive".to_string()
        ))
    );
    assert_eq!(
        parse_rule_spec("from spam@example.com delete"),
        Some((
            "from_addr".to_string(),
            "spam@example.com".to_string(),
            "delete".to_string()
        ))
    );
}

#[test]
fn parse_rule_spec_rejects_a_missing_pattern_or_action() {
    assert_eq!(parse_rule_spec("subject star"), None);
    assert_eq!(parse_rule_spec("subject"), None);
    assert_eq!(parse_rule_spec(""), None);
}

#[test]
fn match_rule_tests_subject_case_insensitively() {
    let r = rule("subject", "Newsletter");
    assert!(match_rule(&r, "Weekly newsletter digest", "a@b.c"));
    assert!(!match_rule(&r, "Team sync", "a@b.c"));
}

#[test]
fn match_rule_tests_from_addr_not_subject() {
    let r = rule("from_addr", "noreply");
    assert!(match_rule(&r, "Newsletter", "NoReply@example.com"));
    assert!(!match_rule(
        &r,
        "noreply mentioned in subject",
        "boss@example.com"
    ));
}
