use anyhow::Result;
use sqlx::SqlitePool;

pub async fn run_calendar_migration(pool: &SqlitePool) -> Result<()> {
    eprintln!("[Migration] Checking calendar schema...");

    // Check and add tasks columns safely
    if !column_exists(pool, "tasks", "scheduled_event_id").await? {
        sqlx::query(
            "ALTER TABLE tasks ADD COLUMN scheduled_event_id INTEGER REFERENCES events(id)",
        )
        .execute(pool)
        .await?;
        eprintln!("  ✓ Added scheduled_event_id to tasks");
    }

    if !column_exists(pool, "tasks", "task_category").await? {
        sqlx::query("ALTER TABLE tasks ADD COLUMN task_category TEXT DEFAULT 'general'")
            .execute(pool)
            .await?;
        eprintln!("  ✓ Added task_category to tasks");
    }

    // Check and add events columns
    if !column_exists(pool, "events", "event_type").await? {
        sqlx::query("ALTER TABLE events ADD COLUMN event_type TEXT DEFAULT 'event'")
            .execute(pool)
            .await?;
        eprintln!("  ✓ Added event_type to events");
    }

    if !column_exists(pool, "events", "recurrence_rule").await? {
        sqlx::query("ALTER TABLE events ADD COLUMN recurrence_rule TEXT")
            .execute(pool)
            .await?;
        eprintln!("  ✓ Added recurrence_rule to events");
    }

    // Smart scheduling: hard deadline separate from scheduled_at, plus task duration
    if !column_exists(pool, "tasks", "deadline").await? {
        sqlx::query("ALTER TABLE tasks ADD COLUMN deadline TEXT")
            .execute(pool)
            .await?;
        eprintln!("  ✓ Added deadline to tasks");
    }

    if !column_exists(pool, "tasks", "duration_minutes").await? {
        sqlx::query("ALTER TABLE tasks ADD COLUMN duration_minutes INTEGER DEFAULT 90")
            .execute(pool)
            .await?;
        eprintln!("  ✓ Added duration_minutes to tasks");
    }

    // Create schedule_blocks table
    sqlx::query(
        r#"
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
    "#,
    )
    .execute(pool)
    .await?;
    eprintln!("  ✓ Schedule blocks table ready");

    // Create indexes
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_schedule_blocks_day ON schedule_blocks(day_of_week, start_time)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_tasks_category ON tasks(task_category)")
        .execute(pool)
        .await?;

    // Task-to-block allocations produced by the smart scheduler
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS task_block_allocations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
            block_date TEXT NOT NULL,
            block_start_time TEXT NOT NULL,
            block_end_time TEXT NOT NULL,
            allocated_minutes INTEGER NOT NULL,
            created_at TEXT DEFAULT CURRENT_TIMESTAMP
        )
    "#,
    )
    .execute(pool)
    .await?;
    eprintln!("  ✓ Task block allocations table ready");

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

    eprintln!("[Migration] Calendar schema ready ✓");
    Ok(())
}

pub async fn run_email_migration(pool: &SqlitePool) -> Result<()> {
    eprintln!("[Migration] Checking email schema...");

    sqlx::query(
        r#"
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
    "#,
    )
    .execute(pool)
    .await?;
    eprintln!("  ✓ Email messages table ready");

    // Pre-multi-account tables were created with `message_id TEXT NOT NULL UNIQUE`
    // (global uniqueness) and no `account` column. That constraint is wrong once
    // more than one mailbox is synced: the same Message-ID can legitimately arrive
    // in two different accounts (mailing lists, CCs), and `INSERT OR IGNORE` would
    // silently drop the second account's copy. SQLite can't drop a column-level
    // UNIQUE via ALTER TABLE, so rebuild the table when `account` is missing.
    if !column_exists(pool, "email_messages", "account").await? {
        eprintln!("  Rebuilding email_messages to scope uniqueness by account...");
        sqlx::query("ALTER TABLE email_messages RENAME TO email_messages_old")
            .execute(pool)
            .await?;

        sqlx::query(
            r#"
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
        "#,
        )
        .execute(pool)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO email_messages
                (id, uid, message_id, account, folder, from_addr, from_name, subject,
                 date_utc, snippet, is_read, task_id, created_at)
            SELECT id, uid, message_id, 'default', folder, from_addr, from_name, subject,
                   date_utc, snippet, is_read, task_id, created_at
            FROM email_messages_old
        "#,
        )
        .execute(pool)
        .await?;

        sqlx::query("DROP TABLE email_messages_old")
            .execute(pool)
            .await?;
        eprintln!("  ✓ email_messages rebuilt with account column");
    }

    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_email_messages_account_msgid ON email_messages(account, message_id)",
    )
    .execute(pool)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_email_messages_date ON email_messages(date_utc)")
        .execute(pool)
        .await?;

    eprintln!("[Migration] Email schema ready ✓");
    Ok(())
}

async fn column_exists(pool: &SqlitePool, table: &str, column: &str) -> Result<bool> {
    let count: i64 = sqlx::query_scalar(&format!(
        "SELECT COUNT(*) FROM pragma_table_info('{}') WHERE name = ?",
        table
    ))
    .bind(column)
    .fetch_one(pool)
    .await?;

    Ok(count > 0)
}
