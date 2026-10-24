//! Job-digest triage: stored digest mail -> postings -> rule score -> an `Apply:` task per good fit.
//!
//! `job_postings.key` is both the dedupe and the tombstone: a posting is scored once, and a task the
//! user deletes is never recreated. See [`CLAUDE.md`](./jobs/CLAUDE.md).

use std::{cmp::Reverse, collections::HashSet};

use chrono::{DateTime, Duration, Utc};
use sqlx::SqlitePool;

use crate::app::{NewTask, insert_task_row};
use crate::email::links;
use crate::nlp::NLPParser;

pub mod digest;
pub mod score;

use digest::{Posting, parse_digest};
pub use score::Profile;
use score::Verdict;

/// Most model ratings one scan asks for; later postings keep their rule score.
const LLM_CAP: usize = 30;

/// The model's part in one scan: the client (gone after its first failure), what is left of the
/// budget, and the candidate words the prompt carries.
struct Rater<'a> {
    nlp: Option<&'a NLPParser>,
    budget: usize,
    likes: String,
    dislikes: String,
}

impl Rater<'_> {
    /// Rates the postings the rules cannot decide, best effort. Ollama being down must not stop the
    /// scan, so the first failure ends the model's part and the rest keep their rule score.
    async fn rate(&mut self, fresh: &mut [(Posting, Verdict)]) {
        for (posting, verdict) in fresh.iter_mut().filter(|(_, v)| v.needs_llm()) {
            let Some(model) = self.nlp else { return };
            if self.budget == 0 {
                return;
            }
            self.budget -= 1;
            match model
                .job_fit(&posting.company, &posting.role, &self.likes, &self.dislikes)
                .await
            {
                Ok(fit) => verdict.apply_fit(&fit),
                Err(e) => {
                    // shortcut: no retry, so a posting stored unrated keeps its rule score for good
                    tracing::warn!("[Jobs] model rating failed, rules only from here: {e}");
                    self.nlp = None;
                }
            }
        }
    }
}

/// Scans recent digests for postings not seen before, stores every one, and makes a task for each
/// good fit. Returns `Company - Role` for the tasks created.
///
/// Postings the rules cannot decide ([`score::LLM_BAND`]) are rated by `nlp` when given. The first
/// failed rating (Ollama down) turns the model off for the rest of the scan.
///
/// # Errors
///
/// Returns an error if a query fails; postings stored before the failure stay stored.
pub async fn scan(
    pool: &SqlitePool,
    profile: &Profile,
    now: DateTime<Utc>,
    nlp: Option<&NLPParser>,
) -> anyhow::Result<Vec<String>> {
    let mut rater = Rater {
        nlp,
        budget: LLM_CAP,
        likes: profile.up.join(", "),
        dislikes: profile.down.join(", "),
    };
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
            let mut fresh: Vec<_> = unseen(pool, &mut known, parse_digest(&body, &anchors))
                .await?
                .into_iter()
                .map(|p| {
                    let verdict = score::score(&p, profile);
                    (p, verdict)
                })
                .collect();
            rater.rate(&mut fresh).await;
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

/// The postings whose key is new, now remembered in `known`. A posting stored without a page (its
/// digest's links were missing or cut off) gets the page found for it now, and so does its task if
/// that still has no links. Both fill once and never change.
async fn unseen(
    pool: &SqlitePool,
    known: &mut HashSet<String>,
    postings: Vec<Posting>,
) -> sqlx::Result<Vec<Posting>> {
    let (fresh, seen): (Vec<_>, Vec<_>) = postings.into_iter().partition(|p| known.insert(p.key()));
    for (key, url) in seen.iter().filter_map(|p| Some((p.key(), p.url.as_ref()?))) {
        let filled = sqlx::query("UPDATE job_postings SET url = ? WHERE key = ? AND url IS NULL")
            .bind(url)
            .bind(&key)
            .execute(pool)
            .await?
            .rows_affected();
        if filled == 1 {
            sqlx::query(
                "UPDATE tasks SET links = ? WHERE links IS NULL \
                 AND id = (SELECT task_id FROM job_postings WHERE key = ?)",
            )
            .bind(serde_json::json!([url]).to_string())
            .bind(&key)
            .execute(pool)
            .await?;
        }
    }
    Ok(fresh)
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
