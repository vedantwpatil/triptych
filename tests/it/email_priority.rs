use chrono::Utc;
use triptych::email::EmailMessage;
use triptych::email::priority::{EmailSort, Level, level, score};

fn mail(subject: &str, snippet: &str, from: &str) -> EmailMessage {
    EmailMessage {
        id: 1,
        uid: 1,
        message_id: "m".into(),
        account: "default".into(),
        folder: "INBOX".into(),
        from_addr: from.into(),
        from_name: None,
        subject: subject.into(),
        date_utc: Utc::now(),
        snippet: Some(snippet.into()),
        is_read: false,
        task_id: None,
        body_text: None,
    }
}

fn level_of(subject: &str, snippet: &str, from: &str) -> Level {
    level(score(&mail(subject, snippet, from)))
}

#[test]
fn plain_mail_is_normal() {
    assert_eq!(level_of("lunch friday?", "", "a@b.c"), Level::Normal);
}

#[test]
fn high_keywords_rank_high_case_insensitively_and_across_punctuation() {
    assert_eq!(level_of("URGENT: server down", "", "a@b.c"), Level::High);
    assert_eq!(level_of("Action-Required!", "", "a@b.c"), Level::High);
    assert_eq!(level_of("hi", "your account expires soon", "a@b.c"), Level::High);
}

#[test]
fn medium_keywords_rank_medium() {
    assert_eq!(level_of("Assignment 3", "", "a@b.c"), Level::Medium);
    assert_eq!(level_of("hi", "the invoice is attached", "a@b.c"), Level::Medium);
}

#[test]
fn keywords_match_whole_words_only() {
    assert_eq!(level_of("redue the residue", "", "a@b.c"), Level::Normal);
    assert_eq!(level_of("urgently needed", "", "a@b.c"), Level::Normal);
}

#[test]
fn bulk_mail_is_pushed_down() {
    assert_eq!(level_of("Big sale, payment plans", "", "a@b.c"), Level::Normal);
    assert_eq!(level_of("Urgent", "", "noreply@shop.com"), Level::Medium);
    assert_eq!(level_of("Urgent", "unsubscribe below", "no-reply@x.com"), Level::Normal);
}

#[test]
fn converted_mail_drops_out_of_the_top() {
    let mut m = mail("Urgent deadline", "", "a@b.c");
    assert_eq!(level(score(&m)), Level::High);
    m.task_id = Some(4);
    assert_eq!(level(score(&m)), Level::Normal);
}

#[test]
fn read_state_never_changes_the_score() {
    let mut m = mail("Reminder: meeting", "", "a@b.c");
    let unread = score(&m);
    m.is_read = true;
    assert_eq!(score(&m), unread);
}

#[test]
fn sort_toggles_between_its_two_modes() {
    assert_eq!(EmailSort::default(), EmailSort::Priority);
    assert_eq!(EmailSort::Priority.toggled(), EmailSort::Date);
    assert_eq!(EmailSort::Date.toggled().label(), "priority");
}
