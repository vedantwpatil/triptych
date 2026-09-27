use chrono::Utc;
use triptych::app::{
    ComposeField, ComposeState, chain_references, forward_subject, merge_reply_all_cc,
    quote_original, reply_subject,
};
use triptych::email::EmailMessage;

fn mail(subject: &str) -> EmailMessage {
    EmailMessage {
        id: 1,
        uid: 1,
        message_id: "<m1@example.com>".into(),
        account: "default".into(),
        folder: "INBOX".into(),
        from_addr: "sender@example.com".into(),
        from_name: Some("Sender Name".into()),
        subject: subject.into(),
        date_utc: Utc::now(),
        snippet: None,
        is_read: false,
        task_id: None,
        body_text: Some("line one\nline two".into()),
        to_addrs: Some("me@example.com".into()),
        cc_addrs: Some("cc1@example.com, cc2@example.com".into()),
        references_header: None,
        is_starred: false,
        has_attachments: false,
        snoozed_until: None,
        triage_focused: None,
    }
}

#[test]
fn reply_subject_adds_re_prefix_once() {
    assert_eq!(reply_subject("lunch?"), "Re: lunch?");
    assert_eq!(reply_subject("Re: lunch?"), "Re: lunch?");
    assert_eq!(reply_subject("RE: lunch?"), "RE: lunch?");
}

#[test]
fn forward_subject_adds_fwd_prefix_once() {
    assert_eq!(forward_subject("lunch?"), "Fwd: lunch?");
    assert_eq!(forward_subject("Fwd: lunch?"), "Fwd: lunch?");
}

#[test]
fn subject_prefixing_does_not_panic_on_short_multibyte_subjects() {
    // Regression guard: byte-slicing a subject shorter than the prefix, or one whose first
    // bytes aren't a char boundary, must not panic.
    assert_eq!(reply_subject("é"), "Re: é");
    assert_eq!(forward_subject("hi"), "Fwd: hi");
}

#[test]
fn chain_references_appends_message_id_to_existing_references() {
    let mut email = mail("hi");
    email.references_header = Some("<a@x> <b@x>".into());
    assert_eq!(chain_references(&email), "<a@x> <b@x> <m1@example.com>");
}

#[test]
fn chain_references_falls_back_to_just_the_message_id() {
    let email = mail("hi");
    assert_eq!(chain_references(&email), "<m1@example.com>");
}

#[test]
fn quote_original_prefixes_every_body_line() {
    let email = mail("hi");
    let quoted = quote_original(&email);
    assert!(quoted.contains("Sender Name wrote:"));
    assert!(quoted.contains("> line one\n"));
    assert!(quoted.contains("> line two\n"));
}

#[test]
fn reply_all_cc_merges_to_and_cc_dropping_self_and_the_direct_recipient() {
    let email = mail("hi");
    let cc = merge_reply_all_cc(&email, "sender@example.com", "me@example.com");
    // to_addrs ("me@example.com") is the replier's own address, dropped by `own_addr`;
    // `to` ("sender@example.com") is already the reply's direct recipient, dropped too.
    assert_eq!(cc, "cc1@example.com, cc2@example.com");
}

#[test]
fn compose_field_cycles_forward_and_back_through_all_four_fields() {
    let mut state = ComposeState::blank("default".to_string());
    assert_eq!(state.active_field, ComposeField::To);

    state.next_field();
    assert_eq!(state.active_field, ComposeField::Cc);
    state.next_field();
    assert_eq!(state.active_field, ComposeField::Subject);
    state.next_field();
    assert_eq!(state.active_field, ComposeField::Body);
    state.next_field();
    assert_eq!(state.active_field, ComposeField::To);

    state.prev_field();
    assert_eq!(state.active_field, ComposeField::Body);
}
