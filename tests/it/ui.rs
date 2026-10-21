use crate::app::{insert_email, test_pool};
use chrono::{Duration, NaiveDate, NaiveTime};
use ratatui::style::{Color, Modifier, Style};
use ratatui::{Terminal, backend::TestBackend};
use triptych::app::{App, ScheduleBlock, ViewMode};
use triptych::ui::*;

fn screen(terminal: &Terminal<TestBackend>) -> String {
    let buf = terminal.backend().buffer();
    (0..buf.area.height)
        .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
        .map(|pos| buf[pos].symbol())
        .collect()
}

#[tokio::test]
async fn a_long_email_list_scrolls_only_as_far_as_the_cursor_needs() {
    let pool = test_pool().await;
    // Newest first, so mail-00 heads the list and mail-39 ends it.
    for uid in 0..40_i64 {
        let age = 39 - uid;
        let date = format!("2026-01-{:02}T{:02}:00:00Z", age / 24 + 1, age % 24);
        insert_email(&pool, uid, &format!("mail-{uid:02}"), &date).await;
    }
    let mut app = App::new(pool).await;
    app.refresh_emails().await.unwrap();
    app.view_mode = ViewMode::Email;
    let mut terminal = Terminal::new(TestBackend::new(130, 14)).unwrap();

    app.selected_email = 39;
    terminal.draw(|f| ui(f, &mut app)).unwrap();
    let bottom = screen(&terminal);
    assert!(bottom.contains("mail-39") && !bottom.contains("mail-00"));

    // One step up stays inside the viewport: no scroll, so the last row is still shown.
    app.selected_email = 38;
    terminal.draw(|f| ui(f, &mut app)).unwrap();
    assert!(screen(&terminal).contains("mail-39"));

    app.selected_email = 0;
    terminal.draw(|f| ui(f, &mut app)).unwrap();
    let top = screen(&terminal);
    assert!(top.contains("mail-00") && !top.contains("mail-39"));

    // Rows past the viewport are never built: a jump to the middle shows neither end.
    app.selected_email = 20;
    terminal.draw(|f| ui(f, &mut app)).unwrap();
    let middle = screen(&terminal);
    assert!(
        middle.contains("mail-20") && !middle.contains("mail-00") && !middle.contains("mail-39")
    );
}

#[tokio::test]
async fn the_selected_email_row_is_reversed_and_keeps_its_unread_bold() {
    let pool = test_pool().await;
    insert_email(&pool, 1, "unread subject", "2026-01-01T00:00:00Z").await;
    let mut app = App::new(pool).await;
    app.refresh_emails().await.unwrap();
    app.view_mode = ViewMode::Email;
    let mut terminal = Terminal::new(TestBackend::new(130, 12)).unwrap();

    terminal.draw(|f| ui(f, &mut app)).unwrap();

    let buf = terminal.backend().buffer();
    let (x, y) = (0..buf.area.height)
        .find_map(|y| {
            let row: String = (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect();
            let at = row.find("unread subject")?;
            Some((u16::try_from(row[..at].chars().count()).unwrap(), y))
        })
        .expect("subject on screen");
    let modifier = buf[(x, y)].style().add_modifier;
    assert!(
        modifier.contains(Modifier::BOLD | Modifier::REVERSED),
        "{modifier:?}"
    );
}

fn day(offset: i64) -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 3, 10).unwrap() + Duration::days(offset)
}

const fn time(hour: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(hour, 0, 0).unwrap()
}

#[test]
fn urgency_style_gets_brighter_red_and_stays_in_the_terminal_palette() {
    let styles: Vec<Style> = (0..=3).map(urgency_style).collect();
    assert_eq!(styles[3].fg, Some(Color::LightRed));
    assert!(styles[3].add_modifier.contains(Modifier::BOLD));
    assert_eq!(styles[2].fg, Some(Color::Red));
    assert!(!styles[2].add_modifier.contains(Modifier::BOLD));
    assert_ne!(styles[0].fg, styles[1].fg);
    for style in styles {
        assert!(
            !matches!(style.fg, Some(Color::Rgb(..) | Color::Indexed(_))),
            "{style:?}"
        );
    }
}

#[test]
fn cell_task_displays_returns_every_task_sharing_an_hour() {
    let d = day(0);
    let scheduled_tasks = vec![
        (d, time(9), 1, "manual one".to_string(), 60, 1),
        (d, time(9), 2, "manual two".to_string(), 60, 1),
    ];
    let task_allocations = vec![(d, time(9), 3, "alloc one".to_string(), 30, 1)];
    let grid = CalendarGrid {
        days: vec![d],
        time_slots: vec![],
        schedule_blocks: &[],
        scheduled_tasks: &scheduled_tasks,
        task_allocations: &task_allocations,
    };

    let displays = cell_task_displays(&grid, d, time(9));
    assert_eq!(displays.len(), 3);
    assert_eq!(displays[0].0, "manual one");
    assert_eq!(displays[1].0, "manual two");
    assert_eq!(displays[2].0, "alloc one");
    assert!(!displays[0].2);
    assert!(!displays[1].2);
    assert!(displays[2].2);
}

#[test]
fn build_cell_view_marks_overflow_when_hour_is_shared() {
    let d = day(0);
    let no_allocations = vec![];

    let two_tasks = vec![
        (d, time(9), 1, "first".to_string(), 60, 1),
        (d, time(9), 2, "second".to_string(), 60, 1),
    ];
    let grid_two = CalendarGrid {
        days: vec![d],
        time_slots: vec![],
        schedule_blocks: &[],
        scheduled_tasks: &two_tasks,
        task_allocations: &no_allocations,
    };
    let view_two = build_cell_view(&grid_two, 0, time(9), 0);
    assert_eq!(view_two.overflow.as_deref(), Some("1/2"));
    assert!(view_two.headline.contains("first"));

    let view_second = build_cell_view(&grid_two, 0, time(9), 1);
    assert_eq!(view_second.overflow.as_deref(), Some("2/2"));
    assert!(view_second.headline.contains("second"));

    let one_task = vec![(d, time(9), 1, "only".to_string(), 60, 1)];
    let grid_one = CalendarGrid {
        days: vec![d],
        time_slots: vec![],
        schedule_blocks: &[],
        scheduled_tasks: &one_task,
        task_allocations: &no_allocations,
    };
    let view_one = build_cell_view(&grid_one, 0, time(9), 0);
    assert!(view_one.overflow.is_none());
}

#[test]
fn build_cell_view_keeps_block_label_when_cell_has_no_task() {
    let d = day(0);
    let blocks = vec![(
        d,
        ScheduleBlock {
            id: 1,
            day_of_week: 0,
            start_time: "09:00".to_string(),
            end_time: "10:00".to_string(),
            block_type: "deepwork".to_string(),
            title: "Deep work".to_string(),
            description: None,
            priority: 1,
        },
    )];
    let no_tasks = vec![];
    let no_allocations = vec![];
    let grid = CalendarGrid {
        days: vec![d],
        time_slots: vec![],
        schedule_blocks: &blocks,
        scheduled_tasks: &no_tasks,
        task_allocations: &no_allocations,
    };

    let view = build_cell_view(&grid, 0, time(9), 0);
    assert_eq!(view.headline, "[deepwork]");
    assert!(view.overflow.is_none());
}

/// Allocation spanning multiple hours must show in every hour cell it
/// covers, not just the one matching its start time - and land on the
/// exact per-task start `allocate_task_to_blocks` now writes, not the
/// block's own start.
#[test]
fn cell_task_displays_finds_multi_hour_allocation_by_range() {
    let d = day(0);
    let no_tasks = vec![];
    // 90-minute allocation starting at 9:00 - should show at 9am and 10am,
    // not 11am, and not at all on a different day.
    let allocations = vec![(d, time(9), 1, "study rust".to_string(), 90, 1)];
    let grid = CalendarGrid {
        days: vec![d],
        time_slots: vec![],
        schedule_blocks: &[],
        scheduled_tasks: &no_tasks,
        task_allocations: &allocations,
    };

    assert_eq!(cell_task_displays(&grid, d, time(9)).len(), 1);
    assert_eq!(cell_task_displays(&grid, d, time(10)).len(), 1);
    assert!(cell_task_displays(&grid, d, time(11)).is_empty());
    assert!(cell_task_displays(&grid, day(1), time(9)).is_empty());
}

/// The calendar shows only the rows and days that fit, and the cursor is always among them.
#[test]
fn window_keeps_the_cursor_in_view_and_inside_the_bounds() {
    // Everything fits: no scrolling, whatever the cursor.
    assert_eq!(window(9, 16, 16), 0..16);
    assert_eq!(window(3, 7, 40), 0..7);
    // Centred when there is room either side, pinned at the ends otherwise.
    assert_eq!(window(8, 16, 3), 7..10);
    assert_eq!(window(0, 16, 3), 0..3);
    assert_eq!(window(15, 16, 3), 13..16);
    // Never empty while there is something to show, even in a zero-height terminal.
    assert_eq!(window(5, 16, 0), 5..6);
    assert_eq!(window(0, 0, 4), 0..0);

    for (total, visible) in [(16, 1), (16, 3), (16, 4), (7, 2), (7, 5)] {
        for selected in 0..total {
            let w = window(selected, total, visible);
            assert!(w.contains(&selected), "{selected} not in {w:?} of {total}");
            assert!(w.end <= total && w.len() == visible, "{w:?} of {total}");
        }
    }
}
