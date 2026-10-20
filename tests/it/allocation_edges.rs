//! Boundaries in `app/allocation.rs` that mutation testing showed no other test pins down.
//! Blocks are seeded on future weekdays, so none of this depends on the time of day it runs.
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveTime, Utc};
use sqlx::SqlitePool;
use triptych::app::{
    ALLOCATION_WINDOW_DAYS, AllocationResult, App, ConflictReason, TaskConflict,
    resolve_local_datetime,
};

use crate::app::{insert_task, insert_task_with_deadline, test_pool};

fn today() -> NaiveDate {
    Local::now().date_naive()
}

async fn add_block(pool: &SqlitePool, date: NaiveDate, start: &str, end: &str, kind: &str) {
    sqlx::query(
        "INSERT INTO schedule_blocks (day_of_week, start_time, end_time, block_type, title) VALUES (?, ?, ?, ?, 'block')",
    )
    .bind(i32::try_from(date.weekday().num_days_from_monday()).unwrap())
    .bind(start)
    .bind(end)
    .bind(kind)
    .execute(pool)
    .await
    .unwrap();
}

fn at(date: NaiveDate, hour: u32) -> DateTime<Utc> {
    resolve_local_datetime(date.and_time(NaiveTime::from_hms_opt(hour, 0, 0).unwrap()))
}

type Row = (i64, String, String, String, i64);

async fn allocations(pool: &SqlitePool) -> Vec<Row> {
    sqlx::query_as(
        "SELECT task_id, block_date, block_start_time, block_end_time, allocated_minutes FROM task_block_allocations ORDER BY task_id, block_date, block_start_time",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

fn row(task: i64, date: NaiveDate, start: &str, end: &str, minutes: i64) -> Row {
    (task, date.to_string(), start.into(), end.into(), minutes)
}

#[tokio::test]
async fn only_deepwork_and_admin_blocks_take_tasks() {
    let pool = test_pool().await;
    let day = today() + Duration::days(1);
    add_block(&pool, day, "08:00", "10:00", "class").await;
    add_block(&pool, day, "10:00", "11:00", "deepwork").await;
    add_block(&pool, day, "11:00", "12:00", "deepwork_input").await;
    add_block(&pool, day, "12:00", "13:00", "deepwork_output").await;
    add_block(&pool, day, "13:00", "14:00", "admin").await;
    let id = insert_task_with_deadline(&pool, "big", at(day + Duration::days(1), 23), 240).await;

    let result = App::new(pool.clone())
        .await
        .reallocate_all_tasks()
        .await
        .unwrap();

    assert!(result.conflicts.is_empty(), "{:?}", result.conflicts);
    assert_eq!(
        allocations(&pool).await,
        vec![
            row(id, day, "10:00", "11:00", 60),
            row(id, day, "11:00", "12:00", 60),
            row(id, day, "12:00", "13:00", 60),
            row(id, day, "13:00", "14:00", 60),
        ]
    );
}

#[tokio::test]
async fn a_block_counts_only_on_its_own_weekday() {
    let pool = test_pool().await;
    // Six days out is the weekday just before today's: every earlier date is the wrong weekday.
    let day = today() + Duration::days(6);
    add_block(&pool, day, "09:00", "10:00", "deepwork").await;
    let id = insert_task_with_deadline(&pool, "t", at(today() + Duration::days(10), 23), 60).await;

    App::new(pool.clone())
        .await
        .reallocate_all_tasks()
        .await
        .unwrap();

    assert_eq!(
        allocations(&pool).await,
        vec![row(id, day, "09:00", "10:00", 60)]
    );
}

#[tokio::test]
async fn a_block_that_already_ended_today_is_not_offered() {
    if Local::now().time() < NaiveTime::from_hms_opt(0, 2, 0).unwrap() {
        return; // the 00:00 to 00:01 block below has not ended yet
    }
    let pool = test_pool().await;
    add_block(&pool, today(), "00:00", "00:01", "deepwork").await;
    insert_task_with_deadline(&pool, "t", at(today() + Duration::days(3), 23), 1).await;

    let result = App::new(pool.clone())
        .await
        .reallocate_all_tasks()
        .await
        .unwrap();

    assert_eq!(result.conflicts.len(), 1);
    assert!(allocations(&pool).await.is_empty());
}

#[tokio::test]
async fn reallocating_drops_allocations_left_over_from_before() {
    let pool = test_pool().await;
    let id = insert_task(&pool, "no deadline").await;
    sqlx::query(
        "INSERT INTO task_block_allocations (task_id, block_date, block_start_time, block_end_time, allocated_minutes) VALUES (?, '2030-01-01', '09:00', '10:00', 60)",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();

    App::new(pool.clone())
        .await
        .reallocate_all_tasks()
        .await
        .unwrap();

    assert!(allocations(&pool).await.is_empty());
}

#[tokio::test]
async fn tasks_share_a_block_back_to_back_and_spill_into_the_next_one() {
    let pool = test_pool().await;
    let (d1, d2) = (today() + Duration::days(1), today() + Duration::days(2));
    add_block(&pool, d1, "09:00", "11:00", "deepwork").await;
    add_block(&pool, d2, "09:00", "11:00", "deepwork").await;
    let a = insert_task_with_deadline(&pool, "alpha", at(d1, 20), 90).await;
    let b = insert_task_with_deadline(&pool, "beta", at(d2, 20), 60).await;

    let result = App::new(pool.clone())
        .await
        .reallocate_all_tasks()
        .await
        .unwrap();

    // Exactly filling the need is not a conflict.
    assert!(result.conflicts.is_empty(), "{:?}", result.conflicts);
    assert_eq!(
        allocations(&pool).await,
        vec![
            row(a, d1, "09:00", "10:30", 90),
            row(b, d1, "10:30", "11:00", 30),
            row(b, d2, "09:00", "09:30", 30),
        ]
    );
}

#[tokio::test]
async fn a_task_that_does_not_fit_is_reported_with_what_it_did_get() {
    let pool = test_pool().await;
    let day = today() + Duration::days(1);
    add_block(&pool, day, "09:00", "10:00", "deepwork").await;
    let id = insert_task_with_deadline(&pool, "long", at(day, 20), 90).await;

    let result = App::new(pool.clone())
        .await
        .reallocate_all_tasks()
        .await
        .unwrap();

    assert_eq!(result.conflicts.len(), 1);
    let c = &result.conflicts[0];
    assert_eq!(
        (c.task_id, c.needed_minutes, c.allocated_minutes),
        (id, 90, 60)
    );
    assert_eq!(c.reason, ConflictReason::OutOfCapacity);
}

#[tokio::test]
async fn a_block_starting_exactly_at_the_deadline_is_too_late() {
    let pool = test_pool().await;
    let day = today() + Duration::days(1);
    add_block(&pool, day, "10:00", "11:00", "deepwork").await;
    let on_time = at(day, 10) + Duration::minutes(1);
    let id = insert_task_with_deadline(&pool, "t", at(day, 10), 60).await;

    App::new(pool.clone())
        .await
        .reallocate_all_tasks()
        .await
        .unwrap();
    assert_eq!(allocations(&pool).await, vec![]);

    sqlx::query("UPDATE tasks SET deadline = ? WHERE id = ?")
        .bind(on_time)
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    App::new(pool.clone())
        .await
        .reallocate_all_tasks()
        .await
        .unwrap();
    assert_eq!(
        allocations(&pool).await,
        vec![row(id, day, "10:00", "11:00", 60)]
    );
}

fn conflict(reason: ConflictReason) -> TaskConflict {
    TaskConflict {
        task_id: 1,
        description: "t".into(),
        needed_minutes: 60,
        allocated_minutes: 0,
        deadline: Utc::now(),
        reason,
    }
}

fn summary(reasons: Vec<ConflictReason>) -> Option<String> {
    AllocationResult {
        conflicts: reasons.into_iter().map(conflict).collect(),
    }
    .conflict_summary()
}

#[test]
fn the_summary_names_only_the_reasons_that_occurred() {
    assert_eq!(summary(vec![]), None);
    assert_eq!(
        summary(vec![ConflictReason::BeyondWindow]).as_deref(),
        Some(
            format!("1 task(s) not scheduled: 1 past the {ALLOCATION_WINDOW_DAYS}-day window")
                .as_str()
        )
    );
    assert_eq!(
        summary(vec![ConflictReason::OutOfCapacity]).as_deref(),
        Some("1 task(s) not scheduled: 1 out of block capacity")
    );
    assert_eq!(
        summary(vec![ConflictReason::BeyondWindow, ConflictReason::OutOfCapacity]).as_deref(),
        Some(
            format!("2 task(s) not scheduled: 1 past the {ALLOCATION_WINDOW_DAYS}-day window, 1 out of block capacity")
                .as_str()
        )
    );
}
