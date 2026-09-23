use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};

use super::message::{EmailMessage, NewEmail};

/// Insert newly-fetched emails, skipping ones already stored (same `account` +
/// `message_id` — the same Message-ID can legitimately show up in more than one
/// account, e.g. mailing lists or CCs). Returns how many rows were actually new.
pub async fn insert_new(pool: &SqlitePool, emails: &[NewEmail]) -> Result<u64> {
    let mut tx = pool.begin().await?;
    let mut inserted = 0;

    for email in emails {
        inserted += sqlx::query(
            r"
            INSERT OR IGNORE INTO email_messages
                (uid, message_id, account, folder, from_addr, from_name, subject, date_utc, snippet, body_text)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ",
        )
        .bind(email.uid)
        .bind(&email.message_id)
        .bind(&email.account)
        .bind(&email.folder)
        .bind(&email.from_addr)
        .bind(&email.from_name)
        .bind(&email.subject)
        .bind(email.date_utc)
        .bind(&email.snippet)
        .bind(&email.body_text)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    }

    tx.commit().await?;
    Ok(inserted)
}

/// A sync resume point for one account+folder: the IMAP UIDVALIDITY epoch it was
/// captured under, and the highest UID synced within that epoch. Fields are `i64`
/// to match the SQLite columns; `client::ImapMailSource` casts to `u32` at the
/// IMAP-protocol boundary. A named struct instead of a `(i64, i64)` tuple so the
/// two same-typed fields can't be silently swapped at a call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncCursor {
    pub uid_validity: i64,
    pub last_uid: i64,
}

/// This account+folder's resume point from last time, or `None` if never synced.
/// Deliberately *not* derived from `MAX(uid)` over `email_messages`: that table
/// can hold rows from more than one UIDVALIDITY epoch at once (dedup is by
/// `message_id`, not `uid`, so old-epoch rows are never removed when the epoch
/// changes), so its max would silently mix a stale epoch's UID back into a live
/// search. This cursor is written by [`set_sync_cursor`] once per sync, scoped to
/// whichever epoch was current then.
pub async fn get_sync_cursor(
    pool: &SqlitePool,
    account: &str,
    folder: &str,
) -> Result<Option<SyncCursor>> {
    let row = sqlx::query(
        "SELECT uid_validity, last_uid FROM email_sync_state WHERE account = ? AND folder = ?",
    )
    .bind(account)
    .bind(folder)
    .fetch_optional(pool)
    .await?;

    Ok(match row {
        Some(row) => Some(SyncCursor {
            uid_validity: row.try_get("uid_validity")?,
            last_uid: row.try_get("last_uid")?,
        }),
        None => None,
    })
}

pub async fn set_sync_cursor(
    pool: &SqlitePool,
    account: &str,
    folder: &str,
    cursor: SyncCursor,
) -> Result<()> {
    sqlx::query(
        r"
        INSERT INTO email_sync_state (account, folder, uid_validity, last_uid)
        VALUES (?, ?, ?, ?)
        ON CONFLICT(account, folder) DO UPDATE SET
            uid_validity = excluded.uid_validity,
            last_uid = excluded.last_uid
        ",
    )
    .bind(account)
    .bind(folder)
    .bind(cursor.uid_validity)
    .bind(cursor.last_uid)
    .execute(pool)
    .await?;

    Ok(())
}

/// Merged inbox across all accounts, most recent first. Only one email's body is
/// ever on screen at a time (the detail popup), so this deliberately leaves
/// `body_text` unset (`NULL AS body_text`, not the real column) rather than
/// pulling every row's full body off disk just to list subjects/senders — call
/// [`get_body`] on demand when a specific email is opened.
pub async fn get_recent(pool: &SqlitePool, limit: i64) -> Result<Vec<EmailMessage>> {
    let emails = sqlx::query_as::<_, EmailMessage>(
        r"
        SELECT id, uid, message_id, account, folder, from_addr, from_name, subject, date_utc,
               snippet, is_read, task_id, NULL AS body_text
        FROM email_messages
        ORDER BY date_utc DESC
        LIMIT ?
        ",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(emails)
}

/// Fetches one email's full body on demand — the counterpart to [`get_recent`]
/// leaving `body_text` unset in its listing.
pub async fn get_body(pool: &SqlitePool, email_id: i64) -> Result<Option<String>> {
    let row = sqlx::query("SELECT body_text FROM email_messages WHERE id = ?")
        .bind(email_id)
        .fetch_optional(pool)
        .await?;

    Ok(row.and_then(|row| row.try_get::<Option<String>, _>("body_text").ok().flatten()))
}

/// The cached AI summary for one email, if one was generated.
pub async fn get_summary(pool: &SqlitePool, email_id: i64) -> Result<Option<String>> {
    let summary = sqlx::query_scalar("SELECT summary FROM email_messages WHERE id = ?")
        .bind(email_id)
        .fetch_optional(pool)
        .await?;

    Ok(summary.flatten())
}

/// Caches an AI summary so the model is asked once per email.
pub async fn set_summary(pool: &SqlitePool, email_id: i64, summary: &str) -> Result<()> {
    sqlx::query("UPDATE email_messages SET summary = ? WHERE id = ?")
        .bind(summary)
        .bind(email_id)
        .execute(pool)
        .await?;

    Ok(())
}

/// Deletes emails older than `cutoff`. Caller (`App::cleanup_old_emails`) runs this
/// on every Email view entry with a 6-month cutoff — there's no other retention
/// path, so without it `email_messages` grows unbounded. `date_utc` is indexed
/// (`idx_email_messages_date`), so this is a cheap indexed range delete, not a
/// table scan.
pub async fn delete_older_than(pool: &SqlitePool, cutoff: DateTime<Utc>) -> Result<u64> {
    let result = sqlx::query("DELETE FROM email_messages WHERE date_utc < ?")
        .bind(cutoff)
        .execute(pool)
        .await?;

    Ok(result.rows_affected())
}

pub async fn mark_read(pool: &SqlitePool, email_id: i64) -> Result<()> {
    sqlx::query("UPDATE email_messages SET is_read = 1 WHERE id = ?")
        .bind(email_id)
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn link_task(pool: &SqlitePool, email_id: i64, task_id: i64) -> Result<()> {
    sqlx::query("UPDATE email_messages SET task_id = ? WHERE id = ?")
        .bind(task_id)
        .bind(email_id)
        .execute(pool)
        .await?;

    Ok(())
}
