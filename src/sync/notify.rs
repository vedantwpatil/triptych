use anyhow::Result;
use sqlx::SqlitePool;
use tokio::sync::broadcast;
use tokio::time::{Duration, MissedTickBehavior, interval};

use crate::notify;

const POLL_SECS: u64 = 60;

/// Checks for approaching deadlines once at startup, then every minute, and sends each alert.
pub async fn notify_worker(db: SqlitePool, mut shutdown_rx: broadcast::Receiver<()>) -> Result<()> {
    let mut poll = interval(Duration::from_secs(POLL_SECS));
    poll.set_missed_tick_behavior(MissedTickBehavior::Delay);

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => break,

            _ = poll.tick() => match notify::due_alerts(&db, chrono::Utc::now()).await {
                Ok(alerts) => {
                    for alert in &alerts {
                        notify::send(alert).await;
                    }
                }
                Err(e) => tracing::warn!("deadline alerts failed: {e}"),
            },
        }
    }

    Ok(())
}
