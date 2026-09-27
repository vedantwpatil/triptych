use anyhow::Result;
use sqlx::SqlitePool;

/// Adds the calendar columns and tables if they are missing. Safe to run on every start.
///
/// # Errors
///
/// Returns an error if a schema change fails.
pub async fn run_calendar_migration(pool: &SqlitePool) -> Result<()> {
    tracing::debug!("[Migration] Checking calendar schema...");

    // Check and add tasks columns safely
    if !column_exists(pool, "tasks", "scheduled_event_id").await? {
        sqlx::query(
            "ALTER TABLE tasks ADD COLUMN scheduled_event_id INTEGER REFERENCES events(id)",
        )
        .execute(pool)
        .await?;
        tracing::info!("  ✓ Added scheduled_event_id to tasks");
    }

    if !column_exists(pool, "tasks", "task_category").await? {
        sqlx::query("ALTER TABLE tasks ADD COLUMN task_category TEXT DEFAULT 'general'")
            .execute(pool)
            .await?;
        tracing::info!("  ✓ Added task_category to tasks");
    }

    // Check and add events columns
    if !column_exists(pool, "events", "event_type").await? {
        sqlx::query("ALTER TABLE events ADD COLUMN event_type TEXT DEFAULT 'event'")
            .execute(pool)
            .await?;
        tracing::info!("  ✓ Added event_type to events");
    }

    if !column_exists(pool, "events", "recurrence_rule").await? {
        sqlx::query("ALTER TABLE events ADD COLUMN recurrence_rule TEXT")
            .execute(pool)
            .await?;
        tracing::info!("  ✓ Added recurrence_rule to events");
    }

    // Smart scheduling: hard deadline separate from scheduled_at, plus task duration
    if !column_exists(pool, "tasks", "deadline").await? {
        sqlx::query("ALTER TABLE tasks ADD COLUMN deadline TEXT")
            .execute(pool)
            .await?;
        tracing::info!("  ✓ Added deadline to tasks");
    }

    if !column_exists(pool, "tasks", "duration_minutes").await? {
        sqlx::query("ALTER TABLE tasks ADD COLUMN duration_minutes INTEGER DEFAULT 90")
            .execute(pool)
            .await?;
        tracing::info!("  ✓ Added duration_minutes to tasks");
    }

    // Create schedule_blocks table
    sqlx::query(
        r"
        CREATE TABLE IF NOT EXISTS schedule_blocks (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            day_of_week INTEGER NOT NULL CHECK(day_of_week >= 0 AND day_of_week <= 6),
            start_time TEXT NOT NULL,
            end_time TEXT NOT NULL,
            block_type TEXT NOT NULL,
            title TEXT NOT NULL,
            description TEXT,
            priority INTEGER DEFAULT 1,
            created_at TEXT DEFAULT CURRENT_TIMESTAMP
        )
    ",
    )
    .execute(pool)
    .await?;
    tracing::debug!("  ✓ Schedule blocks table ready");

    // Create indexes
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_schedule_blocks_day ON schedule_blocks(day_of_week, start_time)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_tasks_category ON tasks(task_category)")
        .execute(pool)
        .await?;

    // Task-to-block allocations produced by the smart scheduler
    sqlx::query(
        r"
        CREATE TABLE IF NOT EXISTS task_block_allocations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
            block_date TEXT NOT NULL,
            block_start_time TEXT NOT NULL,
            block_end_time TEXT NOT NULL,
            allocated_minutes INTEGER NOT NULL,
            created_at TEXT DEFAULT CURRENT_TIMESTAMP
        )
    ",
    )
    .execute(pool)
    .await?;
    tracing::debug!("  ✓ Task block allocations table ready");

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_allocations_task ON task_block_allocations(task_id)",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_allocations_date ON task_block_allocations(block_date)",
    )
    .execute(pool)
    .await?;

    tracing::debug!("[Migration] Calendar schema ready ✓");
    Ok(())
}

// One linear sequence of idempotent CREATE/ALTER checks - splitting it into
// helpers would scatter that sequence without reducing its actual complexity.
/// Adds the email tables if they are missing. Safe to run on every start.
///
/// # Errors
///
/// Returns an error if a schema change fails.
#[allow(clippy::too_many_lines)]
pub async fn run_email_migration(pool: &SqlitePool) -> Result<()> {
    tracing::debug!("[Migration] Checking email schema...");

    sqlx::query(
        r"
        CREATE TABLE IF NOT EXISTS email_messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            uid INTEGER NOT NULL,
            message_id TEXT NOT NULL,
            account TEXT NOT NULL DEFAULT 'default',
            folder TEXT NOT NULL DEFAULT 'INBOX',
            from_addr TEXT NOT NULL,
            from_name TEXT,
            subject TEXT NOT NULL,
            date_utc TEXT NOT NULL,
            snippet TEXT,
            is_read INTEGER NOT NULL DEFAULT 0,
            task_id INTEGER REFERENCES tasks(id),
            created_at TEXT DEFAULT CURRENT_TIMESTAMP
        )
    ",
    )
    .execute(pool)
    .await?;
    tracing::debug!("  ✓ Email messages table ready");

    // Pre-multi-account tables were created with `message_id TEXT NOT NULL UNIQUE`
    // (global uniqueness) and no `account` column. That constraint is wrong once
    // more than one mailbox is synced: the same Message-ID can legitimately arrive
    // in two different accounts (mailing lists, CCs), and `INSERT OR IGNORE` would
    // silently drop the second account's copy. SQLite can't drop a column-level
    // UNIQUE via ALTER TABLE, so rebuild the table when `account` is missing.
    if !column_exists(pool, "email_messages", "account").await? {
        tracing::info!("  Rebuilding email_messages to scope uniqueness by account...");
        sqlx::query("ALTER TABLE email_messages RENAME TO email_messages_old")
            .execute(pool)
            .await?;

        sqlx::query(
            r"
            CREATE TABLE email_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                uid INTEGER NOT NULL,
                message_id TEXT NOT NULL,
                account TEXT NOT NULL DEFAULT 'default',
                folder TEXT NOT NULL DEFAULT 'INBOX',
                from_addr TEXT NOT NULL,
                from_name TEXT,
                subject TEXT NOT NULL,
                date_utc TEXT NOT NULL,
                snippet TEXT,
                is_read INTEGER NOT NULL DEFAULT 0,
                task_id INTEGER REFERENCES tasks(id),
                created_at TEXT DEFAULT CURRENT_TIMESTAMP
            )
        ",
        )
        .execute(pool)
        .await?;

        sqlx::query(
            r"
            INSERT INTO email_messages
                (id, uid, message_id, account, folder, from_addr, from_name, subject,
                 date_utc, snippet, is_read, task_id, created_at)
            SELECT id, uid, message_id, 'default', folder, from_addr, from_name, subject,
                   date_utc, snippet, is_read, task_id, created_at
            FROM email_messages_old
        ",
        )
        .execute(pool)
        .await?;

        sqlx::query("DROP TABLE email_messages_old")
            .execute(pool)
            .await?;
        tracing::info!("  ✓ email_messages rebuilt with account column");
    }

    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_email_messages_account_msgid ON email_messages(account, message_id)",
    )
    .execute(pool)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_email_messages_date ON email_messages(date_utc)")
        .execute(pool)
        .await?;

    if !column_exists(pool, "email_messages", "body_text").await? {
        sqlx::query("ALTER TABLE email_messages ADD COLUMN body_text TEXT")
            .execute(pool)
            .await?;
        tracing::info!("  ✓ Added body_text column to email_messages");
    }

    if !column_exists(pool, "email_messages", "summary").await? {
        sqlx::query("ALTER TABLE email_messages ADD COLUMN summary TEXT")
            .execute(pool)
            .await?;
        tracing::info!("  ✓ Added summary column to email_messages");
    }

    // `email_messages.task_id` was created without `ON DELETE`, so deleting a task an email was
    // converted into failed with a FOREIGN KEY error. SQLite can't alter an FK in place; this
    // trigger gives it `ON DELETE SET NULL`. It must be created after the table rebuild above,
    // since `RENAME TO` would repoint the trigger at the dropped `email_messages_old`.
    sqlx::query(
        r"
        CREATE TRIGGER IF NOT EXISTS email_messages_unlink_task
        BEFORE DELETE ON tasks
        BEGIN
            UPDATE email_messages SET task_id = NULL WHERE task_id = OLD.id;
        END
    ",
    )
    .execute(pool)
    .await?;

    // Tracks each (account, folder)'s last-known IMAP UIDVALIDITY so sync can
    // detect a server-side UID epoch change (e.g. Gmail can renumber a mailbox's
    // UIDs) and fall back to a fresh catch-up instead of resuming from a stale,
    // no-longer-meaningful `max_uid`.
    sqlx::query(
        r"
        CREATE TABLE IF NOT EXISTS email_sync_state (
            account TEXT NOT NULL,
            folder TEXT NOT NULL,
            uid_validity INTEGER NOT NULL,
            PRIMARY KEY (account, folder)
        )
    ",
    )
    .execute(pool)
    .await?;
    tracing::debug!("  ✓ Email sync state table ready");

    if !column_exists(pool, "email_sync_state", "last_uid").await? {
        sqlx::query("ALTER TABLE email_sync_state ADD COLUMN last_uid INTEGER NOT NULL DEFAULT 0")
            .execute(pool)
            .await?;
        tracing::info!("  ✓ Added last_uid column to email_sync_state");
    }

    // Reply/reply-all/forward prefill and thread-chaining, extracted at parse time from the
    // original message's own To/Cc/References headers (see `email/message.rs::parse_raw`).
    if !column_exists(pool, "email_messages", "to_addrs").await? {
        sqlx::query("ALTER TABLE email_messages ADD COLUMN to_addrs TEXT")
            .execute(pool)
            .await?;
        tracing::info!("  ✓ Added to_addrs column to email_messages");
    }

    if !column_exists(pool, "email_messages", "cc_addrs").await? {
        sqlx::query("ALTER TABLE email_messages ADD COLUMN cc_addrs TEXT")
            .execute(pool)
            .await?;
        tracing::info!("  ✓ Added cc_addrs column to email_messages");
    }

    if !column_exists(pool, "email_messages", "references_header").await? {
        sqlx::query("ALTER TABLE email_messages ADD COLUMN references_header TEXT")
            .execute(pool)
            .await?;
        tracing::info!("  ✓ Added references_header column to email_messages");
    }

    if !column_exists(pool, "email_messages", "is_starred").await? {
        sqlx::query("ALTER TABLE email_messages ADD COLUMN is_starred INTEGER NOT NULL DEFAULT 0")
            .execute(pool)
            .await?;
        tracing::info!("  ✓ Added is_starred column to email_messages");
    }

    // `NULL` (never snoozed, or a snooze that already lapsed and was cleared) or a future UTC
    // instant (RFC3339 text, same representation sqlx already uses for `date_utc`) hiding the
    // message from the normal list until then. See `App::snooze_selected_email`.
    if !column_exists(pool, "email_messages", "snoozed_until").await? {
        sqlx::query("ALTER TABLE email_messages ADD COLUMN snoozed_until TEXT")
            .execute(pool)
            .await?;
        tracing::info!("  ✓ Added snoozed_until column to email_messages");
    }

    // `NULL` (not yet classified, or classification failed/unavailable), 1 (Focused: personal/work
    // mail worth attention) or 0 (Other: bulk/automated) — Outlook's Focused Inbox split, computed
    // once per message by `OllamaClient::triage` after sync. See `App::run_email_triage`.
    if !column_exists(pool, "email_messages", "triage_focused").await? {
        sqlx::query("ALTER TABLE email_messages ADD COLUMN triage_focused INTEGER")
            .execute(pool)
            .await?;
        tracing::info!("  ✓ Added triage_focused column to email_messages");
    }

    // Attachment metadata extracted at parse time (see `email/message.rs::parse_raw`); bytes are
    // never persisted, only fetched on demand via `MailSource::fetch_attachments`. `ON DELETE
    // CASCADE` is enforced: sqlx-sqlite's `SqliteConnectOptions` default is `PRAGMA foreign_keys =
    // ON` (this project never overrides it — `App::build`/`import_schedule.rs` both call
    // `SqlitePool::connect` with no options), so a row here is cleaned up automatically whenever
    // its email is deleted, same as `task_block_allocations` -> `tasks` in the calendar migration.
    sqlx::query(
        r"
        CREATE TABLE IF NOT EXISTS email_attachments (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            email_id INTEGER NOT NULL REFERENCES email_messages(id) ON DELETE CASCADE,
            part_index INTEGER NOT NULL,
            filename TEXT,
            content_type TEXT NOT NULL,
            size_bytes INTEGER NOT NULL
        )
    ",
    )
    .execute(pool)
    .await?;
    tracing::debug!("  ✓ Email attachments table ready");

    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_email_attachments_email ON email_attachments(email_id)",
    )
    .execute(pool)
    .await?;

    // Saved-but-unsent compose state. `to_addrs`/`cc_addrs` are comma-joined strings (matching how
    // `ComposeState` already stores them, not a normalized address table) since drafts are never
    // queried by recipient, only listed and resumed by id.
    sqlx::query(
        r"
        CREATE TABLE IF NOT EXISTS email_drafts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            account TEXT NOT NULL,
            to_addrs TEXT NOT NULL DEFAULT '',
            cc_addrs TEXT NOT NULL DEFAULT '',
            subject TEXT NOT NULL DEFAULT '',
            body TEXT NOT NULL DEFAULT '',
            updated_at TEXT NOT NULL
        )
    ",
    )
    .execute(pool)
    .await?;
    tracing::debug!("  ✓ Email drafts table ready");

    // User-defined auto-actions (Slice 18): `match_field` is `"subject"` or `"from_addr"`,
    // `pattern` a substring tested case-insensitively (see `app::mail::match_rule`), `action`
    // `"star"` or `"read"`. No archive/delete action yet — those need an IMAP round-trip per
    // match, deferred past v1 (see `docs/roadmap-email.md`'s Slice 18).
    sqlx::query(
        r"
        CREATE TABLE IF NOT EXISTS email_rules (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            match_field TEXT NOT NULL,
            pattern TEXT NOT NULL,
            action TEXT NOT NULL,
            created_at TEXT NOT NULL
        )
    ",
    )
    .execute(pool)
    .await?;
    tracing::debug!("  ✓ Email rules table ready");

    // 0 (not yet checked against `email_rules`) or 1 (checked, regardless of whether any rule
    // matched) — lets `App::run_email_rules` skip rows it already processed without re-running
    // every rule against the whole table each pass. See `App::run_email_rules`.
    if !column_exists(pool, "email_messages", "rule_applied").await? {
        sqlx::query("ALTER TABLE email_messages ADD COLUMN rule_applied INTEGER NOT NULL DEFAULT 0")
            .execute(pool)
            .await?;
        tracing::info!("  ✓ Added rule_applied column to email_messages");
    }

    // Meeting-invite fields (Slice 19), extracted from a `text/calendar` MIME part at parse time
    // (`email::message::parse_raw`'s private `extract_meeting_invite`). All `NULL` for a message
    // with no calendar part — that's the common case, not a migration gap. `meeting_start`/
    // `meeting_end` are UTC text, same representation as `date_utc`.
    if !column_exists(pool, "email_messages", "meeting_title").await? {
        sqlx::query("ALTER TABLE email_messages ADD COLUMN meeting_title TEXT")
            .execute(pool)
            .await?;
        tracing::info!("  ✓ Added meeting_title column to email_messages");
    }
    if !column_exists(pool, "email_messages", "meeting_start").await? {
        sqlx::query("ALTER TABLE email_messages ADD COLUMN meeting_start TEXT")
            .execute(pool)
            .await?;
        tracing::info!("  ✓ Added meeting_start column to email_messages");
    }
    if !column_exists(pool, "email_messages", "meeting_end").await? {
        sqlx::query("ALTER TABLE email_messages ADD COLUMN meeting_end TEXT")
            .execute(pool)
            .await?;
        tracing::info!("  ✓ Added meeting_end column to email_messages");
    }
    if !column_exists(pool, "email_messages", "meeting_location").await? {
        sqlx::query("ALTER TABLE email_messages ADD COLUMN meeting_location TEXT")
            .execute(pool)
            .await?;
        tracing::info!("  ✓ Added meeting_location column to email_messages");
    }

    // Outlook-style colored category tag (Slice 22), one per message: `NULL` (untagged) or one of
    // `app::mail::CATEGORY_ORDER`'s colour names. Cycled by `App::cycle_selected_category` (`t` in
    // the email list).
    if !column_exists(pool, "email_messages", "category").await? {
        sqlx::query("ALTER TABLE email_messages ADD COLUMN category TEXT")
            .execute(pool)
            .await?;
        tracing::info!("  ✓ Added category column to email_messages");
    }

    tracing::debug!("[Migration] Email schema ready ✓");
    Ok(())
}

async fn column_exists(pool: &SqlitePool, table: &str, column: &str) -> Result<bool> {
    let count: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?"
    ))
    .bind(column)
    .fetch_one(pool)
    .await?;

    Ok(count > 0)
}
