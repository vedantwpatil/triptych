use anyhow::Result;
use sqlx::SqlitePool;

use super::message::{EmailMessage, NewEmail};

/// Insert newly-fetched emails, skipping ones already stored (same `message_id`).
pub async fn insert_new(pool: &SqlitePool, emails: &[NewEmail]) -> Result<()> {
    for email in emails {
        sqlx::query(
            r#"
            INSERT OR IGNORE INTO email_messages
                (uid, message_id, folder, from_addr, from_name, subject, date_utc, snippet)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(email.uid)
        .bind(&email.message_id)
        .bind(&email.folder)
        .bind(&email.from_addr)
        .bind(&email.from_name)
        .bind(&email.subject)
        .bind(email.date_utc)
        .bind(&email.snippet)
        .execute(pool)
        .await?;
    }

    Ok(())
}

pub async fn get_recent(pool: &SqlitePool, limit: i64) -> Result<Vec<EmailMessage>> {
    let emails = sqlx::query_as::<_, EmailMessage>(
        r#"
        SELECT id, uid, message_id, folder, from_addr, from_name, subject, date_utc,
               snippet, is_read, task_id
        FROM email_messages
        ORDER BY date_utc DESC
        LIMIT ?
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(emails)
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
