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
    pub folder: String,
    pub from_addr: String,
    pub from_name: Option<String>,
    pub subject: String,
    pub date_utc: DateTime<Utc>,
    pub snippet: Option<String>,
}

/// Parse a raw RFC822 message fetched over IMAP into a `NewEmail`.
pub fn parse_raw(uid: u32, folder: &str, raw: &[u8]) -> Result<NewEmail> {
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

    let snippet = message.body_preview(200).map(|s| s.to_string());

    Ok(NewEmail {
        uid: uid as i64,
        message_id,
        folder: folder.to_string(),
        from_addr,
        from_name,
        subject,
        date_utc,
        snippet,
    })
}
