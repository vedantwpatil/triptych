use anyhow::Result;
use sqlx::SqlitePool;
use tokio::sync::broadcast;
use tokio::time::{Duration, MissedTickBehavior, interval};

use crate::canvas;

const POLL_SECS: u64 = 900;

/// Polls the Canvas feed: once at startup, then every 15 minutes. A failed poll is logged (the
/// message never carries the feed URL) and retried on the next tick.
pub async fn canvas_sync_worker(
    db: SqlitePool,
    url: String,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> Result<()> {
    let mut poll = interval(Duration::from_secs(POLL_SECS));
    poll.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => break,

            _ = poll.tick() => match canvas::sync(&db, &url).await {
                Ok(r) => tracing::info!("canvas sync: {} added, {} updated", r.added, r.updated),
                Err(e) => tracing::warn!("canvas sync failed: {e}"),
            },
        }
    }

    Ok(())
}
