//! Pure rendering: one file per view, and `ui` dispatches on the current one.

mod calendar;
mod email;
mod grid;
mod popups;
mod todo;

pub use grid::{CalendarGrid, CellView, TimeSlot, build_cell_view, cell_task_displays};
pub use todo::urgency_style;

use crate::app::{App, ViewMode};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Color, Style},
    widgets::{Block, Borders, Paragraph},
};

pub fn ui(f: &mut Frame, app: &mut App) {
    match app.view_mode {
        ViewMode::TodoList => todo::render_todo_view(f, app),
        ViewMode::Calendar => calendar::render_calendar_view(f, app),
        ViewMode::Email => email::render_email_view(f, app),
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

/// The `/query` prompt shown under the todo and email lists while a search is being typed.
fn render_search_box(f: &mut Frame, query: &str, area: Rect) {
    let title = "Search (Enter: jump, Esc: cancel, n/N: next/previous)";
    let input = Paragraph::new(format!("/{query}"))
        .style(Style::default().fg(Color::Yellow))
        .block(Block::default().borders(Borders::ALL).title(title));
    f.render_widget(input, area);
    f.set_cursor_position(Position {
        x: area.x + u16::try_from(query.chars().count()).unwrap_or(u16::MAX) + 2,
        y: area.y + 1,
    });
}
