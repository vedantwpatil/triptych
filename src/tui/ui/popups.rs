//! The calendar's overlay popups: block form, task picker and the two text inputs.

use super::centered_rect;
use crate::app::{App, BlockFormField, cursor_col};
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

    let (field, label) = match form.active_field {
        BlockFormField::StartTime => (Some((&form.start_time, 1)), "Start: "),
        BlockFormField::EndTime => (Some((&form.end_time, 2)), "End: "),
        BlockFormField::Title => (Some((&form.title, 3)), "Title: "),
        BlockFormField::BlockType => (None, ""),
    };
    if let Some((text, row)) = field {
        let col = label.len() + cursor_col(text, app.edit_cursor);
        f.set_cursor_position(ratatui::layout::Position {
            x: field_chunks[row].x + u16::try_from(col).unwrap_or(u16::MAX),
            y: field_chunks[row].y,
        });
    }

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
        x: inner.x
            + u16::try_from(cursor_col(&app.input_buffer, app.edit_cursor)).unwrap_or(u16::MAX),
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
        x: inner.x
            + u16::try_from(cursor_col(&app.input_buffer, app.edit_cursor)).unwrap_or(u16::MAX),
        y: inner.y,
    });
}

/// Full details of every task in the selected calendar cell; `stack_index` is the cursor.
pub(super) fn render_cell_detail(f: &mut Frame, app: &App) {
    let area = centered_rect(60, 50, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Cell (j/k: select, m: move, u: unschedule, e: deadline, Esc: close)")
        .style(Style::default().bg(Color::Black));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let now = chrono::Local::now();
    let mut head: Vec<Line> = app
        .selected_cell_block()
        .into_iter()
        .flat_map(|b| {
            let desc = b.description.clone().filter(|d| !d.is_empty());
            std::iter::once(Line::styled(
                format!(
                    "[{}] {}  {}-{}",
                    b.block_type, b.title, b.start_time, b.end_time
                ),
                Style::default().add_modifier(Modifier::BOLD),
            ))
            .chain(desc.map(Line::from))
        })
        .collect();
    let lines: Vec<Line> = app
        .selected_cell_tasks()
        .iter()
        .enumerate()
        .filter_map(|(idx, cell)| Some((idx, cell, app.tasks.iter().find(|t| t.id == cell.id)?)))
        .flat_map(|(idx, cell, task)| {
            let mut style = Style::default();
            if idx == app.stack_index {
                style = style.bg(Color::DarkGray).add_modifier(Modifier::BOLD);
            }
            let kind = if cell.is_allocation { "auto" } else { "manual" };
            let badges = crate::urgency::priority_badge(task, now.to_utc())
                .map(|(_, badge)| badge)
                .into_iter()
                .chain(
                    task.deadline
                        .map(|d| crate::urgency::deadline_badge(d, now)),
                )
                .chain(task.tags.clone());
            let meta = std::iter::once(format!("    {kind}"))
                .chain(badges)
                .collect::<Vec<_>>()
                .join(", ");
            [
                Line::styled(
                    format!(
                        "{} {}",
                        if idx == app.stack_index { ">" } else { " " },
                        task.description
                    ),
                    style,
                ),
                Line::styled(meta, Style::default().fg(Color::Gray)),
            ]
        })
        .collect();
    if lines.is_empty() {
        head.push(Line::styled("No tasks", Style::default().fg(Color::Gray)));
    }
    head.extend(lines);
    f.render_widget(Paragraph::new(head), inner);
}
