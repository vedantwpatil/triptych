use chrono::{DateTime, Duration, NaiveDate, NaiveTime, TimeZone, Utc};
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

    let scheduled_tasks = vec![(
        day,
        manual_time,
        1i64,
        "write report".to_string(),
        60i32,
        2i32,
    )];
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

/// KI-22: a span shows in every hour cell it overlaps, including a start that is not on the hour.
#[test]
fn span_covers_every_hour_it_overlaps() {
    let t = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();
    let hours = |start, minutes| {
        (0..24)
            .filter(|&h| span_covers_hour(start, minutes, h))
            .collect::<Vec<_>>()
    };
    assert_eq!(hours(t(7, 0), 180), [7, 8, 9]);
    assert_eq!(hours(t(12, 30), 60), [12, 13]);
    assert_eq!(hours(t(9, 0), 60), [9]);
    assert_eq!(hours(t(9, 45), 15), [9]);
    assert_eq!(
        hours(t(18, 0), 0),
        [18],
        "zero duration still marks its start hour"
    );
    assert_eq!(
        hours(t(23, 0), 120),
        [23],
        "never wraps past midnight onto the morning"
    );
}

/// KI-22: a manually scheduled 3h task is reachable (`u`/`m`/`e`) from each of its hours.
#[test]
fn cell_tasks_spans_a_manual_task_over_its_duration() {
    let day = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
    let start = NaiveTime::from_hms_opt(7, 0, 0).unwrap();
    let scheduled_tasks = vec![(day, start, 1i64, "lab report".to_string(), 180i32, 2i32)];
    for hour in [7, 8, 9] {
        let hit = cell_tasks(&scheduled_tasks, &[], day, hour);
        assert_eq!(hit.len(), 1, "hour {hour}");
        assert!(!hit[0].is_allocation);
    }
    assert!(cell_tasks(&scheduled_tasks, &[], day, 10).is_empty());
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

#[tokio::test]
async fn submit_task_inserts_only_when_the_parse_lands() {
    let (mut app, pool) = app_with_tasks(&[]).await;
    app.submit_task("buy milk".to_string(), None, None, None);
    assert!(app.tasks.is_empty(), "submit_task must not insert inline");

    let parsed = app.task_rx.recv().await.expect("background parse result");
    app.apply_task_parse(parsed).await.expect("apply parse");
    assert_eq!(descriptions(&app), ["buy milk"]);
    let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(stored, 1);
}

#[tokio::test]
async fn converting_an_email_twice_before_the_parse_lands_makes_one_task() {
    let pool = test_pool().await;
    let email_id = sqlx::query(
        "INSERT INTO email_messages (uid, message_id, from_addr, subject, date_utc) VALUES (1, 'm1', 'a@b.c', 'reply to advisor', '2026-01-01T00:00:00Z')",
    )
    .execute(&pool)
    .await
    .expect("insert email")
    .last_insert_rowid();
    let mut app = App::new(pool.clone()).await;
    app.refresh_emails().await.expect("load emails");

    app.convert_selected_email_to_task().await;
    app.convert_selected_email_to_task().await;
    for _ in 0..2 {
        let parsed = app.task_rx.recv().await.expect("background parse result");
        app.apply_task_parse(parsed).await.expect("apply parse");
    }

    assert_eq!(descriptions(&app), ["reply to advisor"]);
    assert_eq!(email_task_id(&pool, email_id).await, Some(app.tasks[0].id));
}

#[tokio::test]
async fn converting_an_email_extracts_a_deadline_from_the_body_but_keeps_the_subject_as_title() {
    let pool = test_pool().await;
    sqlx::query(
        "INSERT INTO email_messages (uid, message_id, from_addr, subject, snippet, date_utc) VALUES (1, 'm1', 'a@b.c', 'project update', 'please finish this by tomorrow', '2026-01-01T00:00:00Z')",
    )
    .execute(&pool)
    .await
    .expect("insert email");
    let mut app = App::new(pool.clone()).await;
    app.refresh_emails().await.expect("load emails");

    app.convert_selected_email_to_task().await;
    let parsed = app.task_rx.recv().await.expect("background parse result");
    app.apply_task_parse(parsed).await.expect("apply parse");

    assert_eq!(descriptions(&app), ["project update"]);
    assert!(
        app.tasks[0].deadline.is_some(),
        "the body's date phrase should still set a deadline"
    );
}

#[tokio::test]
async fn converting_an_email_finds_a_deadline_deep_in_the_body_when_the_snippet_has_none() {
    let pool = test_pool().await;
    sqlx::query(
        "INSERT INTO email_messages (uid, message_id, from_addr, subject, body_text, date_utc) VALUES (1, 'm1', 'a@b.c', 'quarterly report', 'Hey team,\n\nJust a heads up this needs to be finished by next friday.\n\nThanks.', '2026-01-01T00:00:00Z')",
    )
    .execute(&pool)
    .await
    .expect("insert email");
    let mut app = App::new(pool.clone()).await;
    app.refresh_emails().await.expect("load emails");

    app.convert_selected_email_to_task().await;
    let parsed = app.task_rx.recv().await.expect("background parse result");
    app.apply_task_parse(parsed).await.expect("apply parse");

    assert_eq!(descriptions(&app), ["quarterly report"]);
    assert!(
        app.tasks[0].deadline.is_some(),
        "a deadline stated only in the full body (past the snippet) should still be found"
    );
}

#[tokio::test]
async fn converting_an_email_ignores_a_deadline_phrase_inside_a_quoted_reply() {
    let pool = test_pool().await;
    sqlx::query(
        "INSERT INTO email_messages (uid, message_id, from_addr, subject, body_text, date_utc) VALUES (1, 'm1', 'a@b.c', 'quick question', 'Hey, just checking in on this - no urgency from me.\n\nOn Mon, Jan 5, 2026 at 3:00 PM Jane Doe <jane@example.com> wrote:\n> Please respond due tomorrow at the latest.\n> Thanks!', '2026-01-01T00:00:00Z')",
    )
    .execute(&pool)
    .await
    .expect("insert email");
    let mut app = App::new(pool.clone()).await;
    app.refresh_emails().await.expect("load emails");

    app.convert_selected_email_to_task().await;
    let parsed = app.task_rx.recv().await.expect("background parse result");
    app.apply_task_parse(parsed).await.expect("apply parse");

    assert_eq!(descriptions(&app), ["quick question"]);
    assert!(
        app.tasks[0].deadline.is_none(),
        "a deadline phrase inside a quoted-reply chain must not leak into the task"
    );
}

#[tokio::test]
async fn refresh_emails_keeps_the_cursor_on_its_message_when_newer_mail_arrives() {
    let pool = test_pool().await;
    let insert = |uid: i64, date: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO email_messages (uid, message_id, from_addr, subject, date_utc) VALUES (?, ?, 'a@b.c', ?, ?)",
            )
            .bind(uid)
            .bind(format!("m{uid}"))
            .bind(format!("subject {uid}"))
            .bind(date)
            .execute(&pool)
            .await
            .expect("insert email");
        }
    };
    insert(1, "2026-01-02T00:00:00Z").await;
    insert(2, "2026-01-01T00:00:00Z").await;
    let mut app = App::new(pool.clone()).await;
    app.refresh_emails().await.expect("load emails");
    app.selected_email = 1;

    insert(3, "2026-01-03T00:00:00Z").await;
    app.refresh_emails().await.expect("reload emails");

    assert_eq!(app.emails.len(), 3);
    assert_eq!(app.emails[app.selected_email].subject, "subject 2");
}

async fn insert_email(pool: &SqlitePool, uid: i64, subject: &str, date: &str) {
    sqlx::query(
        "INSERT INTO email_messages (uid, message_id, from_addr, subject, date_utc) VALUES (?, ?, 'a@b.c', ?, ?)",
    )
    .bind(uid)
    .bind(format!("m{uid}"))
    .bind(subject)
    .bind(date)
    .execute(pool)
    .await
    .expect("insert email");
}

fn subjects(app: &App) -> Vec<&str> {
    app.emails.iter().map(|e| e.subject.as_str()).collect()
}

#[tokio::test]
async fn emails_list_by_priority_then_date_and_the_toggle_restores_date_order() {
    let pool = test_pool().await;
    insert_email(&pool, 1, "lunch", "2026-01-03T00:00:00Z").await;
    insert_email(&pool, 2, "urgent: server", "2026-01-01T00:00:00Z").await;
    insert_email(&pool, 3, "assignment 2", "2026-01-02T00:00:00Z").await;
    insert_email(&pool, 4, "coffee", "2026-01-04T00:00:00Z").await;
    let mut app = App::new(pool).await;

    app.refresh_emails().await.expect("load emails");
    assert_eq!(
        subjects(&app),
        ["urgent: server", "assignment 2", "coffee", "lunch"]
    );

    app.toggle_email_sort().await.expect("toggle");
    assert_eq!(
        subjects(&app),
        ["coffee", "lunch", "assignment 2", "urgent: server"]
    );
}

#[tokio::test]
async fn opening_an_email_does_not_reorder_the_priority_list() {
    let pool = test_pool().await;
    insert_email(&pool, 1, "urgent: server", "2026-01-01T00:00:00Z").await;
    insert_email(&pool, 2, "coffee", "2026-01-02T00:00:00Z").await;
    let mut app = App::new(pool).await;
    app.refresh_emails().await.expect("load emails");

    app.open_selected_email().await.expect("open");

    assert_eq!(subjects(&app), ["urgent: server", "coffee"]);
    assert!(app.emails[0].is_read);
}

#[tokio::test]
async fn toggling_the_sort_keeps_the_cursor_on_its_message() {
    let pool = test_pool().await;
    insert_email(&pool, 1, "urgent: server", "2026-01-01T00:00:00Z").await;
    insert_email(&pool, 2, "coffee", "2026-01-02T00:00:00Z").await;
    let mut app = App::new(pool).await;
    app.refresh_emails().await.expect("load emails");
    assert_eq!(app.emails[app.selected_email].subject, "urgent: server");

    app.toggle_email_sort().await.expect("toggle");

    assert_eq!(app.emails[app.selected_email].subject, "urgent: server");
}

#[tokio::test]
async fn marking_unread_after_viewing_flips_is_read_back_to_false() {
    let pool = test_pool().await;
    insert_email(&pool, 1, "notes", "2026-01-01T00:00:00Z").await;
    let mut app = App::new(pool).await;
    app.refresh_emails().await.expect("load emails");
    app.mark_selected_email_read().await.expect("read");
    assert!(app.emails[app.selected_email].is_read);

    app.mark_selected_email_unread().await.expect("unread");

    assert!(!app.emails[app.selected_email].is_read);
}

#[tokio::test]
async fn toggling_star_flips_the_flag_and_toggling_again_clears_it() {
    let pool = test_pool().await;
    insert_email(&pool, 1, "notes", "2026-01-01T00:00:00Z").await;
    let mut app = App::new(pool).await;
    app.refresh_emails().await.expect("load emails");
    assert!(!app.emails[app.selected_email].is_starred);

    app.toggle_selected_star().await.expect("star");
    assert!(app.emails[app.selected_email].is_starred);

    app.toggle_selected_star().await.expect("unstar");
    assert!(!app.emails[app.selected_email].is_starred);
}

#[test]
fn next_category_walks_the_palette_and_wraps_to_none() {
    use triptych::app::next_category;

    assert_eq!(next_category(None).as_deref(), Some("red"));
    assert_eq!(next_category(Some("red")).as_deref(), Some("orange"));
    assert_eq!(next_category(Some("purple")), None);
    // An unrecognized value (shouldn't happen in practice) restarts at the beginning.
    assert_eq!(next_category(Some("not-a-color")).as_deref(), Some("red"));
}

#[tokio::test]
async fn cycling_category_walks_the_fixed_palette_and_wraps_to_untagged() {
    let pool = test_pool().await;
    insert_email(&pool, 1, "notes", "2026-01-01T00:00:00Z").await;
    let mut app = App::new(pool).await;
    app.refresh_emails().await.expect("load emails");
    assert_eq!(app.emails[app.selected_email].category, None);

    for expected in triptych::app::CATEGORY_ORDER {
        app.cycle_selected_category().await.expect("cycle");
        assert_eq!(
            app.emails[app.selected_email].category.as_deref(),
            Some(expected)
        );
    }

    app.cycle_selected_category().await.expect("wrap");
    assert_eq!(app.emails[app.selected_email].category, None);
}

async fn insert_email_for_account(pool: &SqlitePool, uid: i64, subject: &str, account: &str) {
    sqlx::query(
        "INSERT INTO email_messages (uid, message_id, account, from_addr, subject, date_utc) VALUES (?, ?, ?, 'a@b.c', ?, '2026-01-01T00:00:00Z')",
    )
    .bind(uid)
    .bind(format!("m{uid}"))
    .bind(account)
    .bind(subject)
    .execute(pool)
    .await
    .expect("insert email");
}

#[tokio::test]
async fn refresh_emails_only_loads_the_active_account_filter() {
    let pool = test_pool().await;
    insert_email_for_account(&pool, 1, "work memo", "work").await;
    insert_email_for_account(&pool, 2, "personal note", "personal").await;
    let mut app = App::new(pool).await;
    app.account_filter = Some("work".to_string());

    app.refresh_emails().await.expect("load emails");

    assert_eq!(subjects(&app), ["work memo"]);
}

#[tokio::test]
async fn cycle_account_filter_walks_every_account_then_back_to_merged() {
    let pool = test_pool().await;
    insert_email_for_account(&pool, 1, "work memo", "personal").await;
    insert_email_for_account(&pool, 2, "personal note", "work").await;
    let mut app = App::new(pool).await;
    app.refresh_emails().await.expect("load emails");
    assert_eq!(app.account_filter, None);

    app.cycle_account_filter().await.expect("cycle");
    assert_eq!(app.account_filter.as_deref(), Some("personal"));
    assert_eq!(subjects(&app), ["work memo"]);

    app.cycle_account_filter().await.expect("cycle");
    assert_eq!(app.account_filter.as_deref(), Some("work"));
    assert_eq!(subjects(&app), ["personal note"]);

    app.cycle_account_filter().await.expect("cycle");
    assert_eq!(app.account_filter, None);
    assert_eq!(app.emails.len(), 2);
}

async fn insert_email_for_folder(pool: &SqlitePool, uid: i64, subject: &str, folder: &str) {
    sqlx::query(
        "INSERT INTO email_messages (uid, message_id, folder, from_addr, subject, date_utc) VALUES (?, ?, ?, 'a@b.c', ?, '2026-01-01T00:00:00Z')",
    )
    .bind(uid)
    .bind(format!("m{uid}"))
    .bind(folder)
    .bind(subject)
    .execute(pool)
    .await
    .expect("insert email");
}

#[tokio::test]
async fn refresh_emails_only_loads_the_active_folder_filter() {
    let pool = test_pool().await;
    insert_email_for_folder(&pool, 1, "inbox memo", "INBOX").await;
    insert_email_for_folder(&pool, 2, "archived note", "Archive").await;
    let mut app = App::new(pool).await;
    app.folder_filter = Some("Archive".to_string());

    app.refresh_emails().await.expect("load emails");

    assert_eq!(subjects(&app), ["archived note"]);
}

#[tokio::test]
async fn cycle_folder_filter_walks_every_folder_then_back_to_merged() {
    let pool = test_pool().await;
    insert_email_for_folder(&pool, 1, "archived note", "Archive").await;
    insert_email_for_folder(&pool, 2, "inbox memo", "INBOX").await;
    let mut app = App::new(pool).await;
    app.refresh_emails().await.expect("load emails");
    assert_eq!(app.folder_filter, None);

    app.cycle_folder_filter().await.expect("cycle");
    assert_eq!(app.folder_filter.as_deref(), Some("Archive"));
    assert_eq!(subjects(&app), ["archived note"]);

    app.cycle_folder_filter().await.expect("cycle");
    assert_eq!(app.folder_filter.as_deref(), Some("INBOX"));
    assert_eq!(subjects(&app), ["inbox memo"]);

    app.cycle_folder_filter().await.expect("cycle");
    assert_eq!(app.folder_filter, None);
    assert_eq!(app.emails.len(), 2);
}

#[tokio::test]
async fn cycle_attachment_filter_walks_has_then_lacks_then_back_to_merged() {
    let pool = test_pool().await;
    insert_email_for_account(&pool, 1, "with attachment", "work").await;
    insert_email_for_account(&pool, 2, "plain note", "work").await;
    sqlx::query(
        "INSERT INTO email_attachments (email_id, part_index, filename, content_type, size_bytes)
         SELECT id, 0, 'report.pdf', 'application/pdf', 1024 FROM email_messages WHERE uid = 1",
    )
    .execute(&pool)
    .await
    .expect("insert attachment");
    let mut app = App::new(pool).await;
    app.refresh_emails().await.expect("load emails");
    assert_eq!(app.attachment_filter, None);

    app.cycle_attachment_filter().await.expect("cycle");
    assert_eq!(app.attachment_filter, Some(true));
    assert_eq!(subjects(&app), ["with attachment"]);

    app.cycle_attachment_filter().await.expect("cycle");
    assert_eq!(app.attachment_filter, Some(false));
    assert_eq!(subjects(&app), ["plain note"]);

    app.cycle_attachment_filter().await.expect("cycle");
    assert_eq!(app.attachment_filter, None);
    assert_eq!(app.emails.len(), 2);
}

#[tokio::test]
async fn cycle_unread_filter_walks_unread_then_read_then_back_to_merged() {
    let pool = test_pool().await;
    insert_email_for_account(&pool, 1, "unread memo", "work").await;
    insert_email_for_account(&pool, 2, "read memo", "work").await;
    sqlx::query("UPDATE email_messages SET is_read = 1 WHERE uid = 2")
        .execute(&pool)
        .await
        .expect("mark read");
    let mut app = App::new(pool).await;
    app.refresh_emails().await.expect("load emails");
    assert_eq!(app.unread_filter, None);

    app.cycle_unread_filter().await.expect("cycle");
    assert_eq!(app.unread_filter, Some(true));
    assert_eq!(subjects(&app), ["unread memo"]);

    app.cycle_unread_filter().await.expect("cycle");
    assert_eq!(app.unread_filter, Some(false));
    assert_eq!(subjects(&app), ["read memo"]);

    app.cycle_unread_filter().await.expect("cycle");
    assert_eq!(app.unread_filter, None);
    assert_eq!(app.emails.len(), 2);
}

#[tokio::test]
async fn cycle_starred_filter_walks_starred_then_unstarred_then_back_to_merged() {
    let pool = test_pool().await;
    insert_email_for_account(&pool, 1, "starred memo", "work").await;
    insert_email_for_account(&pool, 2, "plain memo", "work").await;
    sqlx::query("UPDATE email_messages SET is_starred = 1 WHERE uid = 1")
        .execute(&pool)
        .await
        .expect("mark starred");
    let mut app = App::new(pool).await;
    app.refresh_emails().await.expect("load emails");
    assert_eq!(app.starred_filter, None);

    app.cycle_starred_filter().await.expect("cycle");
    assert_eq!(app.starred_filter, Some(true));
    assert_eq!(subjects(&app), ["starred memo"]);

    app.cycle_starred_filter().await.expect("cycle");
    assert_eq!(app.starred_filter, Some(false));
    assert_eq!(subjects(&app), ["plain memo"]);

    app.cycle_starred_filter().await.expect("cycle");
    assert_eq!(app.starred_filter, None);
    assert_eq!(app.emails.len(), 2);
}

#[tokio::test]
async fn cycle_domain_filter_walks_every_domain_then_back_to_merged() {
    let pool = test_pool().await;
    insert_email_for_from(&pool, 1, "gh notice", "bot@github.com").await;
    insert_email_for_from(&pool, 2, "work memo", "boss@work.com").await;
    let mut app = App::new(pool).await;
    app.refresh_emails().await.expect("load emails");
    assert_eq!(app.domain_filter, None);

    app.cycle_domain_filter().await.expect("cycle");
    assert_eq!(app.domain_filter.as_deref(), Some("github.com"));
    assert_eq!(subjects(&app), ["gh notice"]);

    app.cycle_domain_filter().await.expect("cycle");
    assert_eq!(app.domain_filter.as_deref(), Some("work.com"));
    assert_eq!(subjects(&app), ["work memo"]);

    app.cycle_domain_filter().await.expect("cycle");
    assert_eq!(app.domain_filter, None);
    assert_eq!(app.emails.len(), 2);
}

async fn insert_email_for_from(pool: &SqlitePool, uid: i64, subject: &str, from_addr: &str) {
    sqlx::query(
        "INSERT INTO email_messages (uid, message_id, account, from_addr, subject, date_utc) VALUES (?, ?, 'work', ?, ?, '2026-01-01T00:00:00Z')",
    )
    .bind(uid)
    .bind(format!("m{uid}"))
    .bind(from_addr)
    .bind(subject)
    .execute(pool)
    .await
    .expect("insert email");
}

#[test]
fn parse_snooze_spec_reads_minutes_hours_and_days() {
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
    assert_eq!(
        parse_snooze_spec("10m", now),
        Some(now + Duration::minutes(10))
    );
    assert_eq!(parse_snooze_spec("2h", now), Some(now + Duration::hours(2)));
    assert_eq!(parse_snooze_spec("3d", now), Some(now + Duration::days(3)));
}

#[test]
fn parse_snooze_spec_rejects_zero_negative_and_garbage() {
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
    assert_eq!(parse_snooze_spec("0m", now), None);
    assert_eq!(parse_snooze_spec("-1h", now), None);
    assert_eq!(parse_snooze_spec("m", now), None);
    assert_eq!(parse_snooze_spec("", now), None);
    assert_eq!(parse_snooze_spec("10x", now), None);
    assert_eq!(parse_snooze_spec("soon", now), None);
}

#[test]
fn parse_snooze_spec_keywords_land_at_eight_am_local() {
    let now = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
    let tomorrow = parse_snooze_spec("tomorrow", now).expect("parses");
    let next_week = parse_snooze_spec("nextweek", now).expect("parses");
    assert_eq!(
        tomorrow
            .with_timezone(&chrono::Local)
            .format("%H:%M")
            .to_string(),
        "08:00"
    );
    assert_eq!(
        next_week
            .with_timezone(&chrono::Local)
            .format("%H:%M")
            .to_string(),
        "08:00"
    );
    assert_eq!((next_week - tomorrow).num_days(), 6);
}

#[tokio::test]
async fn snoozing_an_email_hides_it_until_unsnoozed_or_it_lapses() {
    let pool = test_pool().await;
    insert_email(&pool, 1, "notes", "2026-01-01T00:00:00Z").await;
    let mut app = App::new(pool).await;
    app.refresh_emails().await.expect("load emails");

    app.input_buffer = "1h".to_string();
    app.commit_snooze().await;
    assert!(
        subjects(&app).is_empty(),
        "snoozed email still in normal view"
    );

    app.toggle_show_snoozed().await.expect("toggle");
    assert_eq!(subjects(&app), ["notes"]);

    app.unsnooze_selected_email().await.expect("unsnooze");
    assert!(
        subjects(&app).is_empty(),
        "unsnoozed email still in snoozed view"
    );

    app.toggle_show_snoozed().await.expect("toggle back");
    assert_eq!(subjects(&app), ["notes"]);
}

#[tokio::test]
async fn commit_snooze_with_an_invalid_spec_leaves_the_email_visible() {
    let pool = test_pool().await;
    insert_email(&pool, 1, "notes", "2026-01-01T00:00:00Z").await;
    let mut app = App::new(pool).await;
    app.refresh_emails().await.expect("load emails");

    app.input_buffer = "whenever".to_string();
    app.commit_snooze().await;

    assert_eq!(subjects(&app), ["notes"]);
}

#[tokio::test]
async fn opening_an_email_shows_its_cached_summary_without_asking_the_model() {
    let pool = test_pool().await;
    insert_email(&pool, 1, "notes", "2026-01-01T00:00:00Z").await;
    sqlx::query("UPDATE email_messages SET summary = 'Stored earlier.', body_text = ?")
        .bind("long body ".repeat(40))
        .execute(&pool)
        .await
        .expect("cache summary");
    let mut app = App::new(pool).await;
    app.refresh_emails().await.expect("load emails");

    app.open_selected_email().await.expect("open");

    let id = app.emails[0].id;
    assert_eq!(
        app.email_summaries.get(&id),
        Some(&Summary::Ready("Stored earlier.".to_string()))
    );
}

#[tokio::test]
async fn short_emails_get_no_summary() {
    let pool = test_pool().await;
    insert_email(&pool, 1, "hi", "2026-01-01T00:00:00Z").await;
    sqlx::query("UPDATE email_messages SET body_text = 'see you at 5'")
        .execute(&pool)
        .await
        .expect("set body");
    let mut app = App::new(pool).await;
    app.refresh_emails().await.expect("load emails");

    app.open_selected_email().await.expect("open");

    assert!(app.email_summaries.is_empty());
}

#[tokio::test]
async fn search_jumps_to_the_next_match_case_insensitively_and_n_repeats_it() {
    let (mut app, _pool) = app_with_tasks(&["alpha", "Beta one", "gamma", "beta two"]).await;

    app.start_search();
    app.input_buffer = "BETA".to_string();
    app.commit_search().await;
    assert_eq!(app.selected, 1);
    assert!(matches!(app.input_mode, InputMode::Normal));

    app.search_step(true).await;
    assert_eq!(app.selected, 3);
    app.search_step(false).await;
    assert_eq!(app.selected, 1);
}

#[tokio::test]
async fn search_wraps_and_says_so() {
    let (mut app, _pool) = app_with_tasks(&["beta", "x", "y"]).await;
    app.selected = 2;
    app.search_query = "beta".to_string();

    app.search_step(true).await;

    assert_eq!(app.selected, 0);
    assert_eq!(
        app.status_message.as_ref().map(|(m, _)| m.as_str()),
        Some("Search hit BOTTOM, continuing at TOP")
    );
}

#[tokio::test]
async fn search_reports_a_missing_pattern_and_keeps_the_cursor() {
    let (mut app, _pool) = app_with_tasks(&["a", "b"]).await;
    app.selected = 1;
    app.search_query = "zzz".to_string();

    app.search_step(true).await;

    assert_eq!(app.selected, 1);
    assert_eq!(
        app.status_message.as_ref().map(|(m, _)| m.as_str()),
        Some("Pattern not found: zzz")
    );
}

#[tokio::test]
async fn an_empty_search_repeats_the_last_query_or_complains_when_there_is_none() {
    let (mut app, _pool) = app_with_tasks(&["a1", "b", "a2"]).await;

    app.start_search();
    app.commit_search().await;
    assert_eq!(
        app.status_message.as_ref().map(|(m, _)| m.as_str()),
        Some("No previous search")
    );

    app.search_query = "a".to_string();
    app.start_search();
    app.commit_search().await;
    assert_eq!(app.search_query, "a");
    assert_eq!(app.selected, 2);
}

#[tokio::test]
async fn cancelling_a_search_leaves_the_cursor_and_the_last_query() {
    let (mut app, _pool) = app_with_tasks(&["a", "b"]).await;
    app.search_query = "b".to_string();

    app.start_search();
    app.input_buffer = "a".to_string();
    app.cancel_search();

    assert_eq!(app.selected, 0);
    assert_eq!(app.search_query, "b");
    assert!(app.input_buffer.is_empty());
    assert!(matches!(app.input_mode, InputMode::Normal));
}

#[tokio::test]
async fn email_search_matches_subject_or_sender() {
    let pool = test_pool().await;
    insert_email(&pool, 1, "lunch", "2026-01-01T00:00:00Z").await;
    insert_email(&pool, 2, "invoice march", "2026-01-02T00:00:00Z").await;
    insert_email(&pool, 3, "hello", "2026-01-03T00:00:00Z").await;
    sqlx::query("UPDATE email_messages SET from_name = 'Carol Lunch' WHERE uid = 3")
        .execute(&pool)
        .await
        .expect("set sender");
    let mut app = App::new(pool).await;
    app.email_sort = triptych::email::EmailSort::Date;
    app.refresh_emails().await.expect("load emails");
    app.view_mode = ViewMode::Email;
    app.search_query = "lunch".to_string();

    app.search_step(true).await;
    assert_eq!(subjects(&app)[app.selected_email], "lunch");
    app.search_step(true).await;
    assert_eq!(subjects(&app)[app.selected_email], "hello");
}

#[tokio::test]
async fn email_search_matches_body_text_via_db_query() {
    let pool = test_pool().await;
    insert_email(&pool, 1, "no match here", "2026-01-01T00:00:00Z").await;
    insert_email(&pool, 2, "also no match", "2026-01-02T00:00:00Z").await;
    sqlx::query(
        "UPDATE email_messages SET body_text = 'the quarterly forecast is attached' WHERE uid = 2",
    )
    .execute(&pool)
    .await
    .expect("set body");
    let mut app = App::new(pool).await;
    app.email_sort = triptych::email::EmailSort::Date;
    app.refresh_emails().await.expect("load emails");
    app.view_mode = ViewMode::Email;
    app.search_query = "forecast".to_string();

    app.search_step(true).await;

    assert_eq!(subjects(&app)[app.selected_email], "also no match");
}

#[tokio::test]
async fn calendar_motions_move_the_cursor_and_clamp_to_the_grid() {
    let (mut app, _pool) = app_with_tasks(&[]).await;
    app.selected_time_slot = 2;
    app.selected_day = 3;
    app.stack_index = 1;

    app.calendar_apply_motion(Motion::Down, Some(5));
    assert_eq!(
        (app.selected_time_slot, app.selected_day, app.stack_index),
        (7, 3, 0)
    );

    app.calendar_apply_motion(Motion::Bottom, None);
    assert_eq!(app.selected_time_slot, 15);
    app.calendar_apply_motion(Motion::LineEnd, None);
    assert_eq!(app.selected_day, 6);
    app.calendar_apply_motion(Motion::LineStart, None);
    assert_eq!(app.selected_day, 0);
    app.calendar_apply_motion(Motion::Top, Some(4));
    assert_eq!(app.selected_time_slot, 3);
}

#[tokio::test]
async fn compose_editing_appends_and_removes_chars_in_the_active_field() {
    let pool = test_pool().await;
    let mut app = App::new(pool).await;
    app.email_compose = Some(ComposeState::blank("default".to_string()));

    app.compose_push_char('a');
    app.compose_push_char('b');
    assert_eq!(app.email_compose.as_ref().expect("compose open").to, "ab");

    app.compose_backspace();
    assert_eq!(app.email_compose.as_ref().expect("compose open").to, "a");

    app.compose_next_field();
    app.compose_push_char('x');
    assert_eq!(app.email_compose.as_ref().expect("compose open").cc, "x");
    assert_eq!(app.email_compose.as_ref().expect("compose open").to, "a");
}

#[tokio::test]
async fn compose_newline_only_inserts_into_the_body_field() {
    let pool = test_pool().await;
    let mut app = App::new(pool).await;
    app.email_compose = Some(ComposeState::blank("default".to_string()));

    app.compose_newline();
    assert_eq!(app.email_compose.as_ref().expect("compose open").to, "");

    for _ in 0..3 {
        app.compose_next_field();
    }
    assert_eq!(
        app.email_compose
            .as_ref()
            .expect("compose open")
            .active_field,
        ComposeField::Body
    );
    app.compose_newline();
    assert_eq!(app.email_compose.as_ref().expect("compose open").body, "\n");
}

#[tokio::test]
async fn cancel_compose_closes_the_form_and_returns_to_normal_mode() {
    let pool = test_pool().await;
    let mut app = App::new(pool).await;
    app.email_compose = Some(ComposeState::blank("default".to_string()));
    app.input_mode = InputMode::EmailCompose;

    app.cancel_compose();

    assert!(app.email_compose.is_none());
    assert!(matches!(app.input_mode, InputMode::Normal));
}

#[tokio::test]
async fn start_reply_without_smtp_config_leaves_no_compose_open() {
    // No `TRIPTYCH_EMAIL_ENABLED`/`SMTP_*` env vars are set in the test process, so
    // `SmtpConfig::for_account` is empty and the guard clause should no-op rather than panic
    // or open a form with nowhere to send.
    let pool = test_pool().await;
    insert_email(&pool, 1, "hi", "2026-01-01T00:00:00Z").await;
    let mut app = App::new(pool).await;
    app.refresh_emails().await.expect("load emails");

    app.start_reply(false);

    assert!(app.email_compose.is_none());
}

async fn draft_count(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM email_drafts")
        .fetch_one(pool)
        .await
        .expect("count drafts")
}

#[tokio::test]
async fn save_compose_as_draft_inserts_a_row_and_closes_the_form() {
    let pool = test_pool().await;
    let mut app = App::new(pool.clone()).await;
    let mut compose = ComposeState::blank("default".to_string());
    compose.to = "a@b.c".to_string();
    compose.subject = "hello".to_string();
    app.email_compose = Some(compose);
    app.input_mode = InputMode::EmailCompose;

    app.save_compose_as_draft().await;

    assert!(app.email_compose.is_none());
    assert!(matches!(app.input_mode, InputMode::Normal));
    assert_eq!(draft_count(&pool).await, 1);
}

#[tokio::test]
async fn save_compose_as_draft_overwrites_the_row_it_was_resumed_from() {
    let pool = test_pool().await;
    let mut app = App::new(pool.clone()).await;
    let mut compose = ComposeState::blank("default".to_string());
    compose.subject = "first".to_string();
    app.email_compose = Some(compose);
    app.save_compose_as_draft().await;
    assert_eq!(draft_count(&pool).await, 1);

    app.open_drafts_list().await;
    app.resume_selected_draft();
    app.email_compose
        .as_mut()
        .expect("compose reopened")
        .subject = "second".to_string();
    app.save_compose_as_draft().await;

    assert_eq!(
        draft_count(&pool).await,
        1,
        "resumed save should overwrite, not insert"
    );
    app.open_drafts_list().await;
    assert_eq!(app.drafts[0].subject, "second");
}

#[tokio::test]
async fn open_drafts_list_loads_saved_drafts_most_recent_first() {
    let pool = test_pool().await;
    let mut app = App::new(pool.clone()).await;
    for subject in ["one", "two"] {
        let mut compose = ComposeState::blank("default".to_string());
        compose.subject = subject.to_string();
        app.email_compose = Some(compose);
        app.save_compose_as_draft().await;
    }

    app.open_drafts_list().await;

    assert!(app.drafts_open);
    assert_eq!(app.drafts.len(), 2);
    assert_eq!(
        app.drafts[0].subject, "two",
        "most recently saved comes first"
    );
}

#[tokio::test]
async fn resume_selected_draft_reopens_it_in_compose_and_closes_the_list() {
    let pool = test_pool().await;
    let mut app = App::new(pool).await;
    let mut compose = ComposeState::blank("default".to_string());
    compose.to = "a@b.c".to_string();
    compose.subject = "hello".to_string();
    app.email_compose = Some(compose);
    app.save_compose_as_draft().await;
    app.open_drafts_list().await;
    let draft_id = app.drafts[0].id;

    app.resume_selected_draft();

    assert!(!app.drafts_open);
    assert!(matches!(app.input_mode, InputMode::EmailCompose));
    let resumed = app.email_compose.as_ref().expect("compose reopened");
    assert_eq!(resumed.draft_id, Some(draft_id));
    assert_eq!(resumed.subject, "hello");
    assert_eq!(resumed.active_field, ComposeField::Body);
}

#[tokio::test]
async fn delete_selected_draft_removes_it_from_db_and_the_open_list() {
    let pool = test_pool().await;
    let mut app = App::new(pool.clone()).await;
    for subject in ["one", "two"] {
        let mut compose = ComposeState::blank("default".to_string());
        compose.subject = subject.to_string();
        app.email_compose = Some(compose);
        app.save_compose_as_draft().await;
    }
    app.open_drafts_list().await;
    assert_eq!(app.drafts.len(), 2);
    app.selected_draft = 1;

    app.delete_selected_draft().await;

    assert_eq!(app.drafts.len(), 1);
    assert_eq!(draft_count(&pool).await, 1);
    assert_eq!(app.selected_draft, 0, "selection clamps back into range");
}

async fn insert_email_with_invite(
    pool: &SqlitePool,
    uid: i64,
    subject: &str,
    meeting_title: &str,
    start: &str,
    end: &str,
) -> i64 {
    sqlx::query(
        "INSERT INTO email_messages (uid, message_id, from_addr, subject, date_utc, meeting_title, meeting_start, meeting_end, meeting_location) \
         VALUES (?, ?, 'a@b.c', ?, '2026-01-01T00:00:00Z', ?, ?, ?, 'Room 1')",
    )
    .bind(uid)
    .bind(format!("m{uid}"))
    .bind(subject)
    .bind(meeting_title)
    .bind(start)
    .bind(end)
    .execute(pool)
    .await
    .expect("insert email with invite")
    .last_insert_rowid()
}

#[tokio::test]
async fn accept_meeting_invite_creates_a_scheduled_task_and_marks_the_email_converted() {
    let pool = test_pool().await;
    insert_email_with_invite(
        &pool,
        1,
        "Invitation: Team Sync",
        "Team Sync",
        "2026-01-15T14:00:00Z",
        "2026-01-15T15:00:00Z",
    )
    .await;
    let mut app = App::new(pool.clone()).await;
    app.refresh_emails().await.expect("load emails");

    app.accept_meeting_invite().await.expect("accept invite");

    app.load_tasks().await.expect("load tasks");
    assert_eq!(app.tasks.len(), 1);
    assert_eq!(app.tasks[0].description, "Meeting: Team Sync");
    assert_eq!(
        app.tasks[0].scheduled_at,
        Some(Utc.with_ymd_and_hms(2026, 1, 15, 14, 0, 0).unwrap())
    );
    assert_eq!(app.tasks[0].duration_minutes, Some(60));
    let task_id = app.tasks[0].id;

    app.refresh_emails().await.expect("reload emails");
    assert_eq!(app.emails[0].task_id, Some(task_id));
}

#[tokio::test]
async fn accept_meeting_invite_is_a_no_op_without_an_invite() {
    let pool = test_pool().await;
    insert_email(&pool, 1, "just a normal email", "2026-01-01T00:00:00Z").await;
    let mut app = App::new(pool).await;
    app.refresh_emails().await.expect("load emails");

    app.accept_meeting_invite().await.expect("no-op");

    app.load_tasks().await.expect("load tasks");
    assert!(app.tasks.is_empty());
}

#[tokio::test]
async fn accept_meeting_invite_is_idempotent_once_already_converted() {
    let pool = test_pool().await;
    insert_email_with_invite(
        &pool,
        1,
        "Invitation: Team Sync",
        "Team Sync",
        "2026-01-15T14:00:00Z",
        "2026-01-15T15:00:00Z",
    )
    .await;
    let mut app = App::new(pool).await;
    app.refresh_emails().await.expect("load emails");
    app.accept_meeting_invite().await.expect("accept invite");

    app.accept_meeting_invite()
        .await
        .expect("second call no-ops");

    app.load_tasks().await.expect("load tasks");
    assert_eq!(app.tasks.len(), 1, "must not create a second task");
}

async fn rule_applied(pool: &SqlitePool, email_id: i64) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT rule_applied FROM email_messages WHERE id = ?")
        .bind(email_id)
        .fetch_one(pool)
        .await
        .expect("read rule_applied")
        == 1
}

#[tokio::test]
async fn commit_rule_input_saves_a_valid_spec_and_reloads_the_popup_list() {
    let pool = test_pool().await;
    let mut app = App::new(pool).await;
    app.start_rule_input();
    "subject newsletter star".clone_into(&mut app.input_buffer);

    app.commit_rule_input().await;

    assert!(matches!(app.input_mode, InputMode::Normal));
    assert_eq!(app.rules.len(), 1);
    assert_eq!(app.rules[0].match_field, "subject");
    assert_eq!(app.rules[0].pattern, "newsletter");
    assert_eq!(app.rules[0].action, "star");
}

#[tokio::test]
async fn commit_rule_input_rejects_an_unparseable_spec_and_saves_nothing() {
    let pool = test_pool().await;
    let mut app = App::new(pool).await;
    app.start_rule_input();
    "body newsletter star".clone_into(&mut app.input_buffer);

    app.commit_rule_input().await;

    assert!(app.rules.is_empty());
}

#[tokio::test]
async fn run_email_rules_stars_only_the_matching_email() {
    let pool = test_pool().await;
    insert_email(&pool, 1, "weekly newsletter", "2026-01-01T00:00:00Z").await;
    insert_email(&pool, 2, "team sync", "2026-01-02T00:00:00Z").await;
    let mut app = App::new(pool.clone()).await;
    app.refresh_emails().await.expect("load emails");
    app.start_rule_input();
    "subject newsletter star".clone_into(&mut app.input_buffer);
    app.commit_rule_input().await;

    app.run_email_rules().await;

    let by_subject = |s: &str| {
        app.emails
            .iter()
            .find(|e| e.subject == s)
            .expect("email present")
    };
    assert!(by_subject("weekly newsletter").is_starred);
    assert!(!by_subject("team sync").is_starred);
    assert!(rule_applied(&pool, by_subject("weekly newsletter").id).await);
    assert!(rule_applied(&pool, by_subject("team sync").id).await);
}

#[tokio::test]
async fn run_email_rules_does_not_re_star_after_a_manual_unstar() {
    let pool = test_pool().await;
    insert_email(&pool, 1, "weekly newsletter", "2026-01-01T00:00:00Z").await;
    let mut app = App::new(pool).await;
    app.refresh_emails().await.expect("load emails");
    app.start_rule_input();
    "subject newsletter star".clone_into(&mut app.input_buffer);
    app.commit_rule_input().await;
    app.run_email_rules().await;
    app.toggle_selected_star().await.expect("unstar");
    assert!(!app.emails[app.selected_email].is_starred);

    app.run_email_rules().await;

    assert!(
        !app.emails[app.selected_email].is_starred,
        "already-checked mail must not be re-matched on a later pass"
    );
}

#[tokio::test]
async fn run_email_rules_archive_and_delete_are_a_no_op_without_imap_config() {
    // No IMAP_* env vars are set in this process (see tests/CLAUDE.md), so `EmailConfig::for_account`
    // returns None for the row's `account = 'default'` and `apply_rule_action` must skip the spawn
    // rather than panic — the email stays exactly as loaded, just marked checked.
    let pool = test_pool().await;
    insert_email(&pool, 1, "please archive me", "2026-01-01T00:00:00Z").await;
    insert_email(&pool, 2, "please delete me", "2026-01-02T00:00:00Z").await;
    let mut app = App::new(pool.clone()).await;
    app.refresh_emails().await.expect("load emails");
    app.start_rule_input();
    "subject archive archive".clone_into(&mut app.input_buffer);
    app.commit_rule_input().await;
    app.start_rule_input();
    "subject delete delete".clone_into(&mut app.input_buffer);
    app.commit_rule_input().await;

    app.run_email_rules().await;

    assert_eq!(
        app.emails.len(),
        2,
        "no config to archive/delete against, both rows remain"
    );
    let by_subject = |s: &str| {
        app.emails
            .iter()
            .find(|e| e.subject == s)
            .expect("email present")
    };
    assert!(rule_applied(&pool, by_subject("please archive me").id).await);
    assert!(rule_applied(&pool, by_subject("please delete me").id).await);
}

#[tokio::test]
async fn delete_selected_rule_removes_it_and_clamps_selection() {
    let pool = test_pool().await;
    let mut app = App::new(pool).await;
    for spec in ["subject newsletter star", "from noreply read"] {
        app.start_rule_input();
        spec.clone_into(&mut app.input_buffer);
        app.commit_rule_input().await;
    }
    assert_eq!(app.rules.len(), 2);
    app.selected_rule = 1;

    app.delete_selected_rule().await;

    assert_eq!(app.rules.len(), 1);
    assert_eq!(app.rules[0].pattern, "newsletter");
    assert_eq!(app.selected_rule, 0);
}

#[tokio::test]
async fn load_tasks_orders_by_priority_then_item_order() {
    let pool = test_pool().await;
    for (desc, order, prio) in [
        ("low", 0, 0),
        ("urgent", 1, 3),
        ("med a", 2, 1),
        ("med b", 3, 1),
    ] {
        sqlx::query("INSERT INTO tasks (description, completed, item_order, priority) VALUES (?, false, ?, ?)")
            .bind(desc)
            .bind(order)
            .bind(prio)
            .execute(&pool)
            .await
            .expect("insert");
    }
    let mut app = App::new(pool).await;
    app.load_tasks().await.expect("load tasks");
    let names: Vec<_> = app.tasks.iter().map(|t| t.description.as_str()).collect();
    assert_eq!(names, ["urgent", "med a", "med b", "low"]);
}

#[tokio::test]
async fn completing_a_task_sinks_it_and_the_cursor_follows() {
    let pool = test_pool().await;
    for (desc, order, prio) in [("a", 0, 3), ("b", 1, 2), ("c", 2, 1)] {
        sqlx::query("INSERT INTO tasks (description, completed, item_order, priority) VALUES (?, false, ?, ?)")
            .bind(desc)
            .bind(order)
            .bind(prio)
            .execute(&pool)
            .await
            .expect("insert");
    }
    let mut app = App::new(pool).await;
    app.load_tasks().await.expect("load tasks");
    app.toggle_completed().await.expect("complete a");
    let names: Vec<_> = app.tasks.iter().map(|t| t.description.as_str()).collect();
    assert_eq!(names, ["b", "c", "a"]);
    assert_eq!(app.tasks[app.selected].description, "a");
    app.toggle_completed().await.expect("reopen a");
    assert_eq!(app.tasks[0].description, "a");
    assert_eq!(app.tasks[app.selected].description, "a");
}

#[tokio::test]
async fn reword_task_updates_description_and_keeps_selection() {
    let pool = test_pool().await;
    for (desc, prio) in [("low", 0), ("high", 2)] {
        sqlx::query(
            "INSERT INTO tasks (description, completed, item_order, priority) VALUES (?, false, 0, ?)",
        )
        .bind(desc)
        .bind(prio)
        .execute(&pool)
        .await
        .expect("insert");
    }
    let mut app = App::new(pool).await;
    app.load_tasks().await.expect("load tasks");
    app.selected = 1;
    app.start_task_reword();
    assert_eq!(app.input_buffer, "low");
    let id = app.editing_task_id.expect("editing id");
    app.input_buffer = "  reworded  ".to_string();
    app.commit_task_reword(id).await.expect("reword");
    assert_eq!(app.tasks[app.selected].description, "reworded");
    assert_eq!(app.tasks[app.selected].priority, 0);
}
