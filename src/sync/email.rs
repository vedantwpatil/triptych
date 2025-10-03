use anyhow::Result;
use sqlx::SqlitePool;
use tokio::sync::broadcast;
use tokio::time::{Duration, interval};

/// Background email sync worker using IMAP IDLE for real-time notifications
pub async fn email_sync_worker(
    db: SqlitePool,
    mut shutdown_rx: broadcast::Receiver<()>,
    interval_secs: u64,
) -> Result<()> {
    eprintln!(
        "[Sync] Starting email sync worker (interval: {}s)",
        interval_secs
    );

    let mut sync_interval = interval(Duration::from_secs(interval_secs));

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                eprintln!("[Sync] Email sync worker shutting down");
                break;
            }

            _ = sync_interval.tick() => {
                if let Err(e) = sync_emails(&db).await {
                    eprintln!("[Sync] Email sync error: {}", e);
                }
            }
        }
    }

    Ok(())
}

/// Sync emails from IMAP server
/// TODO: Implement IMAP IDLE protocol
async fn sync_emails(db: &SqlitePool) -> Result<()> {
    eprintln!("[Sync] Syncing emails...");

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM emails")
        .fetch_one(db)
        .await?;

    eprintln!("[Sync] Email sync complete ({} total emails)", count.0);
    Ok(())
}
