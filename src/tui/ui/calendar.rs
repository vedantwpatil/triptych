//! The weekly calendar view: header, hour rows and status line.

use super::grid::{CalendarGrid, TimeSlot, build_cell_view};
use super::popups::{
    render_block_form_popup, render_calendar_task_input, render_deadline_input, render_task_picker,
};
use crate::app::{App, CalendarInputMode};
use chrono::{Datelike, Duration, NaiveDate, NaiveTime, Timelike};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table},
};

/// The "today / now" accent, shared by the header's today column and the
/// current-hour row so the two can't visually drift apart.
fn today_accent() -> Style {
    Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
}

// One screen's worth of rendering - splitting it into helpers would scatter
// widget-building state without reducing its actual complexity.
#[allow(clippy::too_many_lines)]
pub(super) fn render_calendar_view(f: &mut Frame, app: &App) {
    f.render_widget(Clear, f.area());

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([Constraint::Min(3), Constraint::Length(3)].as_ref())
        .split(f.area());

    let calendar_data = build_calendar_grid(app);

    // Check if calendar is empty (no blocks and no tasks)
    let is_empty =
        calendar_data.schedule_blocks.is_empty() && calendar_data.scheduled_tasks.is_empty();

    // Build header with weekday names
    let header_strings: Vec<String> = std::iter::once("Time".to_string())
        .chain(
            calendar_data
                .days
                .iter()
                .map(|d| d.format("%a %m/%d").to_string()),
        )
        .collect();

    let today = chrono::Local::now().naive_local().date();
    let now = chrono::Local::now().naive_local();
    // Only accent a "now" row when today is actually in the displayed week -
    // paging to another week (H/L) has no current-hour row to show.
    let now_hour: Option<u32> = calendar_data.days.contains(&today).then(|| now.hour());

    let header_cells: Vec<Cell> = header_strings
        .iter()
        .enumerate()
        .map(|(idx, h)| {
            let style = if idx > 0 && calendar_data.days[idx - 1] == today {
                today_accent()
            } else {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            };
            Cell::from(h.as_str()).style(style)
        })
        .collect();

    let header = Row::new(header_cells).height(1).bottom_margin(1);

    // Build rows for each time slot with cursor highlight
    let rows: Vec<Row> = calendar_data
        .time_slots
        .iter()
        .enumerate()
        .map(|(slot_idx, slot)| {
            let is_now_row = now_hour == Some(slot.time.hour());

            let time_cell = if is_now_row {
                Cell::from(format!("▸{}", slot.time_label)).style(today_accent())
            } else {
                Cell::from(slot.time_label.clone())
            };
            let mut cells = vec![time_cell];

            for day_idx in 0..7 {
                let is_selected = day_idx == app.selected_day
                    && slot_idx == app.selected_time_slot
                    && app.calendar_input_mode == CalendarInputMode::Navigate;

                // Non-selected cells always show the first (topmost) task;
                // the selected cell shows whichever one `[`/`]` cycled to.
                let task_index = if is_selected { app.stack_index } else { 0 };
                let view = build_cell_view(&calendar_data, day_idx, slot.time, task_index);

                let headline = if is_selected && is_empty {
                    "[n: add block]".to_string()
                } else {
                    view.headline
                };

                let mut style = view.style;
                if is_now_row && calendar_data.days[day_idx] == today {
                    style = style.patch(Style::default().add_modifier(Modifier::UNDERLINED));
                }
                if is_selected {
                    style = Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD);
                }

                let lines = vec![
                    Line::from(headline),
                    Line::from(Span::styled(
                        view.overflow.unwrap_or_default(),
                        Style::default().add_modifier(Modifier::DIM),
                    )),
                ];
                cells.push(Cell::from(lines).style(style));
            }

            Row::new(cells).height(2)
        })
        .collect();

    // Calculate column widths: time column + 7 day columns
    let widths = vec![Constraint::Length(8)]
        .into_iter()
        .chain(std::iter::repeat_n(Constraint::Fill(1), 7))
        .collect::<Vec<_>>();

    // While a task is held (picked up with 'm'), the title swaps to drop
    // instructions so the pending action is always visible, not just in the
    // fading status line.
    let title = app.held_task.map_or_else(
        || "Weekly Calendar (t: todo, Tab: next view, h/l/j/k: move, H/L: week, n: block, s: schedule, a: add task, m: move task, u: unschedule, e: deadline, [/]: cycle stacked task, d: delete block, q: quit)".to_string(),
        |task_id| format!("Moving task #{task_id} - h/l/j/k: move cursor, m: drop here, Esc: cancel"),
    );

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(title))
        .column_spacing(1);

    f.render_widget(table, chunks[0]);

    // Status line for feedback from m/u/e/d (e.g. "No scheduled task here") -
    // without this the calendar view swallows that feedback entirely, since
    // (unlike the todo/email views) it never rendered app.status_message.
    if app.calendar_input_mode == CalendarInputMode::Navigate
        && let Some((msg, instant)) = &app.status_message
        && instant.elapsed() < std::time::Duration::from_secs(3)
    {
        let status = Paragraph::new(msg.as_str())
            .style(Style::default().fg(Color::Green))
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(status, chunks[1]);
    }

    // Render overlays based on calendar input mode
    match app.calendar_input_mode {
        CalendarInputMode::BlockForm => render_block_form_popup(f, app),
        CalendarInputMode::TaskPicker => render_task_picker(f, app),
        CalendarInputMode::TaskInput => render_calendar_task_input(f, app),
        CalendarInputMode::DeadlineInput => render_deadline_input(f, app),
        CalendarInputMode::Navigate => {}
    }
}

fn build_calendar_grid(app: &App) -> CalendarGrid<'_> {
    // Calculate week start (Monday)
    let today = chrono::Local::now().naive_local().date();
    let week_offset = app.calendar_week_offset.unwrap_or(0);
    let start_of_week = today + Duration::weeks(week_offset)
        - Duration::days(i64::from(today.weekday().num_days_from_monday()));

    // Generate 7 days starting from Monday
    let days: Vec<NaiveDate> = (0..7).map(|i| start_of_week + Duration::days(i)).collect();

    // Generate time slots (7am - 11pm in 1-hour increments)
    let time_slots: Vec<TimeSlot> = (7..23)
        .map(|hour| {
            // `hour` is always 7..23 from the range above, so this is never `None`.
            let time = NaiveTime::from_hms_opt(hour, 0, 0).unwrap_or(NaiveTime::MIN);
            TimeSlot {
                time,
                time_label: time.format("%I%p").to_string().to_lowercase(),
            }
        })
        .collect();

    CalendarGrid {
        days,
        time_slots,
        schedule_blocks: &app.cached_schedule_blocks,
        scheduled_tasks: &app.cached_scheduled_tasks,
        task_allocations: &app.cached_task_allocations,
    }
}
