use crate::app::{
    App, BlockFormField, CalendarInputMode, InputMode, ScheduleBlock, ViewMode,
    allocation_covers_hour, parse_time_string,
};
use chrono::{Datelike, Duration, NaiveDate, NaiveTime, Timelike};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, List, ListItem, Paragraph, Row, Table, Wrap},
};

pub fn ui(f: &mut Frame, app: &mut App) {
    match app.view_mode {
        ViewMode::TodoList => render_todo_view(f, app),
        ViewMode::Calendar => render_calendar_view(f, app),
        ViewMode::Email => render_email_view(f, app),
    }
}

fn render_email_view(f: &mut Frame, app: &mut App) {
    f.render_widget(Clear, f.area());

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([Constraint::Min(3), Constraint::Length(3)].as_ref())
        .split(f.area());

    let items: Vec<ListItem> = app
        .emails
        .iter()
        .map(|email| {
            let from = email.from_name.as_deref().unwrap_or(&email.from_addr);
            let date_text = email
                .date_utc
                .with_timezone(&chrono::Local)
                .format("%m/%d %H:%M")
                .to_string();

            let mut spans = vec![
                Span::styled(format!("({}) ", email.account), Style::default().fg(Color::Magenta)),
                Span::styled(format!("[{date_text}] "), Style::default().fg(Color::Green)),
                Span::styled(format!("{from:20} "), Style::default().fg(Color::Cyan)),
            ];

            let subject_style = if email.is_read {
                Style::default().fg(Color::White)
            } else {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            };
            spans.push(Span::styled(email.subject.clone(), subject_style));

            if email.task_id.is_some() {
                spans.push(Span::styled(" [task]", Style::default().fg(Color::Blue)));
            }

            ListItem::new(Line::from(spans))
        })
        .collect();

    if app.emails.is_empty() {
        app.email_list_state.select(None);
    } else {
        app.email_list_state.select(Some(app.selected_email));
    }

    let email_list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(
            "Email (m/Esc: todo, Tab: next view, j/k: move, v: view, Enter: convert to task, r: mark read)",
        ))
        .highlight_style(
            Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(email_list, chunks[0], &mut app.email_list_state);

    if let Some((msg, instant)) = &app.status_message
        && instant.elapsed() < std::time::Duration::from_secs(3)
    {
        let status = Paragraph::new(msg.as_str())
            .style(Style::default().fg(Color::Green))
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(status, chunks[1]);
    }

    if app.email_detail_open {
        render_email_detail_popup(f, app);
    }
}

fn render_email_detail_popup(f: &mut Frame, app: &App) {
    let Some(email) = app.emails.get(app.selected_email) else {
        return;
    };

    let area = centered_rect(80, 80, f.area());
    f.render_widget(Clear, area);

    let from = email.from_name.as_deref().unwrap_or(&email.from_addr);
    let date_text = email
        .date_utc
        .with_timezone(&chrono::Local)
        .format("%a %b %d, %Y %l:%M %P")
        .to_string();

    let mut text = vec![
        Line::from(vec![
            Span::styled("From: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!("{} <{}>", from, email.from_addr)),
        ]),
        Line::from(vec![
            Span::styled("Date: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(date_text),
        ]),
        Line::from(""),
    ];
    let body = email
        .body_text
        .as_deref()
        .unwrap_or("(no body content)");
    text.extend(body.lines().map(Line::from));

    let popup = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("{} (Esc/v: close, j/k: scroll)", email.subject))
                .style(Style::default().bg(Color::Black)),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.email_detail_scroll, 0));

    f.render_widget(popup, area);
}

// One screen's worth of rendering - splitting it into helpers would scatter
// widget-building state without reducing its actual complexity.
#[allow(clippy::too_many_lines)]
fn render_todo_view(f: &mut Frame, app: &mut App) {
    f.render_widget(Clear, f.area());

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([Constraint::Min(3), Constraint::Length(3)].as_ref())
        .split(f.area());

    let items: Vec<ListItem> = app
        .tasks
        .iter()
        .map(|task| {
            let status = if task.completed { "[✓]" } else { "[ ]" };

            // Parse tags for display
            let tags: Vec<String> = task.tags.as_ref().map_or_else(Vec::new, |tags_json| {
                serde_json::from_str(tags_json).unwrap_or_default()
            });

            // Build the display line with colors and indicators
            let mut spans = vec![Span::raw(format!("{status} "))];

            // Add priority indicator with text
            match task.priority {
                3 => spans.push(Span::styled("[URGENT] ", Style::default().fg(Color::Red))),
                2 => spans.push(Span::styled("[HIGH] ", Style::default().fg(Color::Yellow))),
                1 => spans.push(Span::styled("[MED] ", Style::default().fg(Color::Blue))),
                _ => {}
            }

            // Add schedule indicator with date and time info
            if let Some(scheduled) = task.scheduled_at {
                let scheduled = scheduled.with_timezone(&chrono::Local);
                let now = chrono::Local::now();
                let scheduled_date = scheduled.date_naive();
                let today = now.date_naive();
                let tomorrow = today + chrono::Duration::days(1);

                let time_str = scheduled.format("%l:%M%P").to_string().trim().to_string();

                let date_text = if scheduled_date == today {
                    format!("[TODAY {time_str}]")
                } else if scheduled_date == tomorrow {
                    format!("[TMR {time_str}]")
                } else {
                    format!("[{} {}]", scheduled.format("%m/%d"), time_str)
                };

                spans.push(Span::styled(
                    format!("{date_text} "),
                    Style::default().fg(Color::Green),
                ));
            }

            // Add description with category color
            let category_color = match task.task_category.as_deref() {
                Some("deepwork") => Color::Blue,
                Some("admin") => Color::Yellow,
                Some("learning") => Color::Cyan,
                Some("fitness") => Color::Red,
                _ => Color::White,
            };
            spans.push(Span::styled(
                task.description.as_str(),
                Style::default().fg(category_color),
            ));

            // Add tags
            if !tags.is_empty() {
                spans.push(Span::styled(
                    format!(" #{}", tags.join(" #")),
                    Style::default().fg(Color::Cyan),
                ));
            }

            ListItem::new(Line::from(spans))
        })
        .collect();

    if app.tasks.is_empty() {
        app.todo_list_state.select(None);
    } else {
        app.todo_list_state.select(Some(app.selected));
    }

    let tasks_list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("To-Do (q: quit, c: calendar, m: email, Tab: next view, a: add, x: delete, s: schedule, k/j: move, ENTER: toggle)"),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(tasks_list, chunks[0], &mut app.todo_list_state);

    match app.input_mode {
        InputMode::Editing => {
            let input_box = Paragraph::new(app.input_buffer.as_str())
                .style(Style::default().fg(Color::Yellow))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("New Task (Enter to save, Esc to cancel) - Try: 'Submit report tomorrow #work urgent'"),
                );
            f.render_widget(input_box, chunks[1]);

            f.set_cursor_position(ratatui::layout::Position {
                x: chunks[1].x
                    + u16::try_from(app.input_buffer.chars().count()).unwrap_or(u16::MAX)
                    + 1,
                y: chunks[1].y + 1,
            });
        }
        InputMode::Normal => {
            if let Some((msg, instant)) = &app.status_message
                && instant.elapsed() < std::time::Duration::from_secs(3)
            {
                let status = Paragraph::new(msg.as_str())
                    .style(Style::default().fg(Color::Green))
                    .block(Block::default().borders(Borders::ALL));
                f.render_widget(status, chunks[1]);
            }
        }
    }
}

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
fn render_calendar_view(f: &mut Frame, app: &App) {
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

/// Per-frame render view over the app's cached weekly data. Borrows rather than
/// clones the cached vectors - they're rebuilt on every keystroke's redraw, so a
/// clone here would copy the whole week's schedule/tasks every frame for no reason.
struct CalendarGrid<'a> {
    days: Vec<NaiveDate>,
    time_slots: Vec<TimeSlot>,
    schedule_blocks: &'a [(NaiveDate, ScheduleBlock)],
    scheduled_tasks: &'a [(NaiveDate, NaiveTime, i64, String, i32)],
    task_allocations: &'a [(NaiveDate, NaiveTime, i64, String, i32, i32)],
}

struct TimeSlot {
    time: NaiveTime,
    time_label: String,
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

/// What one calendar cell shows, before per-frame emphasis (cursor selection,
/// current-hour accent) is patched on by the caller in `render_calendar_view`.
struct CellView {
    headline: String,
    /// `Some("position/total")` when more than one task shares this cell's
    /// hour - the grid is hour-granularity, so a second task at the same
    /// day+hour (two manual tasks, or a manual task and a deadline
    /// allocation, or two allocations whose blocks/durations happen to
    /// overlap the same hour) would otherwise be silently invisible. Which
    /// one is addressable via `m`/`u`/`e` is `App::stack_index`, cycled with
    /// `[`/`]` - see `App::cycle_stack_next`/`_prev`.
    overflow: Option<String>,
    style: Style,
}

/// Every task occupying one calendar cell: description, priority, and whether
/// it's a deadline allocation (true) or a manually-scheduled task (false).
/// Manual tasks first, matching `App::selected_cell_task`'s precedence.
fn cell_task_displays<'a>(
    grid: &CalendarGrid<'a>,
    day: NaiveDate,
    slot_time: NaiveTime,
) -> Vec<(&'a str, i32, bool)> {
    grid.scheduled_tasks
        .iter()
        .filter(|(d, t, ..)| *d == day && t.hour() == slot_time.hour())
        .map(|(_, _, _, desc, priority)| (desc.as_str(), *priority, false))
        .chain(
            grid.task_allocations
                .iter()
                .filter(|(d, start, _, _, minutes, _)| {
                    *d == day && allocation_covers_hour(*start, *minutes, slot_time.hour())
                })
                .map(|(_, _, _, desc, _, priority)| (desc.as_str(), *priority, true)),
        )
        .collect()
}

/// `task_index` selects which of the cell's tasks (if more than one shares
/// this hour) supplies the headline - `0` for every cell except the one the
/// cursor is on, which can cycle further with `[`/`]`. `overflow` always
/// reports "position/total" so a stack is visible even before cycling.
fn build_cell_view(
    grid: &CalendarGrid<'_>,
    day_idx: usize,
    slot_time: NaiveTime,
    task_index: usize,
) -> CellView {
    let day = grid.days[day_idx];

    // Parse time strings to NaiveTime for comparison
    let schedule_block = grid.schedule_blocks.iter().find(|(d, block)| {
        *d == day && {
            // Parse start_time and end_time strings to NaiveTime
            if let (Some(start), Some(end)) = (
                parse_time_string(&block.start_time),
                parse_time_string(&block.end_time),
            ) {
                start <= slot_time && end > slot_time
            } else {
                false
            }
        }
    });

    let tasks = cell_task_displays(grid, day, slot_time);
    let overflow = (tasks.len() > 1)
        .then(|| format!("{}/{}", task_index.min(tasks.len() - 1) + 1, tasks.len()));
    let selected_task = tasks.get(task_index.min(tasks.len().saturating_sub(1))).copied();

    match (schedule_block, selected_task) {
        (Some((_, block)), Some((task_desc, priority, is_allocation))) => {
            // Task scheduled in this block - high priority overrides block color
            let style = if priority >= 3 {
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            } else {
                get_block_style(&block.block_type).add_modifier(Modifier::BOLD)
            };
            let symbol = if is_allocation { "◆" } else { "●" };
            CellView {
                headline: format!("{} {}", symbol, truncate_text(task_desc, 12)),
                overflow,
                style,
            }
        }
        (Some((_, block)), None) => CellView {
            // Empty schedule block
            headline: format!("[{}]", block.block_type),
            overflow,
            style: get_block_style(&block.block_type),
        },
        (None, Some((task_desc, priority, is_allocation))) => {
            // Task without schedule block - use priority color
            let color = match priority {
                3 => Color::Red,
                2 => Color::Yellow,
                _ => Color::White,
            };
            let symbol = if is_allocation { "◇" } else { "•" };
            CellView {
                headline: format!("{} {}", symbol, truncate_text(task_desc, 12)),
                overflow,
                style: Style::default().fg(color),
            }
        }
        (None, None) => CellView {
            // Empty cell
            headline: String::new(),
            overflow: None,
            style: Style::default(),
        },
    }
}

fn get_block_style(block_type: &str) -> Style {
    let color = match block_type {
        "deepwork" | "deepwork_input" | "deepwork_output" => Color::Blue,
        "class" => Color::Green,
        "training" | "fitness" => Color::Red,
        "learning" => Color::Cyan,
        "admin" => Color::Yellow,
        "bio-maintenance" | "meal" => Color::Magenta,
        "break" => Color::Gray,
        "social" => Color::LightBlue,
        "planning" => Color::LightYellow,
        "project" => Color::LightMagenta,
        _ => Color::White,
    };
    Style::default().fg(color).bg(Color::Reset)
}

/// Truncate to at most `max_len` characters, appending "...". Counts chars, not
/// bytes, so it never splits a multi-byte UTF-8 character (unlike byte slicing).
fn truncate_text(text: &str, max_len: usize) -> String {
    if text.chars().count() > max_len {
        let truncated: String = text.chars().take(max_len.saturating_sub(3)).collect();
        format!("{truncated}...")
    } else {
        text.to_string()
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

fn render_block_form_popup(f: &mut Frame, app: &App) {
    let area = centered_rect(50, 40, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title("New Schedule Block (Tab: next, Enter: save, Esc: cancel)")
        .style(Style::default().bg(Color::Black));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let field_chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
        ])
        .split(inner);

    let form = &app.block_form;

    let highlight = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let normal = Style::default().fg(Color::White);

    // Block Type field
    let bt_style = if form.active_field == BlockFormField::BlockType {
        highlight
    } else {
        normal
    };
    let bt_text = format!("Type: {} (j/k to cycle)", form.block_type);
    f.render_widget(Paragraph::new(bt_text).style(bt_style), field_chunks[0]);

    // Start Time field
    let st_style = if form.active_field == BlockFormField::StartTime {
        highlight
    } else {
        normal
    };
    let st_text = format!("Start: {}", form.start_time);
    f.render_widget(Paragraph::new(st_text).style(st_style), field_chunks[1]);

    // End Time field
    let et_style = if form.active_field == BlockFormField::EndTime {
        highlight
    } else {
        normal
    };
    let et_text = format!("End: {}", form.end_time);
    f.render_widget(Paragraph::new(et_text).style(et_style), field_chunks[2]);

    // Title field
    let ti_style = if form.active_field == BlockFormField::Title {
        highlight
    } else {
        normal
    };
    let ti_text = format!("Title: {}", form.title);
    f.render_widget(Paragraph::new(ti_text).style(ti_style), field_chunks[3]);
}

fn render_task_picker(f: &mut Frame, app: &App) {
    let area = centered_rect(60, 50, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title("Schedule Task (j/k: navigate, Enter: assign, Esc: cancel)")
        .style(Style::default().bg(Color::Black));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let unscheduled = app.unscheduled_tasks();

    if unscheduled.is_empty() {
        let msg = Paragraph::new("No unscheduled tasks available.")
            .style(Style::default().fg(Color::Gray));
        f.render_widget(msg, inner);
        return;
    }

    let items: Vec<ListItem> = unscheduled
        .iter()
        .enumerate()
        .map(|(idx, task)| {
            let category_color = match task.task_category.as_deref() {
                Some("deepwork") => Color::Blue,
                Some("admin") => Color::Yellow,
                Some("learning") => Color::Cyan,
                Some("fitness") => Color::Red,
                _ => Color::White,
            };

            let prefix = if idx == app.task_picker_selected {
                "> "
            } else {
                "  "
            };
            let category_label = task.task_category.as_deref().unwrap_or("general");

            let line = Line::from(vec![
                Span::raw(prefix),
                Span::styled(
                    format!("[{category_label}] "),
                    Style::default().fg(category_color),
                ),
                Span::raw(&task.description),
            ]);

            ListItem::new(line)
        })
        .collect();

    let list = List::new(items).highlight_style(
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    );

    f.render_widget(list, inner);
}

fn render_calendar_task_input(f: &mut Frame, app: &App) {
    let area = centered_rect(50, 25, f.area());
    f.render_widget(Clear, area);

    let date = app.selected_cell_date();
    let time = app.selected_cell_time();
    let title = format!(
        "Add Task at {} {} (Enter: save, Esc: cancel)",
        date.format("%a %m/%d"),
        time.format("%I:%M%p").to_string().to_lowercase()
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .style(Style::default().bg(Color::Black));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let input_text =
        Paragraph::new(app.input_buffer.as_str()).style(Style::default().fg(Color::Yellow));
    f.render_widget(input_text, inner);

    f.set_cursor_position(ratatui::layout::Position {
        x: inner.x + u16::try_from(app.input_buffer.chars().count()).unwrap_or(u16::MAX),
        y: inner.y,
    });
}

fn render_deadline_input(f: &mut Frame, app: &App) {
    let area = centered_rect(50, 25, f.area());
    f.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title("New deadline - day name or 'tomorrow' (Enter: save, Esc: cancel)")
        .style(Style::default().bg(Color::Black));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let input_text =
        Paragraph::new(app.input_buffer.as_str()).style(Style::default().fg(Color::Yellow));
    f.render_widget(input_text, inner);

    f.set_cursor_position(ratatui::layout::Position {
        x: inner.x + u16::try_from(app.input_buffer.chars().count()).unwrap_or(u16::MAX),
        y: inner.y,
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn day(offset: i64) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 3, 10).unwrap() + Duration::days(offset)
    }

    fn time(hour: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(hour, 0, 0).unwrap()
    }

    #[test]
    fn cell_task_displays_returns_every_task_sharing_an_hour() {
        let d = day(0);
        let scheduled_tasks = vec![
            (d, time(9), 1, "manual one".to_string(), 1),
            (d, time(9), 2, "manual two".to_string(), 1),
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
            (d, time(9), 1, "first".to_string(), 1),
            (d, time(9), 2, "second".to_string(), 1),
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

        let one_task = vec![(d, time(9), 1, "only".to_string(), 1)];
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
}
