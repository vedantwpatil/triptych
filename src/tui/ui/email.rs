//! The email list view and its message popup.

use super::centered_rect;
use crate::app::{App, InputMode, Summary};
use crate::email::priority;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

pub(super) fn render_email_view(f: &mut Frame, app: &mut App) {
    f.render_widget(Clear, f.area());

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([Constraint::Min(3), Constraint::Length(3)].as_ref())
        .split(f.area());

    app.list_rows = usize::from(chunks[0].height.saturating_sub(2));
    let items: Vec<ListItem> = app
        .emails
        .iter()
        .map(|email| {
            let from = email.from_name.as_deref().unwrap_or(&email.from_addr);
            let date_text = email
                .date_utc
                .with_timezone(&chrono::Local)
                .format("%m/%d %H:%M")
                .to_string();

            let mut spans = vec![
                Span::styled(
                    format!("({}) ", email.account),
                    Style::default().fg(Color::Magenta),
                ),
                Span::styled(format!("[{date_text}] "), Style::default().fg(Color::Green)),
                Span::styled(format!("{from:20} "), Style::default().fg(Color::Cyan)),
            ];

            let subject_style = if email.is_read {
                Style::default().fg(Color::White)
            } else {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            };
            if let Some(badge) = priority::level(priority::score(email)).badge() {
                spans.push(Span::styled(
                    format!("{badge} "),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ));
            }
            spans.push(Span::styled(email.subject.clone(), subject_style));

            if email.task_id.is_some() {
                spans.push(Span::styled(" [task]", Style::default().fg(Color::Blue)));
            }

            ListItem::new(Line::from(spans))
        })
        .collect();

    if app.emails.is_empty() {
        app.email_list_state.select(None);
    } else {
        app.email_list_state.select(Some(app.selected_email));
    }

    let email_list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Email (m/Esc: todo, Tab: next view, j/k: move, /: search, v: view, s: sync, o: order, Enter: to task, r: read)")
                .title_top(
                    Line::from(format!("sorted by {} ", app.email_sort.label())).right_aligned(),
                ),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    f.render_stateful_widget(email_list, chunks[0], &mut app.email_list_state);

    if matches!(app.input_mode, InputMode::Search) {
        super::render_search_box(f, &app.input_buffer, chunks[1]);
    } else if let Some((msg, instant)) = &app.status_message
        && instant.elapsed() < std::time::Duration::from_secs(3)
    {
        let status = Paragraph::new(msg.as_str())
            .style(Style::default().fg(Color::Green))
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(status, chunks[1]);
    }

    if app.email_detail_open {
        render_email_detail_popup(f, app);
    }
}

fn render_email_detail_popup(f: &mut Frame, app: &mut App) {
    let Some(email) = app.emails.get(app.selected_email) else {
        return;
    };

    let area = centered_rect(80, 80, f.area());
    f.render_widget(Clear, area);
    app.list_rows = usize::from(area.height.saturating_sub(2));

    let from = email.from_name.as_deref().unwrap_or(&email.from_addr);
    let date_text = email
        .date_utc
        .with_timezone(&chrono::Local)
        .format("%a %b %d, %Y %l:%M %P")
        .to_string();

    let mut text = vec![
        Line::from(vec![
            Span::styled("From: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!("{} <{}>", from, email.from_addr)),
        ]),
        Line::from(vec![
            Span::styled("Date: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(date_text),
        ]),
    ];
    let summary = match app.email_summaries.get(&email.id) {
        Some(Summary::Ready(text)) => Some(("Summary: ", text.as_str(), Color::Cyan)),
        Some(Summary::Pending) => Some(("Summary: ", "generating...", Color::DarkGray)),
        Some(Summary::Failed(why)) => Some(("Summary unavailable: ", *why, Color::DarkGray)),
        None => None,
    };
    if let Some((label, detail, colour)) = summary {
        text.push(Line::from(vec![
            Span::styled(label, Style::default().fg(colour).add_modifier(Modifier::BOLD)),
            Span::styled(detail, Style::default().fg(colour)),
        ]));
    }
    text.push(Line::from(""));
    let body = email.body_text.as_deref().unwrap_or("(no body content)");
    text.extend(body.lines().map(Line::from));

    let popup = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("{} (Esc/v: close, j/k/gg/G: scroll)", email.subject))
                .style(Style::default().bg(Color::Black)),
        )
        .wrap(Wrap { trim: false });

    // Wrapping depends on the popup width, so the scroll limit is only known here. Clamping the
    // stored value (not just the drawn one) keeps `k` responsive right after over-scrolling.
    let max_scroll = popup
        .line_count(area.width)
        .saturating_sub(usize::from(area.height));
    app.email_detail_scroll = app
        .email_detail_scroll
        .min(u16::try_from(max_scroll).unwrap_or(u16::MAX));

    f.render_widget(popup.scroll((app.email_detail_scroll, 0)), area);
}
