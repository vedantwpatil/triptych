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
}

/// Parse a raw RFC822 message fetched over IMAP into a `NewEmail`.
pub fn parse_raw(account: &str, uid: u32, folder: &str, raw: &[u8]) -> Result<NewEmail> {
    let message = MessageParser::default()
        .parse(raw)
        .context("failed to parse RFC822 message")?;

    let message_id = message
        .message_id()
        .map(str::to_string)
        .unwrap_or_else(|| format!("<generated-{}-{}@triptych>", folder, uid));

    let from = message.from().and_then(|addr| addr.first());
    let from_addr = from
        .and_then(|a| a.address())
        .map(str::to_string)
        .unwrap_or_else(|| "unknown@unknown".to_string());
    let from_name = from.and_then(|a| a.name()).map(str::to_string);

    let subject = message.subject().unwrap_or("(no subject)").to_string();

    let date_utc = message
        .date()
        .map(|d| Utc.timestamp_opt(d.to_timestamp(), 0).single().unwrap_or_else(Utc::now))
        .unwrap_or_else(Utc::now);

    let snippet = message.body_preview(200).map(|s| clean_snippet(&s));

    Ok(NewEmail {
        uid: uid as i64,
        message_id,
        account: account.to_string(),
        folder: folder.to_string(),
        from_addr,
        from_name,
        subject,
        date_utc,
        snippet,
    })
}

/// Marketers pad hidden preheader text with zero-width characters to control
/// inbox preview length; `html_to_text` isn't CSS-aware so this junk survives
/// straight into the snippet. Strip it and collapse the resulting whitespace.
fn clean_snippet(s: &str) -> String {
    let cleaned: String = s
        .chars()
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
        .collect();

    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::clean_snippet;

    #[test]
    fn strips_preheader_padding() {
        let padded = "PNC Financial Services Group is hiring\u{A0}\u{200C}\u{200D}\u{200E}\u{200F}\u{FEFF}\u{A0}\u{200C}\u{200D}\u{200E}\u{200F}\u{FEFF}";
        assert_eq!(clean_snippet(padded), "PNC Financial Services Group is hiring");
    }
}
