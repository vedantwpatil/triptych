use chrono::Utc;
use triptych::app::{normalize_subject, thread_count};
use triptych::email::EmailMessage;

fn mail(id: i64, subject: &str) -> EmailMessage {
    EmailMessage {
        id,
        uid: id,
        message_id: format!("<m{id}@example.com>"),
        account: "default".into(),
        folder: "INBOX".into(),
        from_addr: "sender@example.com".into(),
        from_name: None,
        subject: subject.into(),
        date_utc: Utc::now(),
        snippet: None,
        is_read: false,
        task_id: None,
        body_text: None,
        to_addrs: None,
        cc_addrs: None,
        references_header: None,
        is_starred: false,
        category: None,
        has_attachments: false,
        snoozed_until: None,
        triage_focused: None,
        meeting_title: None,
        meeting_start: None,
        meeting_end: None,
        meeting_location: None,
    }
}

#[test]
fn normalize_subject_strips_repeated_reply_and_forward_prefixes() {
    assert_eq!(normalize_subject("lunch?"), "lunch?");
    assert_eq!(normalize_subject("Re: lunch?"), "lunch?");
    assert_eq!(normalize_subject("RE: Fwd: lunch?"), "lunch?");
    assert_eq!(normalize_subject("Fw: Re: Re: lunch?"), "lunch?");
    assert_eq!(normalize_subject("  Re:   lunch?  "), "lunch?");
}

#[test]
fn thread_count_groups_by_normalized_subject() {
    let emails = vec![
        mail(1, "lunch?"),
        mail(2, "Re: lunch?"),
        mail(3, "coffee"),
        mail(4, "RE: Lunch?"),
    ];

    assert_eq!(thread_count(&emails, 1), 3);
    assert_eq!(thread_count(&emails, 2), 3);
    assert_eq!(thread_count(&emails, 3), 1);
    assert_eq!(thread_count(&emails, 4), 3);
}

#[test]
fn thread_count_never_groups_blank_subjects_together() {
    let emails = vec![mail(1, ""), mail(2, "")];

    assert_eq!(thread_count(&emails, 1), 1);
    assert_eq!(thread_count(&emails, 2), 1);
}

#[test]
fn thread_count_is_zero_for_an_unknown_id() {
    let emails = vec![mail(1, "lunch?")];

    assert_eq!(thread_count(&emails, 999), 0);
}

#[test]
fn thread_count_follows_a_references_header_chain_over_subject() {
    let emails = vec![
        mail(1, "lunch?"),
        EmailMessage {
            references_header: Some("<m1@example.com>".into()),
            ..mail(2, "totally unrelated subject")
        },
    ];

    assert_eq!(thread_count(&emails, 1), 2);
    assert_eq!(thread_count(&emails, 2), 2);
}

#[test]
fn thread_count_links_siblings_through_an_unfetched_ancestor() {
    let emails = vec![
        EmailMessage {
            references_header: Some("<ancestor@example.com>".into()),
            ..mail(1, "one subject")
        },
        EmailMessage {
            references_header: Some("<ancestor@example.com>".into()),
            ..mail(2, "a different subject")
        },
    ];

    assert_eq!(thread_count(&emails, 1), 2);
    assert_eq!(thread_count(&emails, 2), 2);
}

#[test]
fn thread_count_falls_back_to_subject_when_the_header_links_nothing_loaded() {
    let emails = vec![
        mail(1, "lunch?"),
        EmailMessage {
            references_header: Some("<not-loaded@example.com>".into()),
            ..mail(2, "Re: lunch?")
        },
    ];

    assert_eq!(thread_count(&emails, 1), 2);
    assert_eq!(thread_count(&emails, 2), 2);
}
