use crate::app::{App, InputMode};
use chrono::Datelike;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};

pub fn ui(f: &mut Frame, app: &App) {
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
