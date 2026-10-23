//! Job-digest triage: stored digest mail -> postings -> rule score -> an `Apply:` task per good fit.
//!
//! `job_postings.key` is both the dedupe and the tombstone: a posting is scored once, and a task the
//! user deletes is never recreated. See [`CLAUDE.md`](./jobs/CLAUDE.md).

use std::{cmp::Reverse, collections::HashSet};

use chrono::{DateTime, Duration, Utc};
use sqlx::SqlitePool;

use crate::app::{NewTask, insert_task_row};
use crate::email::links;

pub mod digest;
pub mod score;

use digest::parse_digest;
pub use score::Profile;

/// Scans recent digests for postings not seen before, stores every one, and makes a task for each
/// good fit. Returns `Company - Role` for the tasks created.
///
/// # Errors
///
/// Returns an error if a query fails; postings stored before the failure stay stored.
pub async fn scan(
    pool: &SqlitePool,
    profile: &Profile,
    now: DateTime<Utc>,
) -> anyhow::Result<Vec<String>> {
    let cutoff = now - Duration::days(profile.max_age_days);
    let mut known: HashSet<String> = sqlx::query_scalar("SELECT key FROM job_postings")
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect();
    let mut created = Vec::new();

    for sender in &profile.senders {
        let digests: Vec<(i64, String, Option<String>)> = sqlx::query_as(
            "SELECT id, body_text, links FROM email_messages \
             WHERE LOWER(from_addr) = LOWER(?) AND date_utc >= ? AND body_text IS NOT NULL \
             ORDER BY date_utc ASC",
        )
        .bind(sender)
        .bind(cutoff)
        .fetch_all(pool)
        .await?;

        for (email_id, body, stored_links) in digests {
            let anchors = links::from_json(stored_links.as_deref());
            let mut fresh: Vec<_> = parse_digest(&body, &anchors)
                .into_iter()
                .filter(|p| known.insert(p.key()))
                .map(|p| {
                    let verdict = score::score(&p, profile);
                    (p, verdict)
                })
                .collect();
            // Best first, so the cap keeps the strongest; the sort is stable, so ties stay in body order.
            fresh.sort_by_key(|(_, v)| Reverse(v.score));

            let mut made = 0;
            for (posting, verdict) in fresh {
                let fit = verdict.shortlisted(profile);
                let go = fit && made < profile.max_per_digest;
                let reason = if fit && !go {
                    format!("{} (over cap)", verdict.reason)
                } else {
                    verdict.reason
                };
                let inserted = sqlx::query(
                    "INSERT OR IGNORE INTO job_postings \
                     (email_id, key, company, role, url, score, reason, status) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(email_id)
                .bind(posting.key())
                .bind(&posting.company)
                .bind(&posting.role)
                .bind(&posting.url)
                .bind(verdict.score)
                .bind(reason)
                .bind(if go { "new" } else { "skipped" })
                .execute(pool)
                .await?
                .rows_affected()
                    == 1;
                if !(go && inserted) {
                    continue;
                }

                made += 1;
                let label = format!("{} - {}", posting.company, posting.role);
                let title = format!("Apply: {label}");
                let task_links = posting
                    .url
                    .as_ref()
                    .and_then(|url| serde_json::to_string(&[url]).ok());
                let task_id = insert_task_row(
                    pool,
                    &NewTask {
                        title: &title,
                        input: &title,
                        item_order: None,
                        priority: 1,
                        tags: &[],
                        scheduled_at: None,
                        deadline: None,
                        duration_minutes: None,
                        recurrence: None,
                        links: task_links.as_deref(),
                    },
                )
                .await?;
                sqlx::query("UPDATE job_postings SET task_id = ? WHERE key = ?")
                    .bind(task_id)
                    .bind(posting.key())
                    .execute(pool)
                    .await?;
                created.push(label);
            }
        }
    }
    Ok(created)
}

/// One stored posting: score, company, role, status, reason.
pub type Row = (i32, String, String, String, String);

/// Stored postings, best first; skipped ones only with `all`.
///
/// # Errors
///
/// Returns an error if the query fails.
pub async fn list(pool: &SqlitePool, all: bool) -> sqlx::Result<Vec<Row>> {
    sqlx::query_as(
        "SELECT score, company, role, status, reason FROM job_postings \
         WHERE ? OR status != 'skipped' ORDER BY score DESC, id DESC",
    )
    .bind(all)
    .fetch_all(pool)
    .await
}
