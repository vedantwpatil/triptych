use anyhow::Result;
use sqlx::SqlitePool;
use tokio::sync::broadcast;
use tokio::time::{Duration, interval};

use crate::email::{EmailConfig, ImapMailSource, MailSource};
use crate::email::{message, store};
use crate::email::store::SyncCursor;

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
    let cursor = store::get_sync_cursor(db, &config.account, &config.imap_folder).await?;

    let (uid_validity, raw_messages) = source.fetch_new(cursor).await?;

    let epoch_changed = match (cursor, uid_validity) {
        (Some(c), Some(current)) => c.uid_validity as u32 != current,
        _ => false,
    };
    if epoch_changed {
        eprintln!(
            "[Mail] UIDVALIDITY changed for '{}'; resyncing recent mail instead of resuming",
            config.account
        );
    }

    let fetched_max_uid = raw_messages.iter().map(|(uid, _)| *uid).max();

    let new_emails: Vec<_> = raw_messages
        .into_iter()
        .filter_map(|(uid, raw)| {
            message::parse_raw(&config.account, uid, &config.imap_folder, &raw).ok()
        })
        .collect();

    if !new_emails.is_empty() {
        store::insert_new(db, &new_emails).await?;
    }

    // If the epoch changed and nothing came back this round, don't persist a
    // synthetic `last_uid = 0`: that would search "UID 1:*" next time, which is
    // NOT capped by INITIAL_SYNC_LIMIT the way a `None` cursor's "ALL" search is.
    // Leaving the stale cursor in place instead makes the next sync detect the
    // same epoch change and retry the correctly-capped catch-up.
    if let Some(validity) = uid_validity
        && !(epoch_changed && fetched_max_uid.is_none())
    {
        let prior_uid = if epoch_changed { 0 } else { cursor.map_or(0, |c| c.last_uid) };
        let last_uid = fetched_max_uid.map_or(prior_uid, |uid| (uid as i64).max(prior_uid));
        store::set_sync_cursor(
            db,
            &config.account,
            &config.imap_folder,
            SyncCursor { uid_validity: validity as i64, last_uid },
        )
        .await?;
    }

    Ok(())
}
