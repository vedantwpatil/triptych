use anyhow::Result;
use sqlx::SqlitePool;
use tokio::sync::broadcast;
use tokio::time::{Duration, interval};

use crate::email::{EmailConfig, ImapMailSource, MailSource};
use crate::email::{message, store};

/// Polls a single IMAP account for new mail every 60s. Not true IMAP IDLE (push) —
/// that's a longer-lived-connection concern, deferred to a later slice.
pub async fn mail_sync_worker(db: SqlitePool, mut shutdown_rx: broadcast::Receiver<()>) -> Result<()> {
    let Some(config) = EmailConfig::from_env() else {
        eprintln!("[Mail] TRIPTYCH_EMAIL_ENABLED not set, mail sync disabled");
        return Ok(());
    };

    let source = ImapMailSource::new(config.clone());
    let mut sync_interval = interval(Duration::from_secs(60));

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                break;
            }

            _ = sync_interval.tick() => {
                if let Err(e) = sync_mail(&db, &source, &config).await {
                    eprintln!("[Mail] sync failed: {}", e);
                }
            }
        }
    }

    Ok(())
}

async fn sync_mail(db: &SqlitePool, source: &ImapMailSource, config: &EmailConfig) -> Result<()> {
    let last_uid: Option<i64> = sqlx::query_scalar(
        "SELECT MAX(uid) FROM email_messages WHERE folder = ?",
    )
    .bind(&config.imap_folder)
    .fetch_one(db)
    .await?;

    let raw_messages = source.fetch_new(last_uid.map(|uid| uid as u32)).await?;

    let new_emails: Vec<_> = raw_messages
        .into_iter()
        .filter_map(|(uid, raw)| message::parse_raw(uid, &config.imap_folder, &raw).ok())
        .collect();

    if !new_emails.is_empty() {
        store::insert_new(db, &new_emails).await?;
    }

    Ok(())
}
