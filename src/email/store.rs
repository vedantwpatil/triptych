use std::collections::HashSet;

use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};

use super::message::{EmailAttachment, EmailMessage, EmailRule, NewEmail};

/// Insert newly-fetched emails, skipping ones already stored (same `account` +
/// `message_id` — the same Message-ID can legitimately show up in more than one
/// account, e.g. mailing lists or CCs). Returns how many rows were actually new.
pub async fn insert_new(pool: &SqlitePool, emails: &[NewEmail]) -> Result<u64> {
    let mut tx = pool.begin().await?;
    let mut inserted = 0;

    for email in emails {
        let result = sqlx::query(
            r"
            INSERT OR IGNORE INTO email_messages
                (uid, message_id, account, folder, from_addr, from_name, subject, date_utc, snippet, body_text,
                 to_addrs, cc_addrs, references_header, meeting_title, meeting_start, meeting_end, meeting_location)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
        .bind(&email.to_addrs)
        .bind(&email.cc_addrs)
        .bind(&email.references_header)
        .bind(&email.meeting_title)
        .bind(email.meeting_start)
        .bind(email.meeting_end)
        .bind(&email.meeting_location)
        .execute(&mut *tx)
        .await?;

        // `rows_affected() == 0` means the uniqueness constraint skipped this row (already
        // stored) — `last_insert_rowid()` would then be stale, pointing at some earlier insert,
        // not this email. Only trust it right after a row this call actually inserted.
        if result.rows_affected() == 1 {
            inserted += 1;
            let email_id = result.last_insert_rowid();
            for attachment in &email.attachments {
                sqlx::query(
                    r"
                    INSERT INTO email_attachments (email_id, part_index, filename, content_type, size_bytes)
                    VALUES (?, ?, ?, ?, ?)
                    ",
                )
                .bind(email_id)
                .bind(attachment.part_index)
                .bind(&attachment.filename)
                .bind(&attachment.content_type)
                .bind(attachment.size_bytes)
                .execute(&mut *tx)
                .await?;
            }
        }
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

/// Drops every sync cursor so the next sync is a first sync (windowed backfill).
pub async fn clear_sync_cursors(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM email_sync_state")
        .execute(pool)
        .await?;
    Ok(())
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
               snippet, is_read, task_id, NULL AS body_text, to_addrs, cc_addrs, references_header,
               is_starred, category, snoozed_until, triage_focused,
               meeting_title, meeting_start, meeting_end, meeting_location,
               EXISTS(SELECT 1 FROM email_attachments a WHERE a.email_id = email_messages.id) AS has_attachments
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

/// One email's attachment metadata (no bytes — see `MailSource::fetch_attachments`), in
/// `part_index` order. Populated into `App`'s per-email cache when its detail popup opens
/// (mirrors `get_body`'s on-demand load), not carried by `get_recent`'s list rows.
pub async fn get_attachments(pool: &SqlitePool, email_id: i64) -> Result<Vec<EmailAttachment>> {
    let attachments = sqlx::query_as::<_, EmailAttachment>(
        r"
        SELECT id, email_id, part_index, filename, content_type, size_bytes
        FROM email_attachments
        WHERE email_id = ?
        ORDER BY part_index
        ",
    )
    .bind(email_id)
    .fetch_all(pool)
    .await?;

    Ok(attachments)
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

/// Ids of every email whose stored `body_text` contains `needle` (SQLite `LIKE`, case-insensitive
/// for ASCII). Backs `/` search's body-text match, since [`get_recent`]'s listing never carries
/// `body_text` in memory to scan.
pub async fn search_body_matches(pool: &SqlitePool, needle: &str) -> Result<HashSet<i64>> {
    let pattern = format!("%{}%", needle.replace('%', "\\%").replace('_', "\\_"));
    let rows = sqlx::query("SELECT id FROM email_messages WHERE body_text LIKE ? ESCAPE '\\'")
        .bind(pattern)
        .fetch_all(pool)
        .await?;

    Ok(rows.iter().map(|row| row.get::<i64, _>("id")).collect())
}

/// Every distinct account label with at least one stored message, alphabetical — backs the
/// email list's per-account filter cycle.
pub async fn distinct_accounts(pool: &SqlitePool) -> Result<Vec<String>> {
    let rows = sqlx::query("SELECT DISTINCT account FROM email_messages ORDER BY account")
        .fetch_all(pool)
        .await?;

    Ok(rows
        .iter()
        .map(|row| row.get::<String, _>("account"))
        .collect())
}

/// Every distinct folder name with at least one stored message, alphabetical — backs the
/// email list's per-folder filter cycle (Slice 13).
pub async fn distinct_folders(pool: &SqlitePool) -> Result<Vec<String>> {
    let rows = sqlx::query("SELECT DISTINCT folder FROM email_messages ORDER BY folder")
        .fetch_all(pool)
        .await?;

    Ok(rows
        .iter()
        .map(|row| row.get::<String, _>("folder"))
        .collect())
}

/// Every distinct sender-domain with at least one stored message, alphabetical — backs the
/// email list's per-domain filter cycle (Slice 25). `from_addr` is always a bare address
/// (`message.rs::parse_raw` already strips any display name), so splitting on the first `@`
/// is enough; a row with no `@` at all (never seen in practice — the parser's own fallback is
/// `"unknown@unknown"`) is excluded rather than grouped under an empty string.
pub async fn distinct_domains(pool: &SqlitePool) -> Result<Vec<String>> {
    let rows = sqlx::query(
        "SELECT DISTINCT substr(from_addr, instr(from_addr, '@') + 1) AS domain
         FROM email_messages WHERE instr(from_addr, '@') > 0 ORDER BY domain",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|row| row.get::<String, _>("domain"))
        .collect())
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

/// Emails not yet classified (`triage_focused IS NULL`), most recent first, capped at `limit` —
/// backs `App::run_email_triage`, which runs a pass after every sync over whatever's new. Subject
/// and snippet (never the full body, `NULL AS body_text` as in [`get_recent`]) are enough for the
/// model to classify, the same fields `priority::score` already keys off of.
pub async fn pending_triage(pool: &SqlitePool, limit: i64) -> Result<Vec<EmailMessage>> {
    let emails = sqlx::query_as::<_, EmailMessage>(
        r"
        SELECT id, uid, message_id, account, folder, from_addr, from_name, subject, date_utc,
               snippet, is_read, task_id, NULL AS body_text, to_addrs, cc_addrs, references_header,
               is_starred, category, snoozed_until, triage_focused,
               meeting_title, meeting_start, meeting_end, meeting_location,
               EXISTS(SELECT 1 FROM email_attachments a WHERE a.email_id = email_messages.id) AS has_attachments
        FROM email_messages
        WHERE triage_focused IS NULL
        ORDER BY date_utc DESC
        LIMIT ?
        ",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(emails)
}

/// Records one email's Focused/Other classification (Outlook's Focused Inbox split), so the model
/// is asked once per email.
pub async fn set_triage(pool: &SqlitePool, email_id: i64, focused: bool) -> Result<()> {
    sqlx::query("UPDATE email_messages SET triage_focused = ? WHERE id = ?")
        .bind(focused)
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

pub async fn mark_unread(pool: &SqlitePool, email_id: i64) -> Result<()> {
    sqlx::query("UPDATE email_messages SET is_read = 0 WHERE id = ?")
        .bind(email_id)
        .execute(pool)
        .await?;

    Ok(())
}

pub async fn set_starred(pool: &SqlitePool, email_id: i64, starred: bool) -> Result<()> {
    sqlx::query("UPDATE email_messages SET is_starred = ? WHERE id = ?")
        .bind(starred)
        .bind(email_id)
        .execute(pool)
        .await?;

    Ok(())
}

/// Sets or clears (`None`) the selected email's colored category tag (Slice 22) — backs
/// `App::cycle_selected_category`.
pub async fn set_category(pool: &SqlitePool, email_id: i64, category: Option<&str>) -> Result<()> {
    sqlx::query("UPDATE email_messages SET category = ? WHERE id = ?")
        .bind(category)
        .bind(email_id)
        .execute(pool)
        .await?;

    Ok(())
}

/// Hides a message from the normal list until `until` (see `EmailMessage::snoozed_until`).
pub async fn set_snooze(pool: &SqlitePool, email_id: i64, until: DateTime<Utc>) -> Result<()> {
    sqlx::query("UPDATE email_messages SET snoozed_until = ? WHERE id = ?")
        .bind(until)
        .bind(email_id)
        .execute(pool)
        .await?;

    Ok(())
}

/// Clears an in-progress snooze early, before `snoozed_until` would have lapsed on its own.
pub async fn clear_snooze(pool: &SqlitePool, email_id: i64) -> Result<()> {
    sqlx::query("UPDATE email_messages SET snoozed_until = NULL WHERE id = ?")
        .bind(email_id)
        .execute(pool)
        .await?;

    Ok(())
}

/// Removes one message's local row. Caller deletes it on the server first (`MailSource::delete`)
/// and only calls this on that success, so a failed remote delete never desyncs the local copy
/// from mail the server still has.
pub async fn delete_email(pool: &SqlitePool, email_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM email_messages WHERE id = ?")
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

/// Every user-defined rule, oldest first (creation order — the order they'd have been applied in
/// if run one at a time) — backs `App::run_email_rules` and the rules-list popup.
pub async fn list_rules(pool: &SqlitePool) -> Result<Vec<EmailRule>> {
    let rules = sqlx::query_as::<_, EmailRule>(
        "SELECT id, match_field, pattern, action, created_at FROM email_rules ORDER BY created_at",
    )
    .fetch_all(pool)
    .await?;

    Ok(rules)
}

/// Adds one rule, returns its new id.
pub async fn create_rule(
    pool: &SqlitePool,
    match_field: &str,
    pattern: &str,
    action: &str,
) -> Result<i64> {
    let result = sqlx::query(
        "INSERT INTO email_rules (match_field, pattern, action, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(match_field)
    .bind(pattern)
    .bind(action)
    .bind(Utc::now())
    .execute(pool)
    .await?;

    Ok(result.last_insert_rowid())
}

pub async fn delete_rule(pool: &SqlitePool, rule_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM email_rules WHERE id = ?")
        .bind(rule_id)
        .execute(pool)
        .await?;

    Ok(())
}

/// Emails not yet checked against `email_rules` (`rule_applied = 0`), most recent first, capped at
/// `limit` — backs `App::run_email_rules`, the same pending-work shape as [`pending_triage`]. Only
/// `subject`/`from_addr` are selected (plus the id), since those are the only fields any rule can
/// currently match on.
pub async fn pending_rule_check(pool: &SqlitePool, limit: i64) -> Result<Vec<EmailMessage>> {
    let emails = sqlx::query_as::<_, EmailMessage>(
        r"
        SELECT id, uid, message_id, account, folder, from_addr, from_name, subject, date_utc,
               snippet, is_read, task_id, NULL AS body_text, to_addrs, cc_addrs, references_header,
               is_starred, category, snoozed_until, triage_focused,
               meeting_title, meeting_start, meeting_end, meeting_location,
               EXISTS(SELECT 1 FROM email_attachments a WHERE a.email_id = email_messages.id) AS has_attachments
        FROM email_messages
        WHERE rule_applied = 0
        ORDER BY date_utc DESC
        LIMIT ?
        ",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(emails)
}

/// Marks one email as checked against every current rule, so `pending_rule_check` never
/// re-offers it. Set regardless of whether any rule actually matched — a non-match is still a
/// completed check, not pending work.
pub async fn mark_rule_checked(pool: &SqlitePool, email_id: i64) -> Result<()> {
    sqlx::query("UPDATE email_messages SET rule_applied = 1 WHERE id = ?")
        .bind(email_id)
        .execute(pool)
        .await?;

    Ok(())
}
