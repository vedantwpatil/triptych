use crate::app::{App, InputMode};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
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
            let status = if task.completed { "[x]" } else { "[ ]" };
            let content = format!("{} {}", status, task.description);
            ListItem::new(content)
        })
        .collect();

    let mut state = ListState::default();
    state.select(Some(app.selected));

    let tasks_list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("To-Do (q: quit, a: add, x: delete, k/j: move, ENTER: check's task on/off)"),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> "); // Symbol to show next to the selected item

    f.render_stateful_widget(tasks_list, chunks[0], &mut state);

    if let InputMode::Editing = app.input_mode {
        let input_box = Paragraph::new(app.input_buffer.as_str())
            .style(Style::default().fg(Color::Yellow))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("New Task (Enter to save, Esc to cancel)"),
            );
        f.render_widget(input_box, chunks[1]);

        f.set_cursor_position(
            // The new method takes a Position struct
            ratatui::layout::Position {
                x: chunks[1].x + app.input_buffer.chars().count() as u16 + 1,
                y: chunks[1].y + 1,
            },
        );
    }
}
