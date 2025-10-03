use anyhow::Result;
use sqlx::SqlitePool;
use tokio::sync::broadcast;
use tokio::time::{Duration, interval};

/// Background calendar sync worker for CalDAV integration
pub async fn calendar_sync_worker(
    db: SqlitePool,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> Result<()> {
    eprintln!("[Sync] Starting calendar sync worker");

    let mut sync_interval = interval(Duration::from_secs(600));

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                eprintln!("[Sync] Calendar sync worker shutting down");
                break;
            }

            _ = sync_interval.tick() => {
                if let Err(e) = sync_calendar(&db).await {
                    eprintln!("[Sync] Calendar sync error: {}", e);
                }
            }
        }
    }

    Ok(())
}

/// Sync calendar events from CalDAV server
/// TODO: Implement CalDAV protocol
async fn sync_calendar(db: &SqlitePool) -> Result<()> {
    eprintln!("[Sync] Syncing calendar events...");

    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM events")
        .fetch_one(db)
        .await?;

    eprintln!("[Sync] Calendar sync complete ({} total events)", count.0);
    Ok(())
}
