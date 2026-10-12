//! The calendar's side to-do panel: open tasks with their urgency and deadline badges.

use super::todo::{date_style, urgency_style};
use crate::{
    app::{App, CalendarInputMode},
    urgency,
};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

/// `●` marks a task already on the grid, `○` one that is not; the focused row is reversed.
pub(super) fn render_calendar_todo(f: &mut Frame, app: &App, area: Rect) {
    let focused = app.calendar_input_mode == CalendarInputMode::TodoPanel;
    let (title, border) = if focused {
        (
            "To-Do (Enter: schedule, x: done)",
            Style::default().fg(Color::Cyan),
        )
    } else {
        ("To-Do (T: focus)", Style::default())
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(border);

    let tasks = app.calendar_todo_tasks();
    if tasks.is_empty() {
        let empty = Paragraph::new("No open tasks.")
            .style(Style::default().fg(Color::Gray))
            .block(block);
        f.render_widget(empty, area);
        return;
    }

    // Scroll just far enough to keep the cursor row on screen.
    let rows = usize::from(area.height.saturating_sub(2));
    let skip = app
        .calendar_todo_selected
        .saturating_sub(rows.saturating_sub(1));
    let now = chrono::Utc::now();
    let items: Vec<ListItem> = tasks
        .iter()
        .enumerate()
        .skip(skip)
        .map(|(idx, task)| {
            let marker = if task.scheduled_at.is_some() {
                Span::styled("● ", Style::default().fg(Color::Cyan))
            } else {
                Span::styled("○ ", Style::default().fg(Color::DarkGray))
            };
            let mut spans = vec![marker];
            if let Some((level, badge)) = urgency::priority_badge(task, now) {
                spans.push(Span::styled(format!("{badge} "), urgency_style(level)));
            }
            if let Some(deadline) = task.deadline {
                let badge = urgency::deadline_badge(deadline, chrono::Local::now());
                let level = urgency::effective_priority(task, now);
                spans.push(Span::styled(format!("{badge} "), date_style(level)));
            }
            spans.push(Span::raw(task.description.as_str()));

            let item = ListItem::new(Line::from(spans));
            if focused && idx == app.calendar_todo_selected {
                item.style(Style::default().add_modifier(Modifier::REVERSED))
            } else {
                item
            }
        })
        .collect();
    f.render_widget(List::new(items).block(block), area);
}
