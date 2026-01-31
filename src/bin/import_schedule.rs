use anyhow::Result;
use sqlx::sqlite::SqlitePool;

#[tokio::main]
async fn main() -> Result<()> {
    let db_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite:triptych.db".to_string());

    let pool = SqlitePool::connect(&db_url).await?;

    println!("🔄 Importing schedule template...");

    // Clear existing blocks
    sqlx::query("DELETE FROM schedule_blocks")
        .execute(&pool)
        .await?;

    let schedule = vec![
        // Monday (1)
        (1, "07:00", "08:00", "fitness", "Full Body Workout A"),
        (1, "09:00", "10:30", "deepwork", "Deep Work Block 1"),
        (1, "10:30", "11:50", "class", "CS 281-A"),
        (1, "12:00", "13:20", "class", "CS 277-001"),
        (1, "13:20", "14:00", "meal", "Lunch"),
        (1, "14:00", "15:00", "class", "LING 102-001"),
        (1, "15:00", "18:00", "class", "SE 310-001"),
        (1, "18:00", "19:30", "meal", "Dinner & Decompress"),
        (1, "19:30", "21:00", "relax", "Chess/Review"),
        (1, "21:00", "23:00", "winddown", "Reading"),
        // Tuesday (2)
        (2, "07:00", "08:00", "fitness", "HIIT/Cardio"),
        (2, "09:00", "10:30", "deepwork", "Deep Work Block 1"),
        (2, "12:00", "13:20", "class", "MATH 300-B"),
        (2, "14:00", "15:20", "class", "SOC 101-001"),
        (2, "15:30", "17:00", "deepwork", "Deep Work Block 2"),
        (2, "17:30", "18:30", "admin", "Secondary Tasks"),
        // Wednesday (3)
        (3, "07:00", "08:00", "fitness", "Full Body Workout B"),
        (3, "09:00", "10:30", "deepwork", "Deep Work Block 1"),
        (3, "10:30", "11:50", "class", "CS 281-A"),
        (3, "12:00", "13:20", "class", "CS 277-001"),
        (3, "14:00", "15:00", "class", "LING 102-001"),
        (3, "15:30", "17:00", "deepwork", "Deep Work Block 2"),
        // Thursday (4)
        (4, "07:00", "08:00", "fitness", "Cardio & Mobility"),
        (4, "09:00", "10:50", "class", "CS 081-001"),
        (4, "12:00", "13:20", "class", "MATH 300-B"),
        (4, "14:00", "15:20", "class", "SOC 101-001"),
        (4, "15:30", "17:00", "deepwork", "Deep Work Block 2"),
        // Friday (5)
        (5, "07:00", "08:15", "fitness", "Soccer Practice"),
        (5, "09:00", "10:30", "deepwork", "Deep Work Block 1"),
        (5, "14:00", "14:50", "class", "LING 102-001"),
        (5, "17:30", "22:00", "social", "Social Time"),
        // Saturday (6)
        (6, "08:00", "09:15", "fitness", "Full Body Workout C"),
        (6, "10:00", "14:00", "project", "Personal Projects"),
        // Sunday (0)
        (0, "08:00", "09:00", "recovery", "Active Recovery"),
        (0, "12:00", "14:00", "review", "Academic Review"),
        (0, "18:00", "20:00", "planning", "Weekly Planning"),
    ];

    for (day, start, end, block_type, title) in schedule {
        sqlx::query(
            "INSERT INTO schedule_blocks (day_of_week, start_time, end_time, block_type, title, priority)
             VALUES (?, ?, ?, ?, ?, 1)"
        )
        .bind(day)
        .bind(start)
        .bind(end)
        .bind(block_type)
        .bind(title)
        .execute(&pool).await?;
    }

    println!("✅ Schedule imported successfully!");
    println!("   Run 'cargo run' and press 'c' to view calendar");

    Ok(())
}
