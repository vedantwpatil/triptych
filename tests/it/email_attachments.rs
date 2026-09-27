use triptych::app::sanitize_filename;
use triptych::email::message::parse_raw;

const RAW_WITH_ATTACHMENT: &[u8] = b"From: Sender <sender@example.com>\r\n\
To: Receiver <receiver@example.com>\r\n\
Subject: Report attached\r\n\
Date: Mon, 1 Jan 2024 00:00:00 +0000\r\n\
Message-ID: <test1@example.com>\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"BOUNDARY\"\r\n\
\r\n\
--BOUNDARY\r\n\
Content-Type: text/plain; charset=\"utf-8\"\r\n\
\r\n\
Hello world\r\n\
--BOUNDARY\r\n\
Content-Type: application/pdf\r\n\
Content-Disposition: attachment; filename=\"report.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
JVBERi0xLjQK\r\n\
--BOUNDARY--\r\n";

#[test]
fn parse_raw_extracts_attachment_metadata() {
    let email = parse_raw("default", 1, "INBOX", RAW_WITH_ATTACHMENT, false).unwrap();

    assert_eq!(email.attachments.len(), 1);
    let attachment = &email.attachments[0];
    assert_eq!(attachment.filename.as_deref(), Some("report.pdf"));
    assert_eq!(attachment.content_type, "application/pdf");
    assert!(attachment.size_bytes > 0);
}

#[test]
fn parse_raw_header_only_skips_attachment_extraction() {
    let email = parse_raw("default", 1, "INBOX", RAW_WITH_ATTACHMENT, true).unwrap();

    assert!(email.attachments.is_empty());
}

#[test]
fn parse_raw_without_attachments_is_empty() {
    let email = parse_raw(
        "default",
        1,
        "INBOX",
        b"From: a@b.c\r\nSubject: no attachment\r\n\r\nplain body\r\n",
        false,
    )
    .unwrap();

    assert!(email.attachments.is_empty());
}

#[test]
fn sanitize_filename_passes_through_a_plain_name() {
    assert_eq!(
        sanitize_filename("report.pdf"),
        Some("report.pdf".to_string())
    );
}

#[test]
fn sanitize_filename_strips_directory_traversal() {
    assert_eq!(
        sanitize_filename("../../etc/passwd"),
        Some("passwd".to_string())
    );
    assert_eq!(
        sanitize_filename("/etc/../../etc/shadow"),
        Some("shadow".to_string())
    );
}

#[test]
fn sanitize_filename_rejects_an_empty_or_traversal_only_name() {
    assert_eq!(sanitize_filename(""), None);
    assert_eq!(sanitize_filename(".."), None);
    assert_eq!(sanitize_filename("../.."), None);
}
