//! Boundaries in `app/tasks.rs` that mutation testing showed no other test pins down.
use chrono::{Duration, TimeZone, Utc};
use sqlx::SqlitePool;
use triptych::app::{App, classify_task, extract_task_fields};
use triptych::nlp::types::{Event, ParsedItem};

use crate::app::{insert_task, task_count, test_pool};

#[test]
fn every_keyword_alone_picks_its_category() {
    for word in [
        "leetcode",
        "project",
        "code",
        "implement",
        "study",
        "homework",
    ] {
        assert_eq!(classify_task(word), "deepwork", "{word}");
    }
    for word in ["schedule", "call", "quick"] {
        assert_eq!(classify_task(word), "admin", "{word}");
    }
    for word in ["read", "watch", "learn", "review"] {
        assert_eq!(classify_task(word), "learning", "{word}");
    }
}

fn event_ending_after(minutes: Option<i64>) -> ParsedItem {
    let start = Utc.with_ymd_and_hms(2030, 1, 1, 9, 0, 0).unwrap();
    ParsedItem::Event(Event {
        title: "Sync".into(),
        start_time: start,
        end_time: minutes.map(|m| start + Duration::minutes(m)),
        location: None,
        tags: vec![],
    })
}

#[test]
fn an_event_lasts_only_a_positive_gap_to_its_end() {
    let duration = |m| extract_task_fields(event_ending_after(m)).5;
    assert_eq!(duration(Some(45)), Some(45));
    assert_eq!(duration(Some(1)), Some(1));
    assert_eq!(duration(Some(0)), None);
    assert_eq!(duration(Some(-30)), None);
    assert_eq!(duration(None), None);
}

async fn insert_ordered(pool: &SqlitePool, description: &str, order: i64) {
    sqlx::query(
        "INSERT INTO tasks (description, completed, item_order, priority) VALUES (?, false, ?, 1)",
    )
    .bind(description)
    .bind(order)
    .execute(pool)
    .await
    .unwrap();
}

async fn order_snapshot(pool: &SqlitePool) -> Vec<(String, i64)> {
    sqlx::query_as("SELECT description, item_order FROM tasks ORDER BY item_order")
        .fetch_all(pool)
        .await
        .unwrap()
}

fn rows(expected: &[(&str, i64)]) -> Vec<(String, i64)> {
    expected.iter().map(|(d, o)| ((*d).into(), *o)).collect()
}

#[tokio::test]
async fn a_task_added_at_the_top_goes_first_and_pushes_the_rest_down() {
    let pool = test_pool().await;
    for (i, name) in (0_i64..).zip(["a", "b", "c"]) {
        insert_ordered(&pool, name, i).await;
    }
    let mut app = App::new(pool.clone()).await;
    app.load_tasks().await.unwrap();
    app.selected = 0;

    app.add_task("fresh").await.unwrap();

    assert_eq!(
        order_snapshot(&pool).await,
        rows(&[("fresh", 0), ("a", 1), ("b", 2), ("c", 3)])
    );
    assert_eq!(app.selected, 0);
}

#[tokio::test]
async fn a_task_added_mid_list_lands_right_after_the_cursor_and_is_selected() {
    let pool = test_pool().await;
    for (i, name) in (0_i64..).zip(["t0", "t1", "t2", "t3", "t4"]) {
        insert_ordered(&pool, name, i).await;
    }
    let mut app = App::new(pool.clone()).await;
    app.load_tasks().await.unwrap();
    app.selected = 2;

    app.add_task("fresh").await.unwrap();

    assert_eq!(
        order_snapshot(&pool).await,
        rows(&[
            ("t0", 0),
            ("t1", 1),
            ("t2", 2),
            ("fresh", 3),
            ("t3", 4),
            ("t4", 5)
        ])
    );
    assert_eq!(app.tasks[app.selected].description, "fresh");
}

fn status(app: &App) -> &str {
    app.status_message.as_ref().map_or("", |(m, _)| m.as_str())
}

#[tokio::test]
async fn deleting_a_range_reports_how_many_went() {
    let pool = test_pool().await;
    for (i, name) in (0_i64..).zip(["a", "b", "c"]) {
        insert_ordered(&pool, name, i).await;
    }
    let mut app = App::new(pool.clone()).await;
    app.load_tasks().await.unwrap();
    app.toggle_visual();
    app.selected = 1;

    app.delete_selected_tasks().await.unwrap();

    assert_eq!(status(&app), "Deleted 2 task(s)");
    assert_eq!(task_count(&pool).await, 1);
}

#[tokio::test]
async fn deleting_a_range_says_how_many_canvas_rows_stayed() {
    let pool = test_pool().await;
    let canvas = insert_task(&pool, "Essay").await;
    insert_task(&pool, "Manual").await;
    sqlx::query("UPDATE tasks SET external_id = 'canvas-1' WHERE id = ?")
        .bind(canvas)
        .execute(&pool)
        .await
        .unwrap();
    let mut app = App::new(pool.clone()).await;
    app.load_tasks().await.unwrap();
    app.toggle_visual();
    app.selected = app.tasks.len() - 1;

    app.delete_selected_tasks().await.unwrap();

    assert_eq!(status(&app), "Deleted 1 task(s), kept 1 from Canvas");
    assert_eq!(task_count(&pool).await, 1);
}

async fn plant_stale_allocation(pool: &SqlitePool, task_id: i64) {
    sqlx::query(
        "INSERT INTO task_block_allocations (task_id, block_date, block_start_time, block_end_time, allocated_minutes) VALUES (?, '2030-01-01', '09:00', '10:00', 60)",
    )
    .bind(task_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn allocation_rows(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM task_block_allocations")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Reallocation wipes `task_block_allocations`, so a surviving planted row proves it did not run.
#[tokio::test]
async fn completing_an_unknown_id_changes_nothing_and_skips_reallocation() {
    let pool = test_pool().await;
    let id = insert_task(&pool, "open").await;
    plant_stale_allocation(&pool, id).await;
    let mut app = App::new(pool.clone()).await;

    assert!(!app.complete_task_by_id(9999).await.unwrap());
    assert_eq!(allocation_rows(&pool).await, 1);

    assert!(app.complete_task_by_id(id).await.unwrap());
    assert_eq!(allocation_rows(&pool).await, 0);
}

#[tokio::test]
async fn clearing_completed_tasks_reallocates_only_when_something_was_removed() {
    let pool = test_pool().await;
    let open = insert_task(&pool, "open").await;
    plant_stale_allocation(&pool, open).await;
    let mut app = App::new(pool.clone()).await;

    assert_eq!(app.clear_completed_tasks().await.unwrap(), 0);
    assert_eq!(allocation_rows(&pool).await, 1);

    let done = insert_task(&pool, "done").await;
    sqlx::query("UPDATE tasks SET completed = true WHERE id = ?")
        .bind(done)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(app.clear_completed_tasks().await.unwrap(), 1);
    assert_eq!(allocation_rows(&pool).await, 0);
}
