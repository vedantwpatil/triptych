//! The todo list view.

use crate::app::{App, InputMode};
use crate::urgency;
use chrono::NaiveTime;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};

/// Todo badge style by effective priority: dim, neutral, red, then bold bright red as it gets more
/// urgent. Only named ANSI colours, so the terminal's own palette decides the actual shades.
#[must_use]
pub fn urgency_style(level: i32) -> Style {
    match level {
        3 => Style::default()
            .fg(Color::LightRed)
            .add_modifier(Modifier::BOLD),
        2 => Style::default().fg(Color::Red),
        1 => Style::default().fg(Color::Gray),
        _ => Style::default().fg(Color::DarkGray),
    }
}

// One screen's worth of rendering - splitting it into helpers would scatter
// widget-building state without reducing its actual complexity.
#[allow(clippy::too_many_lines)]
pub(super) fn render_todo_view(f: &mut Frame, app: &mut App) {
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

            // Priority, date and deadline badges share one urgency style
            let level = urgency::effective_priority(task, chrono::Utc::now());
            let badge_style = urgency_style(level);
            if let Some((_, badge)) = urgency::priority_badge(task, chrono::Utc::now()) {
                spans.push(Span::styled(format!("{badge} "), badge_style));
            }

            // Add schedule indicator with date and time info
            if let Some(scheduled) = task.scheduled_at {
                let scheduled = scheduled.with_timezone(&chrono::Local);
                let now = chrono::Local::now();
                let scheduled_date = scheduled.date_naive();
                let today = now.date_naive();
                let tomorrow = today + chrono::Duration::days(1);

                let time_str = if scheduled.time() == NaiveTime::MIN {
                    String::new()
                } else {
                    format!(" {}", scheduled.format("%l:%M%P").to_string().trim())
                };

                let date_text = if scheduled_date == today {
                    format!("[TODAY{time_str}]")
                } else if scheduled_date == tomorrow {
                    format!("[TMR{time_str}]")
                } else {
                    format!("[{}{}]", scheduled.format("%m/%d"), time_str)
                };

                spans.push(Span::styled(format!("{date_text} "), badge_style));
            }

            if let (Some(deadline), false) = (task.deadline, task.completed) {
                let badge = urgency::deadline_badge(deadline, chrono::Local::now());
                spans.push(Span::styled(format!("{badge} "), badge_style));
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
