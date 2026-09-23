use anyhow::Result;
use sqlx::SqlitePool;
use tokio::sync::broadcast;
use tokio::time::{Duration, interval};

use crate::email::{EmailConfig, sync::sync_account};

/// Polls every configured IMAP account for new mail every 60s, one account after
/// another within each tick. Not true IMAP IDLE (push) — that's a longer-lived-
/// connection concern, deferred to a later slice.
pub async fn mail_sync_worker(
    db: SqlitePool,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> Result<()> {
    let configs = EmailConfig::all_from_env();
    if configs.is_empty() {
        tracing::info!(
            "[Mail] no IMAP accounts configured (TRIPTYCH_EMAIL_ENABLED/IMAP_*), mail sync disabled"
        );
        return Ok(());
    }

    let mut sync_interval = interval(Duration::from_secs(60));

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                break;
            }

            _ = sync_interval.tick() => {
                for config in &configs {
                    if let Err(e) = sync_account(&db, config).await {
                        tracing::warn!("[Mail] sync failed for account '{}': {}", config.account, e);
                    }
                }
            }
        }
    }

    Ok(())
}
