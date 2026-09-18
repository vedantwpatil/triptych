use anyhow::Result;
use sqlx::SqlitePool;
use tokio::sync::broadcast;
use tokio::time::{Duration, interval};

use crate::email::{EmailConfig, ImapMailSource, MailSource};
use crate::email::{message, store};

/// Polls every configured IMAP account for new mail every 60s, one account after
/// another within each tick. Not true IMAP IDLE (push) — that's a longer-lived-
/// connection concern, deferred to a later slice.
pub async fn mail_sync_worker(db: SqlitePool, mut shutdown_rx: broadcast::Receiver<()>) -> Result<()> {
    let configs = EmailConfig::all_from_env();
    if configs.is_empty() {
        eprintln!("[Mail] no IMAP accounts configured (TRIPTYCH_EMAIL_ENABLED/IMAP_*), mail sync disabled");
        return Ok(());
    }

    let sources: Vec<(EmailConfig, ImapMailSource)> = configs
        .into_iter()
        .map(|config| (config.clone(), ImapMailSource::new(config)))
        .collect();
    let mut sync_interval = interval(Duration::from_secs(60));

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                break;
            }

            _ = sync_interval.tick() => {
                for (config, source) in &sources {
                    if let Err(e) = sync_mail(&db, source, config).await {
                        eprintln!("[Mail] sync failed for account '{}': {}", config.account, e);
                    }
                }
            }
        }
    }

    Ok(())
}

async fn sync_mail(db: &SqlitePool, source: &ImapMailSource, config: &EmailConfig) -> Result<()> {
    let last_uid = store::max_uid(db, &config.account, &config.imap_folder).await?;

    let raw_messages = source.fetch_new(last_uid.map(|uid| uid as u32)).await?;

    let new_emails: Vec<_> = raw_messages
        .into_iter()
        .filter_map(|(uid, raw)| {
            message::parse_raw(&config.account, uid, &config.imap_folder, &raw).ok()
        })
        .collect();

    if !new_emails.is_empty() {
        store::insert_new(db, &new_emails).await?;
    }

    Ok(())
}
