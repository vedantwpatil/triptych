use std::str::FromStr;

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use icalendar::{Calendar, CalendarDateTime, Component, DatePerhapsTime, EventLike};
use mail_parser::{MessageParser, MimeHeaders};
use sqlx::FromRow;

/// A stored email, mirrors the `email_messages` table.
#[derive(Debug, Clone, FromRow)]
pub struct EmailMessage {
    pub id: i64,
    pub uid: i64,
    pub message_id: String,
    pub account: String,
    pub folder: String,
    pub from_addr: String,
    pub from_name: Option<String>,
    pub subject: String,
    pub date_utc: DateTime<Utc>,
    pub snippet: Option<String>,
    pub is_read: bool,
    pub task_id: Option<i64>,
    pub body_text: Option<String>,
    /// Comma-separated `To` addresses, for reply-all and forward-prefill.
    pub to_addrs: Option<String>,
    /// Comma-separated `Cc` addresses, for reply-all.
    pub cc_addrs: Option<String>,
    /// This message's raw `References` header, space-joined, if it had one. A reply chains onto
    /// it by appending this message's own `message_id` (RFC 5322 §3.6.4); done by the composer,
    /// not here, since `parse_raw` only extracts what's already on the wire.
    pub references_header: Option<String>,
    pub is_starred: bool,
    /// `None` (untagged) or one of `app::mail::CATEGORY_ORDER`'s colour names — Outlook's
    /// colored-category tag, one per message. Set by `App::cycle_selected_category` (Slice 22).
    pub category: Option<String>,
    /// `EXISTS(...)` over `email_attachments`, computed by `store::get_recent`'s query rather than
    /// stored — always in sync with the real rows, no separate write path to forget.
    pub has_attachments: bool,
    /// `Some(t)` while `t` is still in the future hides this message from the normal list
    /// (`App::refresh_emails`'s snooze-visibility filter); once `t` passes it reappears on its
    /// own, no separate clear needed. Set by `App::commit_snooze`, cleared early by
    /// `App::unsnooze_selected_email`.
    pub snoozed_until: Option<DateTime<Utc>>,
    /// `None` until `App::run_email_triage` classifies it; `Some(true)` is Focused (personal/work
    /// mail worth attention), `Some(false)` is Other (bulk/automated) — Outlook's Focused Inbox
    /// split. Set by `email::store::set_triage`.
    pub triage_focused: Option<bool>,
    /// `Some` when this message carried a `text/calendar` `VEVENT` part (Slice 19) — a meeting
    /// invite. Populated once at parse time (`extract_meeting_invite`), never re-derived, so a
    /// malformed/unparseable ICS part just leaves these `None` rather than erroring the sync.
    pub meeting_title: Option<String>,
    pub meeting_start: Option<DateTime<Utc>>,
    pub meeting_end: Option<DateTime<Utc>>,
    pub meeting_location: Option<String>,
}

/// One MIME attachment's metadata, mirrors the `email_attachments` table. Bytes are never stored
/// here or in the DB — see `MailSource::fetch_attachments`.
#[derive(Debug, Clone, FromRow)]
pub struct EmailAttachment {
    pub id: i64,
    pub email_id: i64,
    /// Position among the message's MIME parts, in `mail_parser::Message::attachments()` order —
    /// the same order `MailSource::fetch_attachments` re-parses the message in, so this index
    /// always lines up with a re-fetch even though the bytes themselves aren't persisted.
    pub part_index: i64,
    pub filename: Option<String>,
    pub content_type: String,
    pub size_bytes: i64,
}

/// A user-defined auto-action, mirrors the `email_rules` table (Slice 18).
///
/// `match_field` is `"subject"` or `"from_addr"`, `action` is `"star"`, `"read"`, `"archive"` or
/// `"delete"` (the latter two added in Slice 21) — see `app::mail::match_rule` for how `pattern`
/// is tested and `app::mail::apply_rule_action` for how each action is carried out.
#[derive(Debug, Clone, FromRow)]
pub struct EmailRule {
    pub id: i64,
    pub match_field: String,
    pub pattern: String,
    pub action: String,
    pub created_at: DateTime<Utc>,
}

/// One attachment's metadata extracted from a raw message, ready to insert (no `id` yet).
#[derive(Debug, Clone)]
pub struct NewAttachment {
    pub part_index: i64,
    pub filename: Option<String>,
    pub content_type: String,
    pub size_bytes: i64,
}

/// Fields extracted from a raw RFC822 message, ready to insert (no `id` yet).
#[derive(Debug, Clone)]
pub struct NewEmail {
    pub uid: i64,
    pub message_id: String,
    pub account: String,
    pub folder: String,
    pub from_addr: String,
    pub from_name: Option<String>,
    pub subject: String,
    pub date_utc: DateTime<Utc>,
    pub snippet: Option<String>,
    pub body_text: Option<String>,
    pub to_addrs: Option<String>,
    pub cc_addrs: Option<String>,
    pub references_header: Option<String>,
    pub attachments: Vec<NewAttachment>,
    pub meeting_title: Option<String>,
    pub meeting_start: Option<DateTime<Utc>>,
    pub meeting_end: Option<DateTime<Utc>>,
    pub meeting_location: Option<String>,
}

/// Comma-joins every address in a `To`/`Cc` header, dropping addresses with no
/// address part (group syntax's own name-only entries). `None` if the header was
/// absent or had no usable address.
fn join_addrs(addr: Option<&mail_parser::Address<'_>>) -> Option<String> {
    let joined = addr?
        .iter()
        .filter_map(mail_parser::Addr::address)
        .collect::<Vec<_>>()
        .join(", ");

    (!joined.is_empty()).then_some(joined)
}

/// Parse a raw RFC822 message fetched over IMAP into a `NewEmail`.
///
/// `header_only` marks a message whose body was skipped during fetch (see `client.rs`'s
/// `LARGE_MESSAGE_BYTES`) — `raw` is headers only, so `snippet`/`body_text` are synthesized instead
/// of extracted.
///
/// # Errors
///
/// Returns an error if `raw` is not a parseable RFC822 message.
pub fn parse_raw(
    account: &str,
    uid: u32,
    folder: &str,
    raw: &[u8],
    header_only: bool,
) -> Result<NewEmail> {
    let message = MessageParser::default()
        .parse(raw)
        .context("failed to parse RFC822 message")?;

    let message_id = message.message_id().map_or_else(
        || format!("<generated-{folder}-{uid}@triptych>"),
        str::to_string,
    );

    let from = message.from().and_then(|addr| addr.first());
    let from_addr = from
        .and_then(|a| a.address())
        .map_or_else(|| "unknown@unknown".to_string(), str::to_string);
    let from_name = from.and_then(|a| a.name()).map(str::to_string);

    let subject = message.subject().unwrap_or("(no subject)").to_string();

    let to_addrs = join_addrs(message.to());
    let cc_addrs = join_addrs(message.cc());
    let references_header = message
        .references()
        .as_text_list()
        .filter(|refs| !refs.is_empty())
        .map(|refs| refs.join(" "));

    let date_utc = message.date().map_or_else(Utc::now, |d| {
        Utc.timestamp_opt(d.to_timestamp(), 0)
            .single()
            .unwrap_or_else(Utc::now)
    });

    let (snippet, body_text) = if header_only {
        (
            Some("[message too large to sync — body not fetched]".to_string()),
            None,
        )
    } else {
        (
            message.body_preview(200).map(|s| clean_snippet(&s)),
            message.body_text(0).map(|s| strip_hidden_chars(&s)),
        )
    };

    // A `header_only` fetch (see `client.rs`'s `LARGE_MESSAGE_BYTES`) has no body, so the MIME
    // part structure genuinely can't be known — an empty list here, not a guess.
    let (attachments, invite) = if header_only {
        (Vec::new(), None)
    } else {
        (
            extract_attachments(&message),
            extract_meeting_invite(&message),
        )
    };

    Ok(NewEmail {
        uid: i64::from(uid),
        message_id,
        account: account.to_string(),
        folder: folder.to_string(),
        from_addr,
        from_name,
        subject,
        date_utc,
        snippet,
        body_text,
        to_addrs,
        cc_addrs,
        references_header,
        attachments,
        meeting_title: invite.as_ref().map(|i| i.title.clone()),
        meeting_start: invite.as_ref().map(|i| i.start),
        meeting_end: invite.as_ref().and_then(|i| i.end),
        meeting_location: invite.and_then(|i| i.location),
    })
}

/// A meeting invite extracted from a `text/calendar` MIME part.
#[derive(Debug, Clone)]
struct MeetingInvite {
    title: String,
    start: DateTime<Utc>,
    end: Option<DateTime<Utc>>,
    location: Option<String>,
}

/// `DatePerhapsTime` -> UTC. A bare `DATE` (all-day event) or a `Floating` `DATE-TIME` (no `Z`
/// suffix, no `TZID`) has no real timezone to resolve against — real Google/Outlook/Apple invites
/// always emit `Utc` or a `TZID`-qualified time in practice, so treating either as UTC directly is
/// an acceptable best-effort simplification for a first pass, not a guess relied on elsewhere.
fn date_perhaps_time_to_utc(dpt: &DatePerhapsTime) -> Option<DateTime<Utc>> {
    match dpt {
        DatePerhapsTime::DateTime(cdt) => match cdt {
            CalendarDateTime::Floating(naive) => Some(naive.and_utc()),
            _ => cdt.try_into_utc(),
        },
        DatePerhapsTime::Date(date) => date.and_hms_opt(0, 0, 0).map(|naive| naive.and_utc()),
    }
}

/// Finds the first `VEVENT` in a `text/calendar` MIME part (sent as an attachment either way,
/// whether or not it declares `Content-Disposition: attachment` — `mail-parser` classifies any
/// non-plain/html text part that way) and extracts the fields a meeting invite needs to become a
/// task. `None` if the message has no calendar part, the part doesn't parse as an ICS calendar, or
/// the event has no usable `DTSTART`.
fn extract_meeting_invite(message: &mail_parser::Message<'_>) -> Option<MeetingInvite> {
    let ics_part = message.attachments().find(|part| {
        part.content_type().is_some_and(|ct| {
            ct.c_type.eq_ignore_ascii_case("text")
                && ct
                    .c_subtype
                    .as_deref()
                    .is_some_and(|sub| sub.eq_ignore_ascii_case("calendar"))
        })
    })?;
    let text = match &ics_part.body {
        mail_parser::PartType::Text(t) => t.to_string(),
        mail_parser::PartType::Binary(b) | mail_parser::PartType::InlineBinary(b) => {
            String::from_utf8_lossy(b).into_owned()
        }
        _ => return None,
    };

    let calendar = Calendar::from_str(&text).ok()?;
    let event = calendar.events().next()?;
    let start = date_perhaps_time_to_utc(&event.get_start()?)?;
    let end = event
        .get_end()
        .and_then(|dpt| date_perhaps_time_to_utc(&dpt));

    Some(MeetingInvite {
        title: event.get_summary().unwrap_or("(no title)").to_string(),
        start,
        end,
        location: event.get_location().map(str::to_string),
    })
}

/// Reads every attachment's filename/content-type/size off an already-parsed message. Never
/// touches an attachment's bytes — those are fetched on demand later (`MailSource::fetch_attachments`),
/// not persisted.
fn extract_attachments(message: &mail_parser::Message<'_>) -> Vec<NewAttachment> {
    message
        .attachments()
        .enumerate()
        .map(|(index, part)| {
            let content_type = part.content_type().map_or_else(
                || "application/octet-stream".to_string(),
                |ct| {
                    ct.c_subtype.as_ref().map_or_else(
                        || ct.c_type.to_string(),
                        |sub| format!("{}/{sub}", ct.c_type),
                    )
                },
            );
            NewAttachment {
                part_index: i64::try_from(index).unwrap_or(i64::MAX),
                filename: part.attachment_name().map(str::to_string),
                content_type,
                size_bytes: i64::try_from(part.len()).unwrap_or(i64::MAX),
            }
        })
        .collect()
}

/// Marketers pad hidden preheader/body text with zero-width characters to
/// control inbox preview length; `html_to_text` isn't CSS-aware so this junk
/// survives straight into the extracted text. Strip it.
fn strip_hidden_chars(s: &str) -> String {
    s.chars()
        .filter(|c| {
            !matches!(
                c,
                '\u{200B}' // zero width space
                | '\u{200C}' // zero width non-joiner
                | '\u{200D}' // zero width joiner
                | '\u{200E}' // left-to-right mark
                | '\u{200F}' // right-to-left mark
                | '\u{FEFF}' // BOM / zero width no-break space
                | '\u{2060}' // word joiner
                | '\u{00AD}' // soft hyphen
            )
        })
        .collect()
}

/// Snippet also collapses all whitespace to single spaces, since it's a
/// one-line preview (unlike the full body, which keeps its line breaks).
#[must_use]
pub fn clean_snippet(s: &str) -> String {
    strip_hidden_chars(s)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// How much of a cleaned body `nlp::rules::extract_deadline_only` bothers scanning; matches
/// `App`'s `SUMMARY_INPUT_CHARS` cap for the same reason (a real deadline phrase is always near
/// the top of a message, never worth paying to scan megabytes of quoted history for).
const DEADLINE_SCAN_CHARS: usize = 4000;

/// Prepares an email body for a narrow deadline scan (see `nlp::rules::extract_deadline_only`).
///
/// Stops at the first quoted-reply chain, forwarded-message header or RFC 3676 `--` signature
/// delimiter, and drops any line that looks like a legal/marketing footer. A bare "due"/"by" phrase
/// surviving in a quoted older message or a footer is far likelier to be boilerplate than the
/// sender's real deadline, so this runs before any date extraction rather than after.
#[must_use]
pub fn clean_body_for_deadline_scan(body: &str) -> String {
    let mut kept = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed == "--" || is_quote_boundary(trimmed) || is_forward_header(trimmed) {
            break;
        }
        if is_boilerplate_line(trimmed) {
            break;
        }
        if !trimmed.starts_with('>') {
            kept.push(line);
        }
    }
    kept.join("\n").chars().take(DEADLINE_SCAN_CHARS).collect()
}

/// "On Mon, Jan 1, 2026 at 3:00 PM Jane Doe <jane@example.com> wrote:" (Gmail/Apple/Thunderbird)
/// or an Outlook-style "-----Original Message-----" separator.
fn is_quote_boundary(line: &str) -> bool {
    let lower = line.to_lowercase();
    (lower.starts_with("on ") && lower.ends_with("wrote:"))
        || line.starts_with("-----Original Message-----")
}

/// The first line or two of an Outlook-style forwarded-message header block.
fn is_forward_header(line: &str) -> bool {
    let lower = line.to_lowercase();
    lower.starts_with("from:")
        || lower.starts_with("sent:")
        || lower.starts_with("forwarded message")
}

fn is_boilerplate_line(line: &str) -> bool {
    let lower = line.to_lowercase();
    [
        "unsubscribe",
        "privacy policy",
        "terms of service",
        "all rights reserved",
        "view this email in your browser",
    ]
    .iter()
    .any(|kw| lower.contains(kw))
}
