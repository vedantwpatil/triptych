use chrono::{TimeZone, Utc};
use triptych::email::message::*;

#[test]
fn strips_preheader_padding() {
    let padded = "PNC Financial Services Group is hiring\u{A0}\u{200C}\u{200D}\u{200E}\u{200F}\u{FEFF}\u{A0}\u{200C}\u{200D}\u{200E}\u{200F}\u{FEFF}";
    assert_eq!(
        clean_snippet(padded),
        "PNC Financial Services Group is hiring"
    );
}

const RAW_WITH_INVITE: &[u8] = b"From: Organizer <organizer@example.com>\r\n\
To: Receiver <receiver@example.com>\r\n\
Subject: Invitation: Team Sync\r\n\
Date: Mon, 1 Jan 2024 00:00:00 +0000\r\n\
Message-ID: <invite1@example.com>\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"BOUNDARY\"\r\n\
\r\n\
--BOUNDARY\r\n\
Content-Type: text/plain; charset=\"utf-8\"\r\n\
\r\n\
You are invited.\r\n\
--BOUNDARY\r\n\
Content-Type: text/calendar; method=REQUEST; charset=\"utf-8\"\r\n\
\r\n\
BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
PRODID:-//Test//EN\r\n\
BEGIN:VEVENT\r\n\
UID:event1@example.com\r\n\
DTSTAMP:20240101T000000Z\r\n\
DTSTART:20240115T140000Z\r\n\
DTEND:20240115T150000Z\r\n\
SUMMARY:Team Sync\r\n\
LOCATION:Conference Room A\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n\
--BOUNDARY--\r\n";

#[test]
fn parse_raw_extracts_meeting_invite_fields() {
    let email = parse_raw("default", 1, "INBOX", RAW_WITH_INVITE, false).unwrap();

    assert_eq!(email.meeting_title.as_deref(), Some("Team Sync"));
    assert_eq!(
        email.meeting_start,
        Some(Utc.with_ymd_and_hms(2024, 1, 15, 14, 0, 0).unwrap())
    );
    assert_eq!(
        email.meeting_end,
        Some(Utc.with_ymd_and_hms(2024, 1, 15, 15, 0, 0).unwrap())
    );
    assert_eq!(email.meeting_location.as_deref(), Some("Conference Room A"));
}

#[test]
fn parse_raw_header_only_skips_meeting_invite_extraction() {
    let email = parse_raw("default", 1, "INBOX", RAW_WITH_INVITE, true).unwrap();

    assert!(email.meeting_title.is_none());
    assert!(email.meeting_start.is_none());
}

#[test]
fn parse_raw_without_a_calendar_part_has_no_meeting_fields() {
    let email = parse_raw(
        "default",
        1,
        "INBOX",
        b"From: a@b.c\r\nSubject: no invite\r\n\r\nplain body\r\n",
        false,
    )
    .unwrap();

    assert!(email.meeting_title.is_none());
    assert!(email.meeting_start.is_none());
    assert!(email.meeting_end.is_none());
    assert!(email.meeting_location.is_none());
}
