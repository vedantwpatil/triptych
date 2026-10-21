//! Cheap "did the database change?" probe for the TUI's periodic reload.

use sqlx::{Sqlite, SqlitePool, pool::PoolConnection};
use std::time::{Duration, Instant};

/// Reload at least this often even when nothing was written: snooze expiry and urgency tiers
/// follow the clock, not a commit.
const FLOOR: Duration = Duration::from_secs(60);

/// Holds one pooled connection for its whole life.
///
/// SQLite's `PRAGMA data_version` changes on a connection only when a *different* connection
/// commits, so the held one never bumps its own counter, while every pool or other-process write
/// does (~10µs to read, vs ~10-30ms to reload).
#[derive(Debug)]
pub struct DbWatch {
    conn: PoolConnection<Sqlite>,
    seen: i64,
    last: Instant,
}

impl DbWatch {
    /// # Errors
    ///
    /// Returns an error if no connection can be acquired or the probe fails.
    pub async fn new(pool: &SqlitePool) -> Result<Self, sqlx::Error> {
        let mut conn = pool.acquire().await?;
        let seen = version(&mut conn).await?;
        Ok(Self {
            conn,
            seen,
            last: Instant::now(),
        })
    }

    /// True when another connection committed since the last call or `FLOOR` has passed. A failed
    /// probe counts as changed, so an error costs a reload, never a stale screen.
    pub async fn changed(&mut self) -> bool {
        let now = version(&mut self.conn).await.ok();
        if now == Some(self.seen) && self.last.elapsed() < FLOOR {
            return false;
        }
        if let Some(v) = now {
            self.seen = v;
        }
        self.last = Instant::now();
        true
    }
}

async fn version(conn: &mut PoolConnection<Sqlite>) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("PRAGMA data_version")
        .fetch_one(&mut **conn)
        .await
}
