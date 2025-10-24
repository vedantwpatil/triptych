use crate::app::{App, InputMode, ScheduleBlock, ViewMode};
use chrono::{Datelike, Duration, NaiveDate, NaiveTime, Timelike};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table},
};

pub fn ui(f: &mut Frame, app: &App) {
    match app.view_mode {
        ViewMode::TodoList => render_todo_view(f, app),
        ViewMode::Calendar => render_calendar_view(f, app),
    }
}

fn render_todo_view(f: &mut Frame, app: &App) {
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
            let tags: Vec<String> = if let Some(tags_json) = &task.tags {
                serde_json::from_str(tags_json).unwrap_or_default()
            } else {
                Vec::new()
            };

            // Build the display line with colors and indicators
            let mut spans = vec![Span::raw(format!("{} ", status))];

            // Add priority indicator with text
            match task.priority {
                3 => spans.push(Span::styled("[HIGH] ", Style::default().fg(Color::Red))),
                2 => spans.push(Span::styled("[MED] ", Style::default().fg(Color::Yellow))),
                1 => spans.push(Span::styled("[LOW] ", Style::default().fg(Color::Blue))),
                _ => {}
            }

            // Add schedule indicator with actual date info
            if let Some(scheduled) = task.scheduled_at {
                let now = chrono::Utc::now();
                let scheduled_date = scheduled.date_naive();
                let today = now.date_naive();
                let tomorrow = today + chrono::Duration::days(1);

                let date_text = if scheduled_date == today {
                    "[TODAY]".to_string()
                } else if scheduled_date == tomorrow {
                    "[TOMORROW]".to_string()
                } else {
                    format!("[{}]", scheduled.format("%m/%d"))
                };

                spans.push(Span::styled(
                    format!("{} ", date_text),
                    Style::default().fg(Color::Green),
                ));
            }

            // Add description
            spans.push(Span::raw(&task.description));

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

    let mut state = ListState::default();
    state.select(Some(app.selected));

    let tasks_list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("To-Do (q: quit, a: add, x: delete, k/j: move, ENTER: toggle)"),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(tasks_list, chunks[0], &mut state);

    if let InputMode::Editing = app.input_mode {
        let input_box = Paragraph::new(app.input_buffer.as_str())
            .style(Style::default().fg(Color::Yellow))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("New Task (Enter to save, Esc to cancel) - Try: 'Submit report tomorrow #work urgent'"),
            );
        f.render_widget(input_box, chunks[1]);

        f.set_cursor_position(ratatui::layout::Position {
            x: chunks[1].x + app.input_buffer.chars().count() as u16 + 1,
            y: chunks[1].y + 1,
        });
    }
}

fn render_calendar_view(f: &mut Frame, app: &App) {
    f.render_widget(Clear, f.area());

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([Constraint::Min(3)].as_ref())
        .split(f.area());

    let calendar_data = build_calendar_grid(app);

    // Build header with weekday names
    let header_strings: Vec<String> = std::iter::once("Time".to_string())
        .chain(
            calendar_data
                .days
                .iter()
                .map(|d| d.format("%a %m/%d").to_string()),
        )
        .collect();

    let header_cells: Vec<Cell> = header_strings
        .iter()
        .map(|h| {
            Cell::from(h.as_str()).style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect();

    let header = Row::new(header_cells).height(1).bottom_margin(1);

    // Build rows for each time slot
    let rows: Vec<Row> = calendar_data
        .time_slots
        .iter()
        .map(|slot| {
            let mut cells = vec![Cell::from(slot.time_label.clone())];

            for day_idx in 0..7 {
                let cell_content = build_cell_content(&calendar_data, day_idx, &slot.time);
                cells.push(cell_content);
            }

            Row::new(cells).height(2)
        })
        .collect();

    // Calculate column widths: time column + 7 day columns
    let widths = vec![Constraint::Length(8)]
        .into_iter()
        .chain(std::iter::repeat(Constraint::Fill(1)).take(7))
        .collect::<Vec<_>>();

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Weekly Calendar (t: todo list, h/l: prev/next week, q: quit)"),
        )
        .column_spacing(1);

    f.render_widget(table, chunks[0]);
}

struct CalendarGrid {
    days: Vec<NaiveDate>,
    time_slots: Vec<TimeSlot>,
    schedule_blocks: Vec<(NaiveDate, ScheduleBlock)>,
    scheduled_tasks: Vec<(NaiveDate, NaiveTime, String)>,
}

struct TimeSlot {
    time: NaiveTime,
    time_label: String,
}

fn build_calendar_grid(app: &App) -> CalendarGrid {
    // Calculate week start (Monday)
    let today = chrono::Local::now().naive_local().date();
    let week_offset = app.calendar_week_offset.unwrap_or(0);
    let start_of_week = today + Duration::weeks(week_offset)
        - Duration::days(today.weekday().num_days_from_monday() as i64);

    // Generate 7 days starting from Monday
    let days: Vec<NaiveDate> = (0..7).map(|i| start_of_week + Duration::days(i)).collect();

    // Generate time slots (7am - 11pm in 1-hour increments)
    let time_slots: Vec<TimeSlot> = (7..23)
        .map(|hour| {
            let time = NaiveTime::from_hms_opt(hour, 0, 0).unwrap();
            TimeSlot {
                time,
                time_label: time.format("%I%p").to_string().to_lowercase(),
            }
        })
        .collect();

    // Use cached data from app
    let schedule_blocks = app.cached_schedule_blocks.clone();
    let scheduled_tasks = app.cached_scheduled_tasks.clone();

    CalendarGrid {
        days,
        time_slots,
        schedule_blocks,
        scheduled_tasks,
    }
}

fn build_cell_content<'a>(grid: &CalendarGrid, day_idx: usize, slot_time: &NaiveTime) -> Cell<'a> {
    let day = grid.days[day_idx];

    // Parse time strings to NaiveTime for comparison
    let schedule_block = grid.schedule_blocks.iter().find(|(d, block)| {
        *d == day && {
            // Parse start_time and end_time strings to NaiveTime
            if let (Some(start), Some(end)) = (
                parse_time_string(&block.start_time),
                parse_time_string(&block.end_time),
            ) {
                start <= *slot_time && end > *slot_time
            } else {
                false
            }
        }
    });

    // Check if there's a scheduled task at this time
    let task = grid
        .scheduled_tasks
        .iter()
        .find(|(d, t, _)| *d == day && t.hour() == slot_time.hour());

    match (schedule_block, task) {
        (Some((_, block)), Some((_, _, task_desc))) => {
            // Task scheduled in this block
            let style = get_block_style(&block.block_type).add_modifier(Modifier::BOLD);
            Cell::from(format!("● {}", truncate_text(task_desc, 12))).style(style)
        }
        (Some((_, block)), None) => {
            // Empty schedule block
            let style = get_block_style(&block.block_type);
            Cell::from(format!("[{}]", block.block_type)).style(style)
        }
        (None, Some((_, _, task_desc))) => {
            // Task without schedule block
            Cell::from(format!("• {}", truncate_text(task_desc, 12)))
                .style(Style::default().fg(Color::White))
        }
        (None, None) => {
            // Empty cell
            Cell::from("")
        }
    }
}

fn parse_time_string(time_str: &str) -> Option<NaiveTime> {
    // Assuming format is "HH:MM:SS" or "HH:MM"
    if time_str.contains(':') {
        let parts: Vec<&str> = time_str.split(':').collect();
        if parts.len() >= 2 {
            let hour: u32 = parts[0].parse().ok()?;
            let minute: u32 = parts[1].parse().ok()?;
            let second: u32 = if parts.len() > 2 {
                parts[2].parse().ok()?
            } else {
                0
            };
            NaiveTime::from_hms_opt(hour, minute, second)
        } else {
            None
        }
    } else {
        None
    }
}

fn get_block_style(block_type: &str) -> Style {
    let color = match block_type {
        "deepwork" => Color::Blue,
        "class" => Color::Green,
        "fitness" => Color::Red,
        "learning" => Color::Cyan,
        "admin" => Color::Yellow,
        "meal" => Color::Magenta,
        "break" => Color::Gray,
        "social" => Color::LightBlue,
        "planning" => Color::LightYellow,
        "project" => Color::LightMagenta,
        _ => Color::White,
    };
    Style::default().fg(color).bg(Color::Reset)
}

fn truncate_text(text: &str, max_len: usize) -> String {
    if text.len() > max_len {
        format!("{}...", &text[..max_len - 3])
    } else {
        text.to_string()
    }
}
