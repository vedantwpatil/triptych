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

    eprintln!("[Migration] Calendar schema ready ✓");
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
