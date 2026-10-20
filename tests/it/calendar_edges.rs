//! Calendar week, cell and schedule-block boundaries that mutation testing showed no other test pins down.
use chrono::{Datelike, Duration, NaiveDate, NaiveTime};
use sqlx::SqlitePool;
use triptych::app::{App, CalendarInputMode, resolve_local_datetime};
use triptych::clock::local_now;

use crate::app::{insert_task, test_pool};

async fn calendar() -> (App, SqlitePool) {
    let pool = test_pool().await;
    let mut app = App::new(pool.clone()).await;
    app.calendar_week_offset = Some(0);
    (app, pool)
}

/// Puts the cursor on `day` (0 = Monday) at `7 + slot` o'clock.
const fn point(app: &mut App, day: usize, slot: usize) {
    app.selected_day = day;
    app.selected_time_slot = slot;
    app.stack_index = 0;
}

fn status(app: &App) -> Option<&str> {
    app.status_message.as_ref().map(|(m, _)| m.as_str())
}

async fn block(pool: &SqlitePool, day: i32, start: &str, end: &str, source: &str) {
    sqlx::query(
        "INSERT INTO schedule_blocks (day_of_week, start_time, end_time, block_type, title, source) VALUES (?, ?, ?, 'deepwork', 'block', ?)",
    )
    .bind(day)
    .bind(start)
    .bind(end)
    .bind(source)
    .execute(pool)
    .await
    .unwrap();
}

async fn block_count(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM schedule_blocks")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// An open task scheduled for `hour` o'clock on `date`, one hour long.
async fn schedule(pool: &SqlitePool, desc: &str, date: NaiveDate, hour: u32) -> i64 {
    let id = insert_task(pool, desc).await;
    sqlx::query("UPDATE tasks SET scheduled_at = ?, duration_minutes = 60 WHERE id = ?")
        .bind(resolve_local_datetime(
            date.and_hms_opt(hour, 0, 0).unwrap(),
        ))
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    id
}

const fn hm(hour: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(hour, 0, 0).unwrap()
}

#[tokio::test]
async fn paging_weeks_moves_the_offset_and_resets_the_stack() {
    let (mut app, _pool) = calendar().await;
    app.stack_index = 4;
    app.next_week().await;
    app.next_week().await;
    assert_eq!((app.calendar_week_offset, app.stack_index), (Some(2), 0));

    app.stack_index = 4;
    for _ in 0..3 {
        app.prev_week().await;
    }
    assert_eq!((app.calendar_week_offset, app.stack_index), (Some(-1), 0));
}

#[tokio::test]
async fn the_loaded_week_follows_the_offset() {
    let (mut app, pool) = calendar().await;
    let today = local_now().date_naive();
    let weekday = i32::try_from(today.weekday().num_days_from_monday()).unwrap();
    block(&pool, weekday, "09:00", "10:00", "manual").await;
    let dates = |app: &App| -> Vec<NaiveDate> {
        app.cached_schedule_blocks.iter().map(|(d, _)| *d).collect()
    };

    app.next_week().await;
    assert_eq!(dates(&app), [today + Duration::days(7)]);
    app.prev_week().await;
    app.prev_week().await;
    assert_eq!(dates(&app), [today - Duration::days(7)]);
}

#[tokio::test]
async fn a_week_loads_each_block_on_its_own_weekday_and_only_open_allocations() {
    let (mut app, pool) = calendar().await;
    point(&mut app, 2, 0);
    let wednesday = app.selected_cell_date();
    block(&pool, 0, "09:00", "10:00", "manual").await;
    block(&pool, 2, "13:00", "14:00", "manual").await;
    let open = insert_task(&pool, "alpha").await;
    let done = insert_task(&pool, "beta").await;
    sqlx::query("UPDATE tasks SET completed = 1 WHERE id = ?")
        .bind(done)
        .execute(&pool)
        .await
        .unwrap();
    for id in [open, done] {
        sqlx::query(
            "INSERT INTO task_block_allocations (task_id, block_date, block_start_time, block_end_time, allocated_minutes) VALUES (?, ?, '13:00', '14:00', 60)",
        )
        .bind(id)
        .bind(wednesday.to_string())
        .execute(&pool)
        .await
        .unwrap();
    }

    app.refresh_calendar_data().await;

    assert_eq!(app.cached_schedule_blocks.len(), 2);
    for (date, b) in &app.cached_schedule_blocks {
        let weekday = i32::try_from(date.weekday().num_days_from_monday()).unwrap();
        assert_eq!(weekday, b.day_of_week, "{date}");
    }
    let allocations: Vec<_> = app
        .cached_task_allocations
        .iter()
        .map(|(d, t, id, desc, minutes, _)| (*d, *t, *id, desc.as_str(), *minutes))
        .collect();
    assert_eq!(allocations, [(wednesday, hm(13), open, "alpha", 60)]);
}

#[tokio::test]
async fn the_selected_cell_lists_the_tasks_scheduled_into_it() {
    let (mut app, pool) = calendar().await;
    point(&mut app, 2, 3);
    let wednesday = app.selected_cell_date();
    let a = schedule(&pool, "a", wednesday, 10).await;
    let b = schedule(&pool, "b", wednesday, 10).await;
    schedule(&pool, "elsewhere", wednesday, 14).await;
    app.refresh_calendar_data().await;

    let ids: Vec<i64> = app.selected_cell_tasks().iter().map(|t| t.id).collect();

    assert_eq!(ids, [a, b]);
}

#[tokio::test]
async fn a_block_covers_its_start_but_not_its_end_and_only_on_its_own_day() {
    let (mut app, pool) = calendar().await;
    block(&pool, 0, "09:00", "11:00", "manual").await;
    app.refresh_calendar_data().await;

    for (day, slot, covered) in [
        (0, 0, false), // 07:00, before it starts
        (0, 2, true),  // 09:00, the start is included
        (0, 3, true),  // 10:00
        (0, 4, false), // 11:00, the end is not
        (1, 3, false), // Tuesday 10:00, another day
    ] {
        point(&mut app, day, slot);
        let found = app.selected_cell_block().map(|b| b.start_time.as_str());
        assert_eq!(
            found,
            covered.then_some("09:00"),
            "day {day} 7+{slot} o'clock"
        );
    }
}

#[tokio::test]
async fn the_detail_popup_opens_for_a_block_or_a_task_and_not_for_an_empty_cell() {
    let (mut app, pool) = calendar().await;
    block(&pool, 0, "09:00", "11:00", "manual").await;
    point(&mut app, 1, 3);
    let tuesday = app.selected_cell_date();
    schedule(&pool, "t", tuesday, 10).await;
    app.refresh_calendar_data().await;

    point(&mut app, 2, 3);
    app.open_cell_detail();
    assert_eq!(app.calendar_input_mode, CalendarInputMode::Navigate);
    assert_eq!(status(&app), Some("Nothing in this cell"));

    for (day, what) in [(0, "block only"), (1, "task only")] {
        app.calendar_input_mode = CalendarInputMode::Navigate;
        point(&mut app, day, 3);
        app.open_cell_detail();
        assert_eq!(
            app.calendar_input_mode,
            CalendarInputMode::CellDetail,
            "{what}"
        );
    }
}

#[tokio::test]
async fn cycling_walks_a_stack_round_and_leaves_small_cells_alone() {
    let (mut app, pool) = calendar().await;
    point(&mut app, 2, 3);
    let wednesday = app.selected_cell_date();
    for name in ["a", "b", "c"] {
        schedule(&pool, name, wednesday, 10).await;
    }
    schedule(&pool, "solo", wednesday, 12).await;
    app.refresh_calendar_data().await;

    let mut seen = Vec::new();
    for _ in 0..4 {
        app.cycle_stack_next();
        seen.push(app.stack_index);
    }
    assert_eq!(seen, [1, 2, 0, 1]);
    app.stack_index = 0;
    seen.clear();
    for _ in 0..4 {
        app.cycle_stack_prev();
        seen.push(app.stack_index);
    }
    assert_eq!(seen, [2, 1, 0, 2]);

    // One task or none: nothing to cycle to, so the index stays where it was.
    for slot in [5, 8] {
        point(&mut app, 2, slot);
        app.stack_index = 1;
        app.cycle_stack_next();
        app.cycle_stack_prev();
        assert_eq!(app.stack_index, 1, "7+{slot} o'clock");
    }
}

#[tokio::test]
async fn the_cursor_date_is_the_viewed_weeks_day_column() {
    let (mut app, _pool) = calendar().await;
    let today = local_now().date_naive();
    for day in 0..7 {
        point(&mut app, day, 0);
        let date = app.selected_cell_date();
        assert_eq!(
            usize::try_from(date.weekday().num_days_from_monday()).unwrap(),
            day
        );
        assert!(date - Duration::days(6) <= today && today <= date + Duration::days(6));
    }
    point(&mut app, 0, 0);
    let monday = app.selected_cell_date();
    assert!(monday <= today && today < monday + Duration::days(7));
    app.calendar_week_offset = Some(1);
    assert_eq!(app.selected_cell_date(), monday + Duration::days(7));
}

#[tokio::test]
async fn picking_up_cancelling_and_editing_act_on_the_task_in_the_cell() {
    let (mut app, pool) = calendar().await;
    point(&mut app, 2, 3);
    let wednesday = app.selected_cell_date();
    let id = schedule(&pool, "move me", wednesday, 10).await;
    app.refresh_calendar_data().await;

    app.cancel_held_task();
    assert!(status(&app).is_none(), "nothing held, nothing to cancel");
    app.pick_up_task_at_selected_cell();
    assert_eq!(app.held_task, Some(id));
    app.cancel_held_task();
    assert_eq!(app.held_task, None);
    assert_eq!(status(&app), Some("Move cancelled"));

    app.input_buffer = "stale".into();
    app.start_deadline_edit_at_selected_cell();
    assert_eq!(app.deadline_edit_task_id, Some(id));
    assert_eq!(app.calendar_input_mode, CalendarInputMode::DeadlineInput);
    assert!(app.input_buffer.is_empty());

    point(&mut app, 2, 8);
    app.deadline_edit_task_id = None;
    app.calendar_input_mode = CalendarInputMode::Navigate;
    app.start_deadline_edit_at_selected_cell();
    assert_eq!(status(&app), Some("No task here to set a deadline for"));
    assert_eq!(app.deadline_edit_task_id, None);
    assert_eq!(app.calendar_input_mode, CalendarInputMode::Navigate);
}

fn fill_form(app: &mut App, start: &str, end: &str) {
    app.block_form.start_time = start.into();
    app.block_form.end_time = end.into();
    app.block_form.block_type = "deepwork".into();
    app.block_form.title = "Focus".into();
    app.calendar_input_mode = CalendarInputMode::BlockForm;
}

#[tokio::test]
async fn the_block_form_creates_a_block_on_the_cursor_day_and_closes() {
    let (mut app, pool) = calendar().await;
    point(&mut app, 3, 0);
    fill_form(&mut app, "09:00", "10:30");

    app.create_schedule_block().await.unwrap();

    let rows: Vec<(i32, String, String, String, String)> = sqlx::query_as(
        "SELECT day_of_week, start_time, end_time, block_type, title FROM schedule_blocks",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    let rows: Vec<_> = rows
        .iter()
        .map(|(d, s, e, t, n)| (*d, s.as_str(), e.as_str(), t.as_str(), n.as_str()))
        .collect();
    assert_eq!(rows, [(3, "09:00", "10:30", "deepwork", "Focus")]);
    assert_eq!(status(&app), Some("Block created"));
    assert_eq!(app.calendar_input_mode, CalendarInputMode::Navigate);
    assert_eq!(app.cached_schedule_blocks.len(), 1, "week reloaded");
}

#[tokio::test]
async fn the_block_form_refuses_bad_times_and_overlaps() {
    for (start, end, message) in [
        ("9am", "10:00", "Invalid start time format"),
        ("09:00", "late", "Invalid end time format"),
        ("10:00", "10:00", "End time must be after start time"),
        ("11:00", "10:00", "End time must be after start time"),
    ] {
        let (mut app, pool) = calendar().await;
        fill_form(&mut app, start, end);
        app.create_schedule_block().await.unwrap();
        assert_eq!(status(&app), Some(message), "{start} to {end}");
        assert_eq!(block_count(&pool).await, 0, "{start} to {end}");
        assert_eq!(app.calendar_input_mode, CalendarInputMode::BlockForm);
    }

    let (mut app, pool) = calendar().await;
    point(&mut app, 0, 0);
    block(&pool, 0, "10:00", "12:00", "manual").await;
    fill_form(&mut app, "11:00", "13:00");
    app.create_schedule_block().await.unwrap();
    assert_eq!(status(&app), Some("Block overlaps with existing block"));
    assert_eq!(block_count(&pool).await, 1);
}

#[tokio::test]
async fn an_imported_block_must_end_after_it_starts() {
    let dir = std::env::temp_dir().join(format!("triptych_calendar_edges_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (mut app, pool) = calendar().await;
    for (start, end) in [("10:00", "09:00"), ("10:00", "10:00"), ("25:00", "26:00")] {
        let path = dir.join("schedule.toml");
        std::fs::write(
            &path,
            format!(
                "[[blocks]]\nday = \"monday\"\ntype = \"deepwork\"\nstart = \"{start}\"\nend = \"{end}\"\ntitle = \"Bad\"\n"
            ),
        )
        .unwrap();

        let result = app.import_schedule_from_toml(&path).await;

        assert!(result.is_err(), "{start} to {end} imported");
        assert_eq!(block_count(&pool).await, 0, "{start} to {end}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn every_day_name_and_abbreviation_has_its_monday_first_number() {
    for (name, number) in [
        ("monday", 0),
        ("mon", 0),
        ("tuesday", 1),
        ("tue", 1),
        ("tues", 1),
        ("wednesday", 2),
        ("wed", 2),
        ("thursday", 3),
        ("thu", 3),
        ("thurs", 3),
        ("friday", 4),
        ("fri", 4),
        ("saturday", 5),
        ("sat", 5),
        ("sunday", 6),
        ("sun", 6),
        ("SUNDAY", 6),
    ] {
        assert_eq!(App::parse_days(name).unwrap(), vec![number], "{name}");
    }
    assert_eq!(App::parse_days("sunday_monday_mon").unwrap(), vec![0, 6]);
    assert!(App::parse_days("").is_err());
}

#[test]
fn day_numbers_map_back_to_lowercase_names() {
    for (number, name) in [
        (0, "monday"),
        (1, "tuesday"),
        (2, "wednesday"),
        (3, "thursday"),
        (4, "friday"),
        (5, "saturday"),
        (6, "sunday"),
        (7, "unknown"),
        (-1, "unknown"),
    ] {
        assert_eq!(App::day_number_to_name(number), name);
    }
}

#[tokio::test]
async fn deleting_at_the_cursor_removes_only_a_manual_block_that_covers_it() {
    let (mut app, pool) = calendar().await;
    block(&pool, 0, "09:00", "11:00", "manual").await;
    app.refresh_calendar_data().await;

    for (day, slot, what) in [
        (0, 0, "before it starts"),
        (0, 4, "at its end"),
        (1, 3, "another day"),
    ] {
        point(&mut app, day, slot);
        app.delete_block_at_selected_cell().await.unwrap();
        assert_eq!(status(&app), Some("No block at this time"), "{what}");
        assert_eq!(block_count(&pool).await, 1, "{what}");
    }

    for slot in [2, 3] {
        point(&mut app, 0, slot);
        app.delete_block_at_selected_cell().await.unwrap();
        assert_eq!(status(&app), Some("Deleted block: block"), "7+{slot}");
        assert_eq!(block_count(&pool).await, 0, "7+{slot}");
        assert!(app.cached_schedule_blocks.is_empty());
        block(&pool, 0, "09:00", "11:00", "manual").await;
        app.refresh_calendar_data().await;
    }
}

#[tokio::test]
async fn a_block_from_the_toml_file_is_not_deleted_from_the_calendar() {
    let (mut app, pool) = calendar().await;
    block(&pool, 0, "09:00", "11:00", "toml").await;
    app.refresh_calendar_data().await;
    point(&mut app, 0, 3);

    app.delete_block_at_selected_cell().await.unwrap();

    assert_eq!(
        status(&app),
        Some("Block is from schedule.toml: edit the file and re-import")
    );
    assert_eq!(block_count(&pool).await, 1);
}
