use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use mail_parser::MessageParser;
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
}

/// Parse a raw RFC822 message fetched over IMAP into a `NewEmail`. `header_only`
/// marks a message whose body was skipped during fetch (see `client.rs`'s
/// `LARGE_MESSAGE_BYTES`) — `raw` is headers only, so `snippet`/`body_text` are
/// synthesized instead of extracted.
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

    let message_id = message
        .message_id().map_or_else(|| format!("<generated-{folder}-{uid}@triptych>"), str::to_string);

    let from = message.from().and_then(|addr| addr.first());
    let from_addr = from
        .and_then(|a| a.address()).map_or_else(|| "unknown@unknown".to_string(), str::to_string);
    let from_name = from.and_then(|a| a.name()).map(str::to_string);

    let subject = message.subject().unwrap_or("(no subject)").to_string();

    let date_utc = message
        .date().map_or_else(Utc::now, |d| Utc.timestamp_opt(d.to_timestamp(), 0).single().unwrap_or_else(Utc::now));

    let (snippet, body_text) = if header_only {
        (Some("[message too large to sync — body not fetched]".to_string()), None)
    } else {
        (
            message.body_preview(200).map(|s| clean_snippet(&s)),
            message.body_text(0).map(|s| strip_hidden_chars(&s)),
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
    })
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
fn clean_snippet(s: &str) -> String {
    strip_hidden_chars(s).split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::clean_snippet;

    #[test]
    fn strips_preheader_padding() {
        let padded = "PNC Financial Services Group is hiring\u{A0}\u{200C}\u{200D}\u{200E}\u{200F}\u{FEFF}\u{A0}\u{200C}\u{200D}\u{200E}\u{200F}\u{FEFF}";
        assert_eq!(clean_snippet(padded), "PNC Financial Services Group is hiring");
    }
}
