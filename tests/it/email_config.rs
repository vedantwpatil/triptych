use triptych::email::config::*;

#[test]
fn parses_comma_separated_labels() {
    assert_eq!(
        parse_account_labels("work, personal ,  "),
        vec!["work".to_string(), "personal".to_string()]
    );
}

#[test]
fn parses_empty_string_to_no_labels() {
    assert_eq!(parse_account_labels(""), Vec::<String>::new());
}

#[test]
fn env_suffix_normalizes_label() {
    assert_eq!(env_suffix("work"), "_WORK");
    assert_eq!(env_suffix("my-personal"), "_MY_PERSONAL");
}

#[test]
fn debug_output_redacts_the_password() {
    let config = EmailConfig {
        account: "work".into(),
        imap_server: "imap.example.com".into(),
        imap_port: 993,
        imap_username: "me@example.com".into(),
        imap_password: "hunter2".into(),
        imap_folder: "INBOX".into(),
    };
    let shown = format!("{config:?}");
    assert!(!shown.contains("hunter2"));
    assert!(shown.contains("<redacted>"));
}
