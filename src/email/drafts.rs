//! Saved-but-unsent compose state: `email_drafts` CRUD backing `App`'s drafts list popup.

use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{FromRow, SqlitePool};

#[derive(Debug, Clone, FromRow)]
pub struct Draft {
    pub id: i64,
    pub account: String,
    pub to_addrs: String,
    pub cc_addrs: String,
    pub subject: String,
    pub body: String,
    pub updated_at: DateTime<Utc>,
}

/// Inserts a new draft, or overwrites an existing one when `id` is `Some`. Returns the draft's id
/// (the existing one, or the new `last_insert_rowid` on insert).
///
/// # Errors
///
/// Returns an error if the write fails.
pub async fn save_draft(
    pool: &SqlitePool,
    id: Option<i64>,
    account: &str,
    to_addrs: &str,
    cc_addrs: &str,
    subject: &str,
    body: &str,
) -> Result<i64> {
    let now = Utc::now();

    if let Some(id) = id {
        sqlx::query(
            "UPDATE email_drafts SET account = ?, to_addrs = ?, cc_addrs = ?, subject = ?, body = ?, updated_at = ? WHERE id = ?",
        )
        .bind(account)
        .bind(to_addrs)
        .bind(cc_addrs)
        .bind(subject)
        .bind(body)
        .bind(now)
        .bind(id)
        .execute(pool)
        .await?;

        Ok(id)
    } else {
        let result = sqlx::query(
            "INSERT INTO email_drafts (account, to_addrs, cc_addrs, subject, body, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(account)
        .bind(to_addrs)
        .bind(cc_addrs)
        .bind(subject)
        .bind(body)
        .bind(now)
        .execute(pool)
        .await?;

        Ok(result.last_insert_rowid())
    }
}

/// Most-recently-updated draft first.
///
/// # Errors
///
/// Returns an error if the query fails.
pub async fn list_drafts(pool: &SqlitePool) -> Result<Vec<Draft>> {
    let drafts = sqlx::query_as::<_, Draft>("SELECT * FROM email_drafts ORDER BY updated_at DESC")
        .fetch_all(pool)
        .await?;

    Ok(drafts)
}

/// # Errors
///
/// Returns an error if the delete fails.
pub async fn delete_draft(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM email_drafts WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;

    Ok(())
}
