//! Desktop alerts for approaching deadlines.
//!
//! [`due_alerts`] is terminal-free and idempotent: each task is alerted once per tier (within 24h,
//! within 1h), remembered in `tasks.notified_tier`, so restarts never repeat an alert. [`send`]
//! shells out; [`check_once`] ties the two together. The TUI worker in `sync/notify.rs` calls it
//! every minute while the TUI is open; [`agent`] installs a `LaunchAgent` that calls it with the TUI
//! closed.

pub mod agent;

use chrono::{DateTime, Duration, Utc};
use sqlx::SqlitePool;

/// Env var: `0`/`false`/`off`/`no` disables alerts.
pub const NOTIFY_ENV: &str = "TRIPTYCH_NOTIFY";
/// Env var: program run as `<cmd> <title> <body>` instead of the platform default.
pub const NOTIFY_CMD_ENV: &str = "TRIPTYCH_NOTIFY_CMD";

/// Longest list of task names in one alert body.
const MAX_LISTED: usize = 3;

/// One notification to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Alert {
    pub title: String,
    pub body: String,
}

/// Whether alerts are on (the default).
#[must_use]
pub fn enabled() -> bool {
    std::env::var(NOTIFY_ENV).map_or(true, |v| {
        !matches!(
            v.trim().to_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        )
    })
}

fn tier_for(left: Duration) -> i64 {
    if left <= Duration::hours(1) { 2 } else { 1 }
}

fn describe(left: Duration) -> String {
    let mins = left.num_minutes().max(1);
    if mins < 60 {
        format!("{mins}m")
    } else {
        format!("{}h {}m", mins / 60, mins % 60)
    }
}

/// Alerts for open tasks whose deadline is within 24h and not yet alerted at that tier, one alert
/// per tier with the tasks batched. Marks them as sent. Overdue tasks are not alerted.
///
/// # Errors
///
/// Returns an error if a database query fails.
pub async fn due_alerts(pool: &SqlitePool, now: DateTime<Utc>) -> Result<Vec<Alert>, sqlx::Error> {
    Ok(claim_alerts(pool, now)
        .await?
        .into_iter()
        .map(|(alert, _)| alert)
        .collect())
}

/// A task whose alert tier was marked as sent: `previous` is what to put back if the alert fails.
struct Claim {
    id: i64,
    previous: i64,
    tier: i64,
}

/// `due_alerts` plus what each alert claimed, so a failed send can be undone.
async fn claim_alerts(
    pool: &SqlitePool,
    now: DateTime<Utc>,
) -> Result<Vec<(Alert, Vec<Claim>)>, sqlx::Error> {
    let rows: Vec<(i64, String, DateTime<Utc>, i64)> = sqlx::query_as(
        "SELECT id, description, deadline, notified_tier FROM tasks \
         WHERE completed = false AND deadline > ? AND deadline <= ? ORDER BY deadline",
    )
    .bind(now)
    .bind(now + Duration::hours(24))
    .fetch_all(pool)
    .await?;

    let mut by_tier: [Vec<(String, Claim)>; 2] = [Vec::new(), Vec::new()];
    for (id, description, deadline, sent) in rows {
        let left = deadline - now;
        let tier = tier_for(left);
        if sent >= tier {
            continue;
        }
        // Atomic claim: the TUI worker and the LaunchAgent may run at once, and only the one whose
        // UPDATE changes the row may alert.
        let claimed =
            sqlx::query("UPDATE tasks SET notified_tier = ? WHERE id = ? AND notified_tier < ?")
                .bind(tier)
                .bind(id)
                .bind(tier)
                .execute(pool)
                .await?;
        if claimed.rows_affected() == 0 {
            continue;
        }
        by_tier[usize::try_from(tier - 1).unwrap_or(0)].push((
            format!("{description} (in {})", describe(left)),
            Claim {
                id,
                previous: sent,
                tier,
            },
        ));
    }

    let titles = ["Due within 24 hours", "Due within 1 hour"];
    Ok(by_tier
        .into_iter()
        .zip(titles)
        .rev()
        .filter(|(items, _)| !items.is_empty())
        .map(|(items, title)| {
            let mut lines: Vec<String> = items
                .iter()
                .take(MAX_LISTED)
                .map(|(line, _)| line.clone())
                .collect();
            if items.len() > MAX_LISTED {
                lines.push(format!("+{} more", items.len() - MAX_LISTED));
            }
            let alert = Alert {
                title: title.to_string(),
                body: lines.join("\n"),
            };
            (alert, items.into_iter().map(|(_, claim)| claim).collect())
        })
        .collect())
}

/// What one [`check_once`] pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CheckReport {
    pub shown: usize,
    /// Alerts the notifier failed to show; they are retried on the next pass.
    pub failed: usize,
}

/// One pass: the alerts due at `now`, each shown.
///
/// An alert whose notifier fails is handed back, so the next pass retries it instead of losing it.
/// Shared by the TUI worker and `triptych notify check`.
///
/// # Errors
///
/// Returns an error if a database query fails.
pub async fn check_once(pool: &SqlitePool, now: DateTime<Utc>) -> Result<CheckReport, sqlx::Error> {
    let mut report = CheckReport::default();
    for (alert, claims) in claim_alerts(pool, now).await? {
        if send(&alert).await {
            report.shown += 1;
            continue;
        }
        report.failed += 1;
        for claim in claims {
            sqlx::query("UPDATE tasks SET notified_tier = ? WHERE id = ? AND notified_tier = ?")
                .bind(claim.previous)
                .bind(claim.id)
                .bind(claim.tier)
                .execute(pool)
                .await?;
        }
    }
    Ok(report)
}

/// Shows `alert` and says whether it worked. A failure is logged, never raised: an alert must not
/// take the worker down.
pub async fn send(alert: &Alert) -> bool {
    let mut cmd = match std::env::var(NOTIFY_CMD_ENV)
        .ok()
        .filter(|c| !c.trim().is_empty())
    {
        Some(program) => {
            let mut c = tokio::process::Command::new(program);
            c.arg(&alert.title).arg(&alert.body);
            c
        }
        None if cfg!(target_os = "macos") => {
            let mut c = tokio::process::Command::new("osascript");
            c.args([
                "-e",
                "on run argv",
                "-e",
                "display notification (item 2 of argv) with title (item 1 of argv)",
                "-e",
                "end run",
            ])
            .arg(&alert.title)
            .arg(&alert.body);
            c
        }
        None => {
            let mut c = tokio::process::Command::new("notify-send");
            c.arg(&alert.title).arg(&alert.body);
            c
        }
    };
    match cmd.kill_on_drop(true).status().await {
        Ok(status) if status.success() => return true,
        Ok(status) => tracing::warn!("notification command exited with {status}"),
        Err(e) => tracing::warn!("notification command failed: {e}"),
    }
    false
}
