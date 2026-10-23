//! The todo list view.

use super::centered_rect;
use crate::app::{
    App, ApplySession, ApplyStage, InputMode, cursor_col, link_label, parse_links, submit_enabled,
};
use crate::canvas::split_course;
use crate::urgency;
use chrono::NaiveTime;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

/// Todo badge style by effective priority.
///
/// Dim, neutral, red, then bold bright red as it gets more urgent. Only named ANSI colours (never RGB or indexed), so the terminal's own colour scheme
/// decides the actual shades; every colour in this file follows that rule.
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

/// Course-code colour: the same course always gets the same slot, so a class reads at a glance.
/// Cyan is left out because dates use it.
fn course_color(code: &str) -> Color {
    const PALETTE: [Color; 4] = [
        Color::Magenta,
        Color::Green,
        Color::LightBlue,
        Color::Yellow,
    ];
    let hash = code.bytes().fold(0usize, |acc, b| {
        acc.wrapping_mul(31).wrapping_add(usize::from(b))
    });
    PALETTE[hash % PALETTE.len()]
}

/// Date and deadline badge style: red family when close, calm cyan otherwise.
pub(super) fn date_style(level: i32) -> Style {
    if level >= 2 {
        urgency_style(level)
    } else {
        Style::default().fg(Color::Cyan)
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

    app.list_rows = usize::from(chunks[0].height.saturating_sub(2));
    let visual = app.visual_range();
    let items: Vec<ListItem> = app
        .tasks
        .iter()
        .enumerate()
        .map(|(row, task)| {
            let status = if task.completed { "[✓]" } else { "[ ]" };

            // Parse tags for display
            let tags: Vec<String> = task.tags.as_ref().map_or_else(Vec::new, |tags_json| {
                serde_json::from_str(tags_json).unwrap_or_default()
            });

            // Build the display line with colors and indicators
            let status_style = if task.completed {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let mut spans = vec![Span::styled(format!("{status} "), status_style)];

            // Priority, date and deadline badges share one urgency style
            let level = urgency::effective_priority(task, crate::clock::now());
            let badge_style = date_style(level);
            if let Some((_, badge)) = urgency::priority_badge(task, crate::clock::now()) {
                spans.push(Span::styled(format!("{badge} "), urgency_style(level)));
            }

            // Add schedule indicator with date and time info
            if let Some(scheduled) = task.scheduled_at {
                let scheduled = scheduled.with_timezone(&chrono::Local);
                let now = crate::clock::local_now();
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
                let badge = urgency::deadline_badge(deadline, crate::clock::local_now());
                spans.push(Span::styled(format!("{badge} "), badge_style));
            }

            // Add description with category color
            // Red is reserved for priority; uncategorised text keeps the terminal's own foreground.
            let text_style = if task.completed {
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::CROSSED_OUT)
            } else {
                match task.task_category.as_deref() {
                    Some("deepwork") => Style::default().fg(Color::LightBlue),
                    Some("admin") => Style::default().fg(Color::Yellow),
                    Some("learning") => Style::default().fg(Color::Cyan),
                    Some("fitness") => Style::default().fg(Color::Green),
                    _ => Style::default(),
                }
            };
            match split_course(&task.description) {
                Some((code, title)) => {
                    let code_style = if task.completed {
                        text_style
                    } else {
                        Style::default()
                            .fg(course_color(code))
                            .add_modifier(Modifier::BOLD)
                    };
                    spans.push(Span::styled(format!("{code} "), code_style));
                    spans.push(Span::styled(title, text_style));
                }
                None => spans.push(Span::styled(task.description.as_str(), text_style)),
            }

            // Links past the first (the Canvas page): `o` opens the first, `O` lists them all
            let extra = parse_links(task.links.as_deref()).len().saturating_sub(1);
            if extra > 0 {
                spans.push(Span::styled(
                    format!(" ⇗{extra}"),
                    Style::default().fg(Color::DarkGray),
                ));
            }

            if let Some(rule) = &task.recurrence {
                spans.push(Span::styled(
                    format!(" ↻ {rule}"),
                    Style::default().fg(Color::DarkGray),
                ));
            }

            // Add tags
            if !tags.is_empty() {
                spans.push(Span::styled(
                    format!(" #{}", tags.join(" #")),
                    Style::default().fg(Color::Magenta),
                ));
            }

            let item = ListItem::new(Line::from(spans));
            if visual.as_ref().is_some_and(|r| r.contains(&row)) {
                item.style(Style::default().add_modifier(Modifier::REVERSED))
            } else {
                item
            }
        })
        .collect();

    if app.tasks.is_empty() {
        app.todo_list_state.select(None);
    } else {
        app.todo_list_state.select(Some(app.selected));
    }

    let title = if app.visual_anchor.is_some() {
        "-- VISUAL -- (j/k/5j/gg/G: extend, d/x: delete, v/Esc: cancel)"
    } else {
        "To-Do (a: add, x: delete, Enter: done, c: calendar, q: quit, ?: help)"
    };
    let tasks_list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        // Bold only: a fg colour here would repaint the row's priority and course colours.
        .highlight_style(Style::default().add_modifier(Modifier::BOLD))
        .highlight_symbol("> ");

    f.render_stateful_widget(tasks_list, chunks[0], &mut app.todo_list_state);

    match app.input_mode {
        InputMode::Editing => {
            let title = if app.editing_task_id.is_some() {
                "Reword Task (Enter to save, Esc to cancel)"
            } else {
                "New Task (Enter to save, Esc to cancel) - Try: 'Submit report tomorrow #work urgent'"
            };
            let input_box = Paragraph::new(app.input_buffer.as_str())
                .style(Style::default().fg(Color::Yellow))
                .block(Block::default().borders(Borders::ALL).title(title));
            f.render_widget(input_box, chunks[1]);

            f.set_cursor_position(ratatui::layout::Position {
                x: chunks[1].x
                    + u16::try_from(cursor_col(&app.input_buffer, app.edit_cursor))
                        .unwrap_or(u16::MAX)
                    + 1,
                y: chunks[1].y + 1,
            });
        }
        InputMode::Search => {
            super::render_search_box(f, &app.input_buffer, app.edit_cursor, chunks[1]);
        }
        // Compose/snooze/rule-input only open from the Email view; nothing to draw over the todo
        // list for them.
        InputMode::EmailCompose | InputMode::EmailSnooze | InputMode::EmailRuleInput => {}
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

    if app.links_open {
        render_links_popup(f, app);
    }
    if let Some(session) = &app.apply {
        render_apply_popup(f, session);
    }
}

/// The job application popup (`A`): progress while the sidecar works, then the review to approve.
fn render_apply_popup(f: &mut Frame, session: &ApplySession) {
    let area = centered_rect(80, 60, f.area());
    f.render_widget(Clear, area);

    let mut lines = vec![Line::from(Span::styled(
        session.label.clone(),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    let hint = match &session.stage {
        ApplyStage::Working => {
            lines.push(Line::from("Working in the browser..."));
            "Esc: cancel"
        }
        ApplyStage::Submitting => {
            lines.push(Line::from("Submitting..."));
            "wait"
        }
        ApplyStage::Review {
            ats,
            filled,
            unanswered,
            screenshot,
        } => {
            lines.push(Line::from(format!("Site: {ats}")));
            lines.push(Line::from(format!(
                "Filled ({}): {}",
                filled.len(),
                filled.join(", ")
            )));
            if unanswered.is_empty() {
                lines.push(Line::styled(
                    "No required field left open",
                    Style::default().fg(Color::Green),
                ));
            } else {
                lines.push(Line::styled(
                    format!("Required, answer in the browser ({}):", unanswered.len()),
                    Style::default().fg(Color::Yellow),
                ));
                lines.extend(unanswered.iter().map(|q| Line::from(format!("  ▲ {q}"))));
            }
            if let Some(path) = screenshot {
                lines.push(Line::from(format!("Screenshot: {path}")));
            }
            if !submit_enabled() {
                lines.push(Line::styled(
                    "Submit is off: set TRIPTYCH_APPLY_SUBMIT=1",
                    Style::default().fg(Color::Yellow),
                ));
            }
            "y: submit, r: recheck, n: cancel"
        }
    };
    let popup = Paragraph::new(lines).wrap(Wrap { trim: false }).block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!("Apply ({hint})"))
            .style(Style::default().bg(Color::Black)),
    );
    f.render_widget(popup, area);
}

/// The links popup (`O`): one numbered row per link of the selected task.
fn render_links_popup(f: &mut Frame, app: &App) {
    let area = centered_rect(80, 50, f.area());
    f.render_widget(Clear, area);

    let width = usize::from(area.width.saturating_sub(8));
    let items: Vec<ListItem> = app
        .links
        .iter()
        .enumerate()
        .map(|(i, url)| {
            let number = if i < 9 {
                format!("{} ", i + 1)
            } else {
                "  ".to_string()
            };
            ListItem::new(Line::from(vec![
                Span::styled(number, Style::default().fg(Color::Cyan)),
                Span::raw(link_label(url, width)),
            ]))
        })
        .collect();
    let list = List::new(items)
        .highlight_style(
            Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ")
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Links (j/k: move, Enter or 1-9: open, Esc: close)")
                .style(Style::default().bg(Color::Black)),
        );

    let mut state = ratatui::widgets::ListState::default();
    state.select(Some(app.selected_link));
    f.render_stateful_widget(list, area, &mut state);
}
