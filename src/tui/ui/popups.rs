//! The calendar's overlay popups: block form, task picker and the two text inputs.

use super::centered_rect;
use crate::app::{App, BlockFormField};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
};

pub(super) fn render_block_form_popup(f: &mut Frame, app: &App) {
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
            Constraint::Length(1),
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

    // Rejection reasons (bad time, overlap) would otherwise be hidden behind this popup.
    if let Some((msg, instant)) = &app.status_message
        && instant.elapsed() < std::time::Duration::from_secs(3)
    {
        f.render_widget(
            Paragraph::new(msg.as_str()).style(Style::default().fg(Color::Red)),
            field_chunks[4],
        );
    }
}

pub(super) fn render_task_picker(f: &mut Frame, app: &App) {
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

pub(super) fn render_calendar_task_input(f: &mut Frame, app: &App) {
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

pub(super) fn render_deadline_input(f: &mut Frame, app: &App) {
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
