//! The email list view and its message popup.

use super::centered_rect;
use crate::app::{
    App, ComposeField, InputMode, Summary, before_cursor, compose_full_body, cursor_col,
    thread_count,
};
use crate::email::{EmailMessage, priority};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

/// Maps a [`crate::app::CATEGORY_ORDER`] colour name to a named ANSI colour (Slice 22) — no
/// `Color::Rgb`/`Indexed`, so the terminal's own theme applies, same convention as `src/urgency.rs`.
/// Ratatui has no true orange/purple variant, so `LightRed`/`Magenta` approximate them.
fn category_color(name: &str) -> Color {
    match name {
        "red" => Color::Red,
        "orange" => Color::LightRed,
        "yellow" => Color::Yellow,
        "green" => Color::Green,
        "blue" => Color::Blue,
        "purple" => Color::Magenta,
        _ => Color::White,
    }
}

/// Title-cases a category name (`"red"` -> `"Red"`) for the detail popup's "Category: " line.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + chars.as_str()
    })
}

/// One row of the email list: account/date/sender, `[folder]` tag when not `INBOX` (Slice 13),
/// priority badge, star, `[category]` tag (Slice 22), thread-count badge, subject (bold if
/// unread), `[attach]`/`[invite]`/`[task]` tags.
fn email_list_item(all_emails: &[EmailMessage], email: &EmailMessage) -> ListItem<'static> {
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
    // INBOX is the overwhelming common case; tagging only the rest keeps the merged view (which
    // now includes synced Archive mail, Slice 13) from repeating "INBOX" on every single row.
    if email.folder != "INBOX" {
        spans.push(Span::styled(
            format!("[{}] ", email.folder),
            Style::default().fg(Color::DarkGray),
        ));
    }
    // Focused is the common case once triage catches up; only tag the "Other" bulk mail so the
    // merged view stays clean, same reasoning as the folder tag above.
    if email.triage_focused == Some(false) {
        spans.push(Span::styled(
            "[other] ",
            Style::default().fg(Color::DarkGray),
        ));
    }

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
    if email.is_starred {
        spans.push(Span::styled("● ", Style::default().fg(Color::Yellow)));
    }
    if let Some(category) = &email.category {
        spans.push(Span::styled(
            format!("[{category}] "),
            Style::default().fg(category_color(category)),
        ));
    }
    let count = thread_count(all_emails, email.id);
    if count > 1 {
        spans.push(Span::styled(
            format!("[{count}] "),
            Style::default().fg(Color::DarkGray),
        ));
    }
    spans.push(Span::styled(email.subject.clone(), subject_style));

    if email.has_attachments {
        spans.push(Span::styled(" [attach]", Style::default().fg(Color::Gray)));
    }
    // Once accepted, task_id is set and the [task] tag below already covers it.
    if email.meeting_title.is_some() && email.task_id.is_none() {
        spans.push(Span::styled(" [invite]", Style::default().fg(Color::Cyan)));
    }
    if email.task_id.is_some() {
        spans.push(Span::styled(" [task]", Style::default().fg(Color::Blue)));
    }
    // A lapsed snooze's `snoozed_until` stays set until cleared (see the field's doc), so this
    // only tags a message that's genuinely still hidden by it right now.
    if let Some(until) = email.snoozed_until
        && until > chrono::Utc::now()
    {
        let until_text = until.with_timezone(&chrono::Local).format("%a %H:%M");
        spans.push(Span::styled(
            format!(" [snoozed until {until_text}]"),
            Style::default().fg(Color::DarkGray),
        ));
    }

    ListItem::new(Line::from(spans))
}

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
        .map(|email| email_list_item(&app.emails, email))
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
                .title("Email (m/Esc: todo, Tab: next view, j/k: move, /: search, v: view, c: compose, s: sync, o: order, Enter: to task, M: accept meeting, r: read, u: unread, f: star, t: category, d: delete, a: archive, D: drafts, A: account filter, F: folder filter, I: focus filter, H: attachment filter, U: unread filter, S: starred filter, @: domain filter, B: browse folders, R: rules, z: snooze, Z: snoozed view, x: unsnooze)")
                .title_top(
                    Line::from(format!(
                        "sorted by {}{}{}{}{}{}{}{}{} ",
                        app.email_sort.label(),
                        app.account_filter
                            .as_deref()
                            .map_or_else(String::new, |a| format!("  ·  account: {a}")),
                        app.folder_filter
                            .as_deref()
                            .map_or_else(String::new, |f| format!("  ·  folder: {f}")),
                        app.focus_filter.map_or_else(String::new, |focused| format!(
                            "  ·  focus: {}",
                            if focused { "Focused" } else { "Other" }
                        )),
                        app.attachment_filter.map_or_else(String::new, |has_attach| format!(
                            "  ·  attach: {}",
                            if has_attach { "Yes" } else { "No" }
                        )),
                        app.unread_filter.map_or_else(String::new, |unread| format!(
                            "  ·  unread: {}",
                            if unread { "Yes" } else { "No" }
                        )),
                        app.starred_filter.map_or_else(String::new, |starred| format!(
                            "  ·  starred: {}",
                            if starred { "Yes" } else { "No" }
                        )),
                        app.domain_filter
                            .as_deref()
                            .map_or_else(String::new, |d| format!("  ·  domain: {d}")),
                        if app.show_snoozed { "  ·  snoozed view" } else { "" }
                    ))
                    .right_aligned(),
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
        super::render_search_box(f, &app.input_buffer, app.edit_cursor, chunks[1]);
    } else if matches!(app.input_mode, InputMode::EmailSnooze) {
        super::render_snooze_box(f, &app.input_buffer, app.edit_cursor, chunks[1]);
    } else if matches!(app.input_mode, InputMode::EmailRuleInput) {
        super::render_rule_input_box(f, &app.input_buffer, app.edit_cursor, chunks[1]);
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
    if app.email_compose.is_some() {
        render_compose_popup(f, app);
    }
    if app.drafts_open {
        render_drafts_popup(f, app);
    }
    if app.folder_browser_open {
        render_folder_browser_popup(f, app);
    }
    if app.rules_open {
        render_rules_popup(f, app);
    }
}

/// The drafts list popup (`D`): account, `To`, subject and last-saved time per saved draft.
fn render_drafts_popup(f: &mut Frame, app: &App) {
    let area = centered_rect(70, 60, f.area());
    f.render_widget(Clear, area);

    let items: Vec<ListItem> = app
        .drafts
        .iter()
        .map(|draft| {
            let updated = draft
                .updated_at
                .with_timezone(&chrono::Local)
                .format("%m/%d %H:%M")
                .to_string();
            let subject = if draft.subject.is_empty() {
                "(no subject)"
            } else {
                &draft.subject
            };
            let to = if draft.to_addrs.is_empty() {
                "(no recipient)"
            } else {
                &draft.to_addrs
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("({}) ", draft.account),
                    Style::default().fg(Color::Magenta),
                ),
                Span::styled(format!("[{updated}] "), Style::default().fg(Color::Green)),
                Span::styled(format!("{to:20} "), Style::default().fg(Color::Cyan)),
                Span::raw(subject.to_string()),
            ]))
        })
        .collect();

    let list = if items.is_empty() {
        List::new(vec![ListItem::new("(no saved drafts)")])
    } else {
        List::new(items)
            .highlight_style(
                Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ")
    };
    let list = list.block(
        Block::default()
            .borders(Borders::ALL)
            .title("Drafts (j/k: move, Enter: resume, d: delete, Esc: close)")
            .style(Style::default().bg(Color::Black)),
    );

    let mut state = ratatui::widgets::ListState::default();
    if !app.drafts.is_empty() {
        state.select(Some(app.selected_draft));
    }
    f.render_stateful_widget(list, area, &mut state);
}

/// The folder-browser popup (`B`): every server folder discovered by the last
/// `App::open_folder_browser` LIST pass, one row per `(account, folder)` pair.
fn render_folder_browser_popup(f: &mut Frame, app: &App) {
    let area = centered_rect(70, 60, f.area());
    f.render_widget(Clear, area);

    let items: Vec<ListItem> = app
        .discovered_folders
        .iter()
        .map(|(account, folder)| {
            ListItem::new(Line::from(vec![
                Span::styled(format!("({account}) "), Style::default().fg(Color::Magenta)),
                Span::raw(folder.clone()),
            ]))
        })
        .collect();

    let list = if items.is_empty() {
        List::new(vec![ListItem::new("(no folders found)")])
    } else {
        List::new(items)
            .highlight_style(
                Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ")
    };
    let list = list.block(
        Block::default()
            .borders(Borders::ALL)
            .title("Folders (j/k: move, Enter: sync + view, Esc: close)")
            .style(Style::default().bg(Color::Black)),
    );

    let mut state = ratatui::widgets::ListState::default();
    if !app.discovered_folders.is_empty() {
        state.select(Some(app.selected_discovered_folder));
    }
    f.render_stateful_widget(list, area, &mut state);
}

/// The rules popup (`R`): every saved auto-action rule, one row per `email_rules` row.
fn render_rules_popup(f: &mut Frame, app: &App) {
    let area = centered_rect(70, 60, f.area());
    f.render_widget(Clear, area);

    let items: Vec<ListItem> = app
        .rules
        .iter()
        .map(|rule| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{:10} ", rule.match_field),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(
                    format!("{:20} ", rule.pattern),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    format!("-> {}", rule.action),
                    Style::default().fg(Color::Yellow),
                ),
            ]))
        })
        .collect();

    let list = if items.is_empty() {
        List::new(vec![ListItem::new("(no saved rules)")])
    } else {
        List::new(items)
            .highlight_style(
                Style::default()
                    .fg(Color::Blue)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ")
    };
    let list = list.block(
        Block::default()
            .borders(Borders::ALL)
            .title("Rules (j/k: move, n: new, d: delete, Esc: close)")
            .style(Style::default().bg(Color::Black)),
    );

    let mut state = ratatui::widgets::ListState::default();
    if !app.rules.is_empty() {
        state.select(Some(app.selected_rule));
    }
    f.render_stateful_widget(list, area, &mut state);
}

/// Appends the meeting-invite lines (title/time, location, accept hint) when `email` carries one
/// (Slice 19: `EmailMessage.meeting_title`). No-op otherwise.
fn push_meeting_invite_lines<'a>(text: &mut Vec<Line<'a>>, email: &'a EmailMessage) {
    let Some(title) = &email.meeting_title else {
        return;
    };
    let when = email.meeting_start.map_or_else(String::new, |start| {
        start
            .with_timezone(&chrono::Local)
            .format(" — %a %b %d, %l:%M %P")
            .to_string()
    });
    text.push(Line::from(Span::styled(
        format!("Meeting: {title}{when}"),
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )));
    if let Some(location) = &email.meeting_location {
        text.push(Line::from(vec![
            Span::styled("Location: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(location.clone()),
        ]));
    }
    if email.task_id.is_none() {
        text.push(Line::from(Span::styled(
            "Press M to accept as a task",
            Style::default().fg(Color::DarkGray),
        )));
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
    if email.is_starred {
        text.push(Line::from(Span::styled(
            "● Starred",
            Style::default().fg(Color::Yellow),
        )));
    }
    if let Some(category) = &email.category {
        text.push(Line::from(vec![
            Span::styled("Category: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                capitalize(category),
                Style::default().fg(category_color(category)),
            ),
        ]));
    }
    let thread_size = thread_count(&app.emails, email.id);
    if thread_size > 1 {
        text.push(Line::from(Span::styled(
            format!("{thread_size} messages in this thread"),
            Style::default().fg(Color::DarkGray),
        )));
    }
    push_meeting_invite_lines(&mut text, email);
    if let Some(attachments) = app.email_attachments.get(&email.id)
        && !attachments.is_empty()
    {
        let names = attachments
            .iter()
            .map(|a| a.filename.as_deref().unwrap_or("(unnamed)"))
            .collect::<Vec<_>>()
            .join(", ");
        text.push(Line::from(vec![
            Span::styled(
                "Attachments: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(names),
        ]));
    }
    let summary = match app.email_summaries.get(&email.id) {
        Some(Summary::Ready(text)) => Some(("Summary: ", text.as_str(), Color::Cyan)),
        Some(Summary::Pending) => Some(("Summary: ", "generating...", Color::DarkGray)),
        Some(Summary::Failed(why)) => Some(("Summary unavailable: ", *why, Color::DarkGray)),
        None => None,
    };
    if let Some((label, detail, colour)) = summary {
        text.push(Line::from(vec![
            Span::styled(
                label,
                Style::default().fg(colour).add_modifier(Modifier::BOLD),
            ),
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
                .title(format!(
                    "{} (Esc/v: close, j/k/gg/G: scroll, R: reply, A: reply-all, F: forward, f: star, u: unread, t: category, d: delete, a: archive, s: save attachments, M: accept meeting)",
                    email.subject
                ))
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

/// The compose/reply/forward form: `To`/`Cc`/`Subject` as single-line fields, `Body` as a
/// multi-line one with the quoted original (if any) appended read-only below it. The active
/// field is yellow with the terminal cursor placed at its end (fields are append/backspace-only,
/// no mid-string editing — matching the rest of this app's text inputs).
fn render_compose_popup(f: &mut Frame, app: &App) {
    let Some(compose) = app.email_compose.clone() else {
        return;
    };

    let area = centered_rect(80, 80, f.area());
    f.render_widget(Clear, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(
            [
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Length(3),
                Constraint::Min(3),
            ]
            .as_ref(),
        )
        .split(area);

    render_compose_field(
        f,
        chunks[0],
        "To",
        &compose.to,
        compose.active_field == ComposeField::To,
        app.edit_cursor,
    );
    render_compose_field(
        f,
        chunks[1],
        "Cc",
        &compose.cc,
        compose.active_field == ComposeField::Cc,
        app.edit_cursor,
    );
    render_compose_field(
        f,
        chunks[2],
        "Subject",
        &compose.subject,
        compose.active_field == ComposeField::Subject,
        app.edit_cursor,
    );

    let body_text = compose_full_body(
        &compose.body,
        compose.signature.as_deref(),
        compose.quoted.as_deref(),
    );
    let body_active = compose.active_field == ComposeField::Body;
    let body_style = if body_active {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let body = Paragraph::new(body_text)
        .style(body_style)
        .block(Block::default().borders(Borders::ALL).title(format!(
            "Body — {} (Tab: next field, Ctrl-S: send, Ctrl-D: save draft, Esc: cancel)",
            compose.account
        )))
        .wrap(Wrap { trim: false });
    f.render_widget(body, chunks[3]);

    if body_active {
        let (row, col) = cursor_position_in(before_cursor(&compose.body, app.edit_cursor));
        f.set_cursor_position(ratatui::layout::Position {
            x: chunks[3].x + 1 + u16::try_from(col).unwrap_or(u16::MAX),
            y: chunks[3].y + 1 + u16::try_from(row).unwrap_or(u16::MAX),
        });
    }
}

fn render_compose_field(
    f: &mut Frame,
    area: Rect,
    label: &str,
    value: &str,
    active: bool,
    caret: Option<usize>,
) {
    let style = if active {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };
    let field = Paragraph::new(value).style(style).block(
        Block::default()
            .borders(Borders::ALL)
            .title(label.to_string()),
    );
    f.render_widget(field, area);
    if active {
        f.set_cursor_position(ratatui::layout::Position {
            x: area.x + 1 + u16::try_from(cursor_col(value, caret)).unwrap_or(u16::MAX),
            y: area.y + 1,
        });
    }
}

/// (row, col) of the end of `body` (pass the text before the caret), for cursor placement — a plain newline count, not aware of
/// line-wrapping (fields here are append/backspace-only, so exact-to-the-wrap placement isn't
/// worth the complexity it'd add).
fn cursor_position_in(body: &str) -> (usize, usize) {
    let mut row = 0;
    let mut col = 0;
    for c in body.chars() {
        if c == '\n' {
            row += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (row, col)
}
