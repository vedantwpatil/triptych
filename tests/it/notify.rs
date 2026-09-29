//! `notify`: deadline alert tiers, batching and idempotence.

use chrono::{DateTime, Duration, Utc};
use sqlx::SqlitePool;
use triptych::notify::due_alerts;

async fn test_pool() -> SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("in-memory pool");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("base schema");
    triptych::migrations::run_calendar_migration(&pool)
        .await
        .expect("calendar schema");
    pool
}

async fn add(pool: &SqlitePool, desc: &str, deadline: Option<DateTime<Utc>>, done: bool) {
    sqlx::query(
        "INSERT INTO tasks (description, completed, item_order, priority, deadline) VALUES (?, ?, 0, 1, ?)",
    )
    .bind(desc)
    .bind(done)
    .bind(deadline)
    .execute(pool)
    .await
    .expect("insert");
}

#[tokio::test]
async fn alerts_once_per_tier_and_skips_far_done_and_overdue_tasks() {
    let pool = test_pool().await;
    let now = Utc::now();
    add(&pool, "soon", Some(now + Duration::hours(10)), false).await;
    add(&pool, "far", Some(now + Duration::days(3)), false).await;
    add(&pool, "done", Some(now + Duration::hours(2)), true).await;
    add(&pool, "late", Some(now - Duration::hours(2)), false).await;
    add(&pool, "none", None, false).await;

    let first = due_alerts(&pool, now).await.unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].title, "Due within 24 hours");
    assert!(first[0].body.contains("soon (in 10h 0m)"));
    assert!(
        due_alerts(&pool, now).await.unwrap().is_empty(),
        "no repeat"
    );

    // Closer in, the same task alerts again at the 1-hour tier, once.
    let later = now + Duration::minutes(570);
    let second = due_alerts(&pool, later).await.unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].title, "Due within 1 hour");
    assert!(due_alerts(&pool, later).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_moved_deadline_alerts_again_and_long_lists_are_batched() {
    let pool = test_pool().await;
    let now = Utc::now();
    for i in 0..5 {
        add(
            &pool,
            &format!("t{i}"),
            Some(now + Duration::minutes(30 + i)),
            false,
        )
        .await;
    }
    let alerts = due_alerts(&pool, now).await.unwrap();
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].body.lines().count(), 4);
    assert!(alerts[0].body.ends_with("+2 more"));

    sqlx::query("UPDATE tasks SET deadline = ?, notified_tier = 0 WHERE description = 't0'")
        .bind(now + Duration::minutes(20))
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(due_alerts(&pool, now).await.unwrap().len(), 1);
}
