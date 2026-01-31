use crate::app::{App, BlockFormField, CalendarInputMode, InputMode, ScheduleBlock, ViewMode};
use chrono::{Datelike, Duration, NaiveDate, NaiveTime, Timelike};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
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

            // Add schedule indicator with date and time info
            if let Some(scheduled) = task.scheduled_at {
                let now = chrono::Utc::now();
                let scheduled_date = scheduled.date_naive();
                let today = now.date_naive();
                let tomorrow = today + chrono::Duration::days(1);

                let time_str = scheduled.format("%l:%M%P").to_string().trim().to_string();

                let date_text = if scheduled_date == today {
                    format!("[TODAY {}]", time_str)
                } else if scheduled_date == tomorrow {
                    format!("[TMR {}]", time_str)
                } else {
                    format!("[{} {}]", scheduled.format("%m/%d"), time_str)
                };

                spans.push(Span::styled(
                    format!("{} ", date_text),
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

    let mut state = ListState::default();
    state.select(Some(app.selected));

    let tasks_list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("To-Do (q: quit, a: add, x: delete, s: schedule, k/j: move, ENTER: toggle)"),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(tasks_list, chunks[0], &mut state);

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
                x: chunks[1].x + app.input_buffer.chars().count() as u16 + 1,
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

fn render_calendar_view(f: &mut Frame, app: &App) {
    f.render_widget(Clear, f.area());

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([Constraint::Min(3)].as_ref())
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

    let header_cells: Vec<Cell> = header_strings
        .iter()
        .enumerate()
        .map(|(idx, h)| {
            let style = if idx > 0 && calendar_data.days[idx - 1] == today {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
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
            let mut cells = vec![Cell::from(slot.time_label.clone())];

            for day_idx in 0..7 {
                let mut cell_content = build_cell_content(&calendar_data, day_idx, &slot.time);

                // Highlight selected cell
                if day_idx == app.selected_day
                    && slot_idx == app.selected_time_slot
                    && app.calendar_input_mode == CalendarInputMode::Navigate
                {
                    let display_text = if is_empty
                        && slot_idx == app.selected_time_slot
                        && day_idx == app.selected_day
                    {
                        "[n: add block]".to_string()
                    } else {
                        // Get existing text or empty
                        get_cell_text(&calendar_data, day_idx, &slot.time)
                    };
                    cell_content = Cell::from(display_text).style(
                        Style::default()
                            .bg(Color::DarkGray)
                            .add_modifier(Modifier::BOLD),
                    );
                }

                cells.push(cell_content);
            }

            Row::new(cells).height(2)
        })
        .collect();

    // Calculate column widths: time column + 7 day columns
    let widths = vec![Constraint::Length(8)]
        .into_iter()
        .chain(std::iter::repeat_n(Constraint::Fill(1), 7))
        .collect::<Vec<_>>();

    let title = "Weekly Calendar (t: todo, h/l/j/k: move, H/L: week, n: block, s: schedule, a: add task, q: quit)".to_string();

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(title))
        .column_spacing(1);

    f.render_widget(table, chunks[0]);

    // Render overlays based on calendar input mode
    match app.calendar_input_mode {
        CalendarInputMode::BlockForm => render_block_form_popup(f, app),
        CalendarInputMode::TaskPicker => render_task_picker(f, app),
        CalendarInputMode::TaskInput => render_calendar_task_input(f, app),
        CalendarInputMode::Navigate => {}
    }
}

struct CalendarGrid {
    days: Vec<NaiveDate>,
    time_slots: Vec<TimeSlot>,
    schedule_blocks: Vec<(NaiveDate, ScheduleBlock)>,
    scheduled_tasks: Vec<(NaiveDate, NaiveTime, String, i32)>,
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
        .find(|(d, t, _, _)| *d == day && t.hour() == slot_time.hour());

    match (schedule_block, task) {
        (Some((_, block)), Some((_, _, task_desc, priority))) => {
            // Task scheduled in this block - high priority overrides block color
            let style = if *priority >= 3 {
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            } else {
                get_block_style(&block.block_type).add_modifier(Modifier::BOLD)
            };
            Cell::from(format!("● {}", truncate_text(task_desc, 12))).style(style)
        }
        (Some((_, block)), None) => {
            // Empty schedule block
            let style = get_block_style(&block.block_type);
            Cell::from(format!("[{}]", block.block_type)).style(style)
        }
        (None, Some((_, _, task_desc, priority))) => {
            // Task without schedule block - use priority color
            let color = match priority {
                3 => Color::Red,
                2 => Color::Yellow,
                _ => Color::White,
            };
            Cell::from(format!("• {}", truncate_text(task_desc, 12)))
                .style(Style::default().fg(color))
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

fn truncate_text(text: &str, max_len: usize) -> String {
    if text.len() > max_len {
        format!("{}...", &text[..max_len - 3])
    } else {
        text.to_string()
    }
}

fn get_cell_text(grid: &CalendarGrid, day_idx: usize, slot_time: &NaiveTime) -> String {
    let day = grid.days[day_idx];

    let schedule_block = grid.schedule_blocks.iter().find(|(d, block)| {
        *d == day && {
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

    let task = grid
        .scheduled_tasks
        .iter()
        .find(|(d, t, _, _)| *d == day && t.hour() == slot_time.hour());

    match (schedule_block, task) {
        (Some((_, _block)), Some((_, _, task_desc, _))) => {
            format!("● {}", truncate_text(task_desc, 12))
        }
        (Some((_, block)), None) => {
            format!("[{}]", block.block_type)
        }
        (None, Some((_, _, task_desc, _))) => {
            format!("• {}", truncate_text(task_desc, 12))
        }
        (None, None) => String::new(),
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
                    format!("[{}] ", category_label),
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
        x: inner.x + app.input_buffer.chars().count() as u16,
        y: inner.y,
    });
}
