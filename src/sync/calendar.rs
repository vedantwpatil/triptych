use anyhow::Result;
use sqlx::SqlitePool;
use tokio::sync::broadcast;
use tokio::time::{Duration, interval};

/// Background calendar sync worker for CalDAV integration
pub async fn calendar_sync_worker(
    db: SqlitePool,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> Result<()> {
    let mut sync_interval = interval(Duration::from_secs(600));

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                break;
            }

            _ = sync_interval.tick() => {
                let _ = sync_calendar(&db).await;
            }
        }
    }

    Ok(())
}

/// Sync calendar events from CalDAV server
/// TODO: Implement CalDAV protocol
async fn sync_calendar(db: &SqlitePool) -> Result<()> {
    let _count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM events")
        .fetch_one(db)
        .await?;

    Ok(())
}
