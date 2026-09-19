use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Utc};
use sqlx::SqlitePool;
use triptych::app::*;

#[test]
fn classify_task_picks_deepwork_for_coding_keywords() {
    assert_eq!(classify_task("implement the new parser"), "deepwork");
    assert_eq!(classify_task("leetcode grind"), "deepwork");
}

#[test]
fn classify_task_picks_admin_for_scheduling_keywords() {
    assert_eq!(classify_task("schedule dentist call"), "admin");
}

#[test]
fn classify_task_picks_learning_for_reading_keywords() {
    assert_eq!(classify_task("read chapter 3"), "learning");
}

#[test]
fn classify_task_falls_back_to_general() {
    assert_eq!(classify_task("buy groceries"), "general");
}

#[test]
fn default_duration_matches_category() {
    assert_eq!(default_duration_for_category("deepwork"), 90);
    assert_eq!(default_duration_for_category("admin"), 30);
    assert_eq!(default_duration_for_category("learning"), 60);
    assert_eq!(default_duration_for_category("general"), 60);
}

#[test]
fn resolve_local_datetime_roundtrips_a_normal_time() {
    let naive = NaiveDate::from_ymd_opt(2026, 3, 10)
        .unwrap()
        .and_hms_opt(9, 30, 0)
        .unwrap();
    let resolved = resolve_local_datetime(naive);
    let local = resolved.with_timezone(&chrono::Local);
    assert_eq!(local.naive_local(), naive);
}

#[test]
fn allocation_window_end_is_local_midnight_after_the_last_covered_day() {
    let today = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
    let expected = resolve_local_datetime(
        (today + Duration::days(ALLOCATION_WINDOW_DAYS))
            .and_hms_opt(0, 0, 0)
            .unwrap(),
    );
    assert_eq!(allocation_window_end(today), expected);
}

#[test]
fn classify_conflict_flags_deadline_past_the_allocation_window() {
    let today = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
    let window_end = allocation_window_end(today);
    let far_deadline = window_end + Duration::days(1);
    assert_eq!(
        classify_conflict(far_deadline, window_end),
        ConflictReason::BeyondWindow
    );
    // Boundary: a deadline exactly at window_end wasn't considered either
    // (get_available_deepwork_blocks stops the day before), so it counts
    // as beyond the window rather than a capacity shortfall.
    assert_eq!(
        classify_conflict(window_end, window_end),
        ConflictReason::BeyondWindow
    );
}

#[test]
fn classify_conflict_flags_capacity_when_deadline_is_inside_the_window() {
    let today = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
    let window_end = allocation_window_end(today);
    let near_deadline = window_end - Duration::days(1);
    assert_eq!(
        classify_conflict(near_deadline, window_end),
        ConflictReason::OutOfCapacity
    );
}

fn sample_conflict(reason: ConflictReason) -> TaskConflict {
    TaskConflict {
        task_id: 1,
        description: "sample".to_string(),
        needed_minutes: 90,
        allocated_minutes: 0,
        deadline: resolve_local_datetime(
            NaiveDate::from_ymd_opt(2026, 3, 10)
                .unwrap()
                .and_hms_opt(9, 0, 0)
                .unwrap(),
        ),
        reason,
    }
}

#[test]
fn conflict_summary_is_none_without_conflicts() {
    let result = AllocationResult::default();
    assert!(result.conflict_summary().is_none());
}

#[test]
fn conflict_summary_reports_both_reason_counts() {
    let result = AllocationResult {
        conflicts: vec![
            sample_conflict(ConflictReason::BeyondWindow),
            sample_conflict(ConflictReason::OutOfCapacity),
            sample_conflict(ConflictReason::OutOfCapacity),
        ],
    };
    let summary = result.conflict_summary().expect("has conflicts");
    assert!(summary.contains("3 task(s) not scheduled"));
    assert!(summary.contains("1 past the 14-day window"));
    assert!(summary.contains("2 out of block capacity"));
}

#[test]
fn parse_days_expands_special_groups() {
    assert_eq!(App::parse_days("weekdays").unwrap(), vec![0, 1, 2, 3, 4]);
    assert_eq!(App::parse_days("weekends").unwrap(), vec![5, 6]);
    assert_eq!(
        App::parse_days("everyday").unwrap(),
        vec![0, 1, 2, 3, 4, 5, 6]
    );
}

#[test]
fn parse_days_expands_compound_names() {
    assert_eq!(App::parse_days("monday_wednesday").unwrap(), vec![0, 2]);
}

#[test]
fn parse_days_rejects_unknown_names() {
    assert!(App::parse_days("someday").is_err());
}

#[test]
fn time_to_minutes_parses_hh_mm() {
    assert_eq!(App::time_to_minutes("09:30"), Some(570));
    assert_eq!(App::time_to_minutes("00:00"), Some(0));
    assert_eq!(App::time_to_minutes("23:59"), Some(1439));
}

#[test]
fn time_to_minutes_rejects_malformed_input() {
    assert_eq!(App::time_to_minutes("bogus"), None);
}

#[test]
fn validate_time_format_accepts_in_range_times() {
    assert!(App::validate_time_format("09:30").is_ok());
    assert!(App::validate_time_format("23:59").is_ok());
}

#[test]
fn validate_time_format_rejects_out_of_range_times() {
    assert!(App::validate_time_format("24:00").is_err());
    assert!(App::validate_time_format("09:60").is_err());
    assert!(App::validate_time_format("not-a-time").is_err());
}

#[test]
fn day_number_to_name_matches_monday_first_numbering() {
    assert_eq!(App::day_number_to_name(0), "monday");
    assert_eq!(App::day_number_to_name(6), "sunday");
}

#[test]
fn parse_time_string_handles_optional_seconds() {
    assert_eq!(
        parse_time_string("09:30"),
        NaiveTime::from_hms_opt(9, 30, 0)
    );
    assert_eq!(
        parse_time_string("09:30:15"),
        NaiveTime::from_hms_opt(9, 30, 15)
    );
    assert_eq!(parse_time_string("not-a-time"), None);
}

#[test]
fn cell_tasks_finds_manual_and_allocated_tasks() {
    let day = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
    let other_day = NaiveDate::from_ymd_opt(2026, 3, 11).unwrap();
    let manual_time = NaiveTime::from_hms_opt(9, 0, 0).unwrap();
    let alloc_time = NaiveTime::from_hms_opt(14, 0, 0).unwrap();

    let scheduled_tasks = vec![(day, manual_time, 1i64, "write report".to_string(), 2i32)];
    let task_allocations = vec![(day, alloc_time, 2i64, "study rust".to_string(), 90i32, 1i32)];

    let manual = cell_tasks(&scheduled_tasks, &task_allocations, day, 9)
        .into_iter()
        .next()
        .expect("manual task at 9am");
    assert_eq!(manual.id, 1);
    assert!(!manual.is_allocation);

    let alloc = cell_tasks(&scheduled_tasks, &task_allocations, day, 14)
        .into_iter()
        .next()
        .expect("allocation at 2pm");
    assert_eq!(alloc.id, 2);
    assert!(alloc.is_allocation);

    assert!(cell_tasks(&scheduled_tasks, &task_allocations, day, 10).is_empty());
    assert!(cell_tasks(&scheduled_tasks, &task_allocations, other_day, 9).is_empty());
}

/// Regression for the "deadline allocations only ever render in their
/// block's start hour" Known Issue: two tasks allocated into the same
/// block must land on different, sequential start times, not both stack
/// onto the block's own start.
#[test]
fn cell_tasks_separates_two_allocations_in_the_same_block() {
    let day = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
    let block_start = NaiveTime::from_hms_opt(9, 0, 0).unwrap();
    let second_start = NaiveTime::from_hms_opt(10, 0, 0).unwrap();
    let scheduled_tasks = vec![];
    // First task takes the block's first 60 minutes, second task the next 30.
    let task_allocations = vec![
        (
            day,
            block_start,
            1i64,
            "first task".to_string(),
            60i32,
            1i32,
        ),
        (
            day,
            second_start,
            2i64,
            "second task".to_string(),
            30i32,
            1i32,
        ),
    ];

    let at_9am = cell_tasks(&scheduled_tasks, &task_allocations, day, 9);
    assert_eq!(at_9am.len(), 1);
    assert_eq!(at_9am[0].id, 1);

    let at_10am = cell_tasks(&scheduled_tasks, &task_allocations, day, 10);
    assert_eq!(at_10am.len(), 1);
    assert_eq!(at_10am[0].id, 2);
}

/// A single-connection in-memory DB, fully migrated the same way `App::build`
/// migrates the real one, so these tests exercise the real read/write paths
/// instead of a hand-rolled stand-in schema.
async fn test_pool() -> SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("in-memory pool");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("base schema migration");
    triptych::migrations::run_calendar_migration(&pool)
        .await
        .expect("calendar schema migration");
    triptych::migrations::run_email_migration(&pool)
        .await
        .expect("email schema migration");
    pool
}

/// Inserts an email already converted into task `task_id`, returning the email row id.
async fn insert_linked_email(pool: &SqlitePool, task_id: i64) -> i64 {
    sqlx::query(
        "INSERT INTO email_messages (uid, message_id, from_addr, subject, date_utc, task_id) VALUES (1, 'm1', 'a@b.c', 'hi', '2026-01-01T00:00:00Z', ?)",
    )
    .bind(task_id)
    .execute(pool)
    .await
    .expect("insert email")
    .last_insert_rowid()
}

async fn email_task_id(pool: &SqlitePool, email_id: i64) -> Option<i64> {
    sqlx::query_scalar("SELECT task_id FROM email_messages WHERE id = ?")
        .bind(email_id)
        .fetch_one(pool)
        .await
        .expect("read email link")
}

async fn insert_task(pool: &SqlitePool, description: &str) -> i64 {
    sqlx::query(
        "INSERT INTO tasks (description, completed, item_order, priority) VALUES (?, false, 0, 1)",
    )
    .bind(description)
    .execute(pool)
    .await
    .expect("insert task")
    .last_insert_rowid()
}

async fn insert_task_with_deadline(
    pool: &SqlitePool,
    description: &str,
    deadline: DateTime<Utc>,
    duration_minutes: i32,
) -> i64 {
    sqlx::query(
        "INSERT INTO tasks (description, completed, item_order, priority, deadline, duration_minutes) VALUES (?, false, 0, 1, ?, ?)"
    )
    .bind(description)
    .bind(deadline)
    .bind(duration_minutes)
    .execute(pool)
    .await
    .expect("insert task with deadline")
    .last_insert_rowid()
}

/// A deadline task with no eligible schedule blocks at all always misses its
/// deadline; which `ConflictReason` it gets depends on whether the deadline
/// itself falls inside or beyond the allocation window - see
/// `reallocate_marks_far_deadline_as_beyond_window` and
/// `reallocate_marks_near_deadline_as_out_of_capacity` below.
#[tokio::test]
async fn reallocate_marks_far_deadline_as_beyond_window() {
    let pool = test_pool().await;
    let far_deadline = resolve_local_datetime(
        (chrono::Local::now().naive_local().date() + Duration::days(30))
            .and_hms_opt(9, 0, 0)
            .unwrap(),
    );
    insert_task_with_deadline(&pool, "distant report", far_deadline, 90).await;

    let mut app = App::new(pool).await;
    let result = app.reallocate_all_tasks().await.expect("reallocate");

    assert_eq!(result.conflicts.len(), 1);
    assert_eq!(result.conflicts[0].reason, ConflictReason::BeyondWindow);
}

#[tokio::test]
async fn reallocate_marks_near_deadline_as_out_of_capacity() {
    let pool = test_pool().await;
    let near_deadline = resolve_local_datetime(
        (chrono::Local::now().naive_local().date() + Duration::days(1))
            .and_hms_opt(9, 0, 0)
            .unwrap(),
    );
    insert_task_with_deadline(&pool, "urgent report", near_deadline, 90).await;

    let mut app = App::new(pool).await;
    let result = app.reallocate_all_tasks().await.expect("reallocate");

    assert_eq!(result.conflicts.len(), 1);
    assert_eq!(result.conflicts[0].reason, ConflictReason::OutOfCapacity);
}

/// Round-trips a task through pick-up/drop: the goal of the "move a
/// scheduled task in the calendar" half of Task 2. Regression guard for the
/// write path (`drop_held_task`, naive-local-as-UTC) and read path
/// (`get_scheduled_tasks_internal`) staying on the same convention - if they
/// ever drift, the task would land in the wrong cell after a reload.
#[tokio::test]
async fn drop_held_task_moves_task_to_selected_cell_and_reload_agrees() {
    let pool = test_pool().await;
    let task_id = insert_task(&pool, "write report").await;

    let mut app = App::new(pool).await;
    app.held_task = Some(task_id);
    app.calendar_week_offset = Some(0);
    app.selected_day = 2;
    app.selected_time_slot = 3; // 7 + 3 = 10:00

    app.drop_held_task().await.expect("drop task");

    let target_day = app.selected_cell_date();
    let cell = cell_tasks(
        &app.cached_scheduled_tasks,
        &app.cached_task_allocations,
        target_day,
        10,
    )
    .into_iter()
    .next()
    .expect("task lands in dropped cell");
    assert_eq!(cell.id, task_id);
    assert!(!cell.is_allocation);
    assert!(app.held_task.is_none());
}

/// Every calendar-grid write path (schedule/drop/add-at-cell/auto-schedule)
/// must store `scheduled_at` via `resolve_local_datetime`, the same
/// conversion `nlp::rules` uses for NLP-parsed times - not a naive
/// local-wall-clock value mislabeled as UTC. Pins the stored value itself
/// (not just grid-cell self-consistency) so the two write paths can't
/// silently drift back apart.
#[tokio::test]
async fn schedule_task_to_selected_cell_stores_true_utc_not_naive_local() {
    let pool = test_pool().await;
    let task_id = insert_task(&pool, "write report").await;

    let mut app = App::new(pool).await;
    app.load_tasks().await.expect("load tasks");
    app.calendar_week_offset = Some(0);
    app.selected_day = 2;
    app.selected_time_slot = 3; // 7 + 3 = 10:00

    app.schedule_task_to_selected_cell()
        .await
        .expect("schedule task");

    let task = app
        .get_task_by_id(task_id)
        .await
        .expect("query task")
        .expect("task exists");
    let expected =
        resolve_local_datetime(app.selected_cell_date().and_time(app.selected_cell_time()));
    assert_eq!(task.scheduled_at, Some(expected));
}

/// Round-trips a task through schedule -> unschedule: it must disappear from
/// the calendar cell and reappear in the todolist's unscheduled pool - the
/// other half of "reflect in the todolist".
#[tokio::test]
async fn unschedule_task_clears_cell_and_returns_task_to_unscheduled_pool() {
    let pool = test_pool().await;
    let task_id = insert_task(&pool, "study rust").await;

    let mut app = App::new(pool).await;
    app.calendar_week_offset = Some(0);
    app.selected_day = 1;
    app.selected_time_slot = 0; // 7:00
    app.held_task = Some(task_id);
    app.drop_held_task().await.expect("schedule task");

    let day = app.selected_cell_date();
    assert!(
        !cell_tasks(
            &app.cached_scheduled_tasks,
            &app.cached_task_allocations,
            day,
            7
        )
        .is_empty()
    );

    app.unschedule_task_at_selected_cell()
        .await
        .expect("unschedule task");

    assert!(
        cell_tasks(
            &app.cached_scheduled_tasks,
            &app.cached_task_allocations,
            day,
            7
        )
        .is_empty()
    );
    assert!(app.unscheduled_tasks().iter().any(|t| t.id == task_id));
}

/// Editing a deadline from the calendar (the "move a deadline" half of
/// Task 2) must persist through the same regex parser `add_task` uses, and
/// show up on reload - it must not just update the DB row silently.
#[tokio::test]
async fn submit_deadline_edit_persists_parsed_deadline_and_reloads_task() {
    let pool = test_pool().await;
    let task_id = insert_task(&pool, "renew license").await;

    let mut app = App::new(pool).await;
    app.deadline_edit_task_id = Some(task_id);
    app.input_buffer = "tomorrow".to_string();

    app.submit_deadline_edit();
    let parsed = app
        .deadline_rx
        .recv()
        .await
        .expect("background parse result");
    app.apply_deadline_parse(parsed)
        .await
        .expect("apply deadline parse");

    let task = app
        .get_task_by_id(task_id)
        .await
        .expect("query task")
        .expect("task exists");
    let deadline = task.deadline.expect("deadline parsed and saved");

    let expected_date = chrono::Local::now().naive_local().date() + Duration::days(1);
    assert_eq!(
        deadline.with_timezone(&chrono::Local).date_naive(),
        expected_date
    );
}

#[tokio::test]
async fn delete_task_unlinks_the_email_it_came_from() {
    let pool = test_pool().await;
    let task_id = insert_task(&pool, "reply to advisor").await;
    let email_id = insert_linked_email(&pool, task_id).await;

    let mut app = App::new(pool.clone()).await;
    app.load_tasks().await.expect("load tasks");
    app.delete_task().await.expect("delete linked task");

    assert!(app.get_task_by_id(task_id).await.expect("query").is_none());
    assert_eq!(email_task_id(&pool, email_id).await, None);
}

#[tokio::test]
async fn remove_task_by_id_unlinks_the_email_it_came_from() {
    let pool = test_pool().await;
    let task_id = insert_task(&pool, "reply to advisor").await;
    let email_id = insert_linked_email(&pool, task_id).await;

    let mut app = App::new(pool.clone()).await;
    assert!(
        app.remove_task_by_id(task_id)
            .await
            .expect("remove linked task")
    );
    assert_eq!(email_task_id(&pool, email_id).await, None);
}

#[tokio::test]
async fn clear_completed_tasks_unlinks_the_emails_they_came_from() {
    let pool = test_pool().await;
    let task_id = insert_task(&pool, "reply to advisor").await;
    sqlx::query("UPDATE tasks SET completed = 1 WHERE id = ?")
        .bind(task_id)
        .execute(&pool)
        .await
        .expect("complete task");
    let email_id = insert_linked_email(&pool, task_id).await;

    let mut app = App::new(pool.clone()).await;
    assert_eq!(
        app.clear_completed_tasks().await.expect("clear completed"),
        1
    );
    assert_eq!(email_task_id(&pool, email_id).await, None);
}

async fn app_with_tasks(descriptions: &[&str]) -> (App, SqlitePool) {
    let pool = test_pool().await;
    for (i, d) in descriptions.iter().enumerate() {
        sqlx::query("INSERT INTO tasks (description, completed, item_order, priority) VALUES (?, false, ?, 1)")
            .bind(d)
            .bind(i64::try_from(i).expect("small index"))
            .execute(&pool)
            .await
            .expect("insert task");
    }
    let mut app = App::new(pool.clone()).await;
    app.load_tasks().await.expect("load tasks");
    (app, pool)
}

fn descriptions(app: &App) -> Vec<&str> {
    app.tasks.iter().map(|t| t.description.as_str()).collect()
}

#[tokio::test]
async fn visual_range_spans_anchor_to_cursor_in_either_direction() {
    let (mut app, _pool) = app_with_tasks(&["a", "b", "c", "d"]).await;
    assert_eq!(app.visual_range(), None);

    app.selected = 2;
    app.toggle_visual();
    assert_eq!(app.visual_range(), Some(2..=2));
    app.selected = 3;
    assert_eq!(app.visual_range(), Some(2..=3));
    app.selected = 0;
    assert_eq!(app.visual_range(), Some(0..=2));

    app.toggle_visual();
    assert_eq!(app.visual_range(), None);
}

#[tokio::test]
async fn toggle_visual_does_nothing_on_an_empty_list() {
    let (mut app, _pool) = app_with_tasks(&[]).await;
    app.toggle_visual();
    assert_eq!(app.visual_anchor, None);
}

#[tokio::test]
async fn delete_selected_tasks_removes_the_whole_range() {
    let (mut app, _pool) = app_with_tasks(&["a", "b", "c", "d"]).await;
    app.selected = 1;
    app.toggle_visual();
    app.selected = 2;

    app.delete_selected_tasks().await.expect("delete range");

    assert_eq!(descriptions(&app), ["a", "d"]);
    assert_eq!(app.visual_anchor, None);
    assert_eq!(app.selected, 1);
}

#[tokio::test]
async fn delete_selected_tasks_at_the_end_clamps_the_cursor() {
    let (mut app, _pool) = app_with_tasks(&["a", "b", "c"]).await;
    app.selected = 2;
    app.toggle_visual();
    app.selected = 1;

    app.delete_selected_tasks().await.expect("delete range");

    assert_eq!(descriptions(&app), ["a"]);
    assert_eq!(app.selected, 0);
}

#[tokio::test]
async fn delete_selected_tasks_without_a_selection_deletes_only_the_cursor_row() {
    let (mut app, _pool) = app_with_tasks(&["a", "b", "c"]).await;
    app.selected = 1;

    app.delete_selected_tasks().await.expect("delete one");

    assert_eq!(descriptions(&app), ["a", "c"]);
}
