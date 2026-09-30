//! Keyboard dispatch for the TUI. `handle_key_event` is the single entry point
//! the event loop in main.rs calls per keypress; it routes on
//! (`InputMode`, `ViewMode`, `CalendarInputMode`) to the per-mode handlers below,
//! each translating raw `KeyCodes` into App method calls. Splitting this out of
//! main.rs keeps process bootstrapping (terminal setup, daemon wiring)
//! separate from "what does this key do in this mode".

use crate::app::{
    App, BlockFormField, BlockFormState, CalendarInputMode, Feed, InputMode, Motion, MotionKey,
    ViewMode, list_target,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Whether the event loop should keep running or exit after this key.
pub enum KeyOutcome {
    Continue,
    Quit,
}

fn set_error(app: &mut App, e: impl std::fmt::Display) {
    app.status_message = Some((format!("Error: {e}"), std::time::Instant::now()));
}

/// Lines the email popup can scroll through; `G` lands at the end and the render clamps to the real one.
const DETAIL_SCROLL_RANGE: usize = 1 << 16;

pub async fn handle_key_event(app: &mut App, key: KeyEvent) -> KeyOutcome {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl && key.code == KeyCode::Char('c') {
        return KeyOutcome::Quit;
    }
    match app.input_mode {
        InputMode::Normal => {
            if handle_motion(app, key) {
                return KeyOutcome::Continue;
            }
            // A Ctrl chord that is not a motion does nothing, rather than acting as its bare letter.
            if ctrl {
                return KeyOutcome::Continue;
            }
            match app.view_mode {
                ViewMode::TodoList => handle_todo_key(app, key.code).await,
                ViewMode::Calendar => handle_calendar_key(app, key.code).await,
                ViewMode::Email => handle_email_key(app, key.code).await,
            }
        }
        InputMode::Editing if !ctrl => {
            handle_editing_key(app, key.code).await;
            KeyOutcome::Continue
        }
        InputMode::Search if !ctrl => {
            handle_search_key(app, key.code).await;
            KeyOutcome::Continue
        }
        InputMode::EmailSnooze if !ctrl => {
            handle_email_snooze_key(app, key.code).await;
            KeyOutcome::Continue
        }
        InputMode::EmailRuleInput if !ctrl => {
            handle_email_rule_input_key(app, key.code).await;
            KeyOutcome::Continue
        }
        InputMode::EmailCompose => handle_email_compose_key(app, key).await,
        InputMode::Editing
        | InputMode::Search
        | InputMode::EmailSnooze
        | InputMode::EmailRuleInput => KeyOutcome::Continue,
    }
}

const fn motion_key(key: KeyEvent) -> MotionKey {
    match key.code {
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => MotionKey::Ctrl(c),
        KeyCode::Char(_) if key.modifiers.contains(KeyModifiers::ALT) => MotionKey::Other,
        KeyCode::Char(c) => MotionKey::Char(c),
        KeyCode::Up => MotionKey::Up,
        KeyCode::Down => MotionKey::Down,
        KeyCode::Left => MotionKey::Left,
        KeyCode::Right => MotionKey::Right,
        KeyCode::Esc => MotionKey::Esc,
        _ => MotionKey::Other,
    }
}

/// Views whose cursor moves with vim motions: both lists, the email popup and the calendar grid (but
/// not while one of its forms is open, and not while the drafts list popup is — that one owns `j`/`k`
/// directly, same as `CalendarInputMode::TaskPicker`).
fn motions_active(app: &App) -> bool {
    if app.drafts_open || app.folder_browser_open || app.rules_open {
        return false;
    }
    app.view_mode != ViewMode::Calendar || app.calendar_input_mode == CalendarInputMode::Navigate
}

/// Runs the key through the count/`g` parser and applies a finished motion to the current view.
/// Returns true when the key was used up (a motion, or part or cancellation of a prefix).
fn handle_motion(app: &mut App, key: KeyEvent) -> bool {
    if !motions_active(app) {
        app.key_prefix.clear();
        return false;
    }
    match app.key_prefix.feed(motion_key(key)) {
        Feed::Other => false,
        Feed::Consumed => true,
        Feed::Motion { motion, count } => {
            apply_motion(app, motion, count);
            true
        }
    }
}

fn apply_motion(app: &mut App, motion: Motion, count: Option<usize>) {
    let rows = app.list_rows;
    match app.view_mode {
        ViewMode::TodoList => {
            if let Some(row) = list_target(app.selected, app.tasks.len(), rows, motion, count) {
                app.selected = row;
            }
        }
        ViewMode::Calendar => app.calendar_apply_motion(motion, count),
        ViewMode::Email if app.email_detail_open => {
            let from = usize::from(app.email_detail_scroll);
            if let Some(line) = list_target(from, DETAIL_SCROLL_RANGE, rows, motion, count) {
                app.email_detail_scroll = u16::try_from(line).unwrap_or(u16::MAX);
            }
        }
        ViewMode::Email => {
            if let Some(row) =
                list_target(app.selected_email, app.emails.len(), rows, motion, count)
            {
                app.selected_email = row;
            }
        }
    }
}

async fn handle_todo_key(app: &mut App, code: KeyCode) -> KeyOutcome {
    // Motions never reach here (see `handle_motion`), so any other key but a delete or a `v` toggle
    // ends visual selection; Esc only ends it.
    if app.visual_anchor.is_some() && !matches!(code, KeyCode::Char('v' | 'V' | 'd' | 'D' | 'x')) {
        app.visual_anchor = None;
        if code == KeyCode::Esc {
            return KeyOutcome::Continue;
        }
    }
    match code {
        KeyCode::Char('q') => return KeyOutcome::Quit,
        KeyCode::Char('v' | 'V') => app.toggle_visual(),
        KeyCode::Char('c') => app.toggle_to_calendar().await,
        KeyCode::Char('m') => app.toggle_to_email().await,
        KeyCode::Tab => app.cycle_view_next().await,
        KeyCode::BackTab => app.cycle_view_prev().await,
        KeyCode::Char('a') => {
            app.input_mode = InputMode::Editing;
            app.editing_task_id = None;
            app.input_buffer.clear();
        }
        KeyCode::Char('r' | 'R') => app.start_task_reword(),
        KeyCode::Char('x' | 'd' | 'D') => {
            if let Err(e) = app.delete_selected_tasks().await {
                set_error(app, e);
            }
        }
        KeyCode::Char('s') => {
            if let Err(e) = app.auto_schedule_task().await {
                set_error(app, e);
            }
        }
        KeyCode::Enter => {
            if let Err(e) = app.toggle_completed().await {
                set_error(app, e);
            }
        }
        KeyCode::Char('/') => app.start_search(),
        KeyCode::Char('n') => app.search_step(true).await,
        KeyCode::Char('N') => app.search_step(false).await,
        _ => {}
    }
    KeyOutcome::Continue
}

async fn handle_calendar_key(app: &mut App, code: KeyCode) -> KeyOutcome {
    match app.calendar_input_mode {
        CalendarInputMode::Navigate => return handle_calendar_navigate_key(app, code).await,
        CalendarInputMode::BlockForm => handle_block_form_key(app, code).await,
        CalendarInputMode::TaskPicker => handle_task_picker_key(app, code).await,
        CalendarInputMode::TaskInput => handle_task_input_key(app, code).await,
        CalendarInputMode::DeadlineInput => handle_deadline_input_key(app, code),
        CalendarInputMode::CellDetail => handle_cell_detail_key(app, code).await,
    }
    KeyOutcome::Continue
}

async fn handle_calendar_navigate_key(app: &mut App, code: KeyCode) -> KeyOutcome {
    match code {
        KeyCode::Char('q') => return KeyOutcome::Quit,
        // Esc/t cancel an in-progress task move first, rather than leaving the
        // calendar with a task still held.
        KeyCode::Char('t') | KeyCode::Esc => {
            if app.held_task.is_some() {
                app.cancel_held_task();
            } else {
                app.toggle_to_todo().await;
            }
        }
        KeyCode::Char('H') => app.prev_week().await,
        KeyCode::Char('L') => app.next_week().await,
        KeyCode::Char('n') => {
            app.block_form = BlockFormState::new_at(app.selected_time_slot);
            app.calendar_input_mode = CalendarInputMode::BlockForm;
        }
        KeyCode::Char('s') => {
            app.task_picker_selected = 0;
            app.calendar_input_mode = CalendarInputMode::TaskPicker;
        }
        KeyCode::Char('a') => {
            app.input_buffer.clear();
            app.calendar_input_mode = CalendarInputMode::TaskInput;
        }
        // 'm' toggles pick-up/drop of a scheduled task, letting it be moved to
        // another cell in two keypresses.
        KeyCode::Char('m') => {
            if app.held_task.is_some() {
                if let Err(e) = app.drop_held_task().await {
                    set_error(app, e);
                }
            } else {
                app.pick_up_task_at_selected_cell();
            }
        }
        KeyCode::Char('u') => {
            if let Err(e) = app.unschedule_task_at_selected_cell().await {
                set_error(app, e);
            }
        }
        KeyCode::Char('e') => app.start_deadline_edit_at_selected_cell(),
        KeyCode::Enter | KeyCode::Char('v') => app.open_cell_detail(),
        // Cycle which task in a stacked cell (see the "+N more" overflow
        // indicator) subsequent m/u/e presses act on.
        KeyCode::Char(']') => app.cycle_stack_next(),
        KeyCode::Char('[') => app.cycle_stack_prev(),
        KeyCode::Char('d') => {
            if let Err(e) = app.delete_block_at_selected_cell().await {
                set_error(app, e);
            }
        }
        KeyCode::Tab => app.cycle_view_next().await,
        KeyCode::BackTab => app.cycle_view_prev().await,
        _ => {}
    }
    KeyOutcome::Continue
}

/// Cell detail popup: `j`/`k` move `stack_index` directly (motions are off outside `Navigate`), and
/// `m`/`u`/`e` act on that task exactly as they do from the grid.
async fn handle_cell_detail_key(app: &mut App, code: KeyCode) {
    let last = app.selected_cell_tasks().len().saturating_sub(1);
    match code {
        KeyCode::Esc | KeyCode::Enter => app.calendar_input_mode = CalendarInputMode::Navigate,
        KeyCode::Char('j') | KeyCode::Down => app.stack_index = (app.stack_index + 1).min(last),
        KeyCode::Char('k') | KeyCode::Up => app.stack_index = app.stack_index.saturating_sub(1),
        KeyCode::Char('m') => {
            app.calendar_input_mode = CalendarInputMode::Navigate;
            app.pick_up_task_at_selected_cell();
        }
        KeyCode::Char('u') => {
            if let Err(e) = app.unschedule_task_at_selected_cell().await {
                set_error(app, e);
            }
            app.stack_index = app
                .stack_index
                .min(app.selected_cell_tasks().len().saturating_sub(1));
            if app.selected_cell_tasks().is_empty() {
                app.calendar_input_mode = CalendarInputMode::Navigate;
            }
        }
        KeyCode::Char('e') => app.start_deadline_edit_at_selected_cell(),
        _ => {}
    }
}

async fn handle_block_form_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            app.calendar_input_mode = CalendarInputMode::Navigate;
        }
        KeyCode::Tab => app.block_form.next_field(),
        KeyCode::BackTab => app.block_form.prev_field(),
        KeyCode::Enter => {
            if !app.block_form.title.is_empty()
                && let Err(e) = app.create_schedule_block().await
            {
                set_error(app, e);
            }
        }
        KeyCode::Char(c) => match app.block_form.active_field {
            BlockFormField::BlockType => {
                if c == 'j' {
                    app.block_form.cycle_block_type(true);
                } else if c == 'k' {
                    app.block_form.cycle_block_type(false);
                }
            }
            BlockFormField::StartTime => {
                if c.is_ascii_digit() || c == ':' {
                    app.block_form.start_time.push(c);
                }
            }
            BlockFormField::EndTime => {
                if c.is_ascii_digit() || c == ':' {
                    app.block_form.end_time.push(c);
                }
            }
            BlockFormField::Title => {
                app.block_form.title.push(c);
            }
        },
        KeyCode::Backspace => match app.block_form.active_field {
            BlockFormField::StartTime => {
                app.block_form.start_time.pop();
            }
            BlockFormField::EndTime => {
                app.block_form.end_time.pop();
            }
            BlockFormField::Title => {
                app.block_form.title.pop();
            }
            BlockFormField::BlockType => {}
        },
        _ => {}
    }
}

async fn handle_task_picker_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            app.calendar_input_mode = CalendarInputMode::Navigate;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            let max = app.unscheduled_tasks().len().saturating_sub(1);
            if app.task_picker_selected < max {
                app.task_picker_selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.task_picker_selected = app.task_picker_selected.saturating_sub(1);
        }
        KeyCode::Enter => {
            if !app.unscheduled_tasks().is_empty()
                && let Err(e) = app.schedule_task_to_selected_cell().await
            {
                set_error(app, e);
            }
        }
        _ => {}
    }
}

async fn handle_task_input_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            app.input_buffer.clear();
            app.calendar_input_mode = CalendarInputMode::Navigate;
        }
        KeyCode::Enter => {
            let description = app.input_buffer.trim().to_string();
            if !description.is_empty()
                && let Err(e) = app.add_task_at_selected_cell(&description).await
            {
                set_error(app, e);
            }
            app.input_buffer.clear();
            app.calendar_input_mode = CalendarInputMode::Navigate;
        }
        KeyCode::Char(c) => {
            app.input_buffer.push(c);
        }
        KeyCode::Backspace => {
            app.input_buffer.pop();
        }
        _ => {}
    }
}

fn handle_deadline_input_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            app.input_buffer.clear();
            app.deadline_edit_task_id = None;
            app.calendar_input_mode = CalendarInputMode::Navigate;
        }
        KeyCode::Enter => app.submit_deadline_edit(),
        KeyCode::Char(c) => {
            app.input_buffer.push(c);
        }
        KeyCode::Backspace => {
            app.input_buffer.pop();
        }
        _ => {}
    }
}

async fn handle_email_key(app: &mut App, code: KeyCode) -> KeyOutcome {
    if app.folder_browser_open {
        return handle_folder_browser_key(app, code);
    }
    if app.drafts_open {
        return handle_drafts_key(app, code).await;
    }
    if app.rules_open {
        return handle_rules_key(app, code).await;
    }
    if app.email_detail_open {
        return handle_email_detail_key(app, code).await;
    }

    match code {
        KeyCode::Char('q') => return KeyOutcome::Quit,
        KeyCode::Char('m') | KeyCode::Esc => {
            app.toggle_to_todo().await;
        }
        KeyCode::Tab => app.cycle_view_next().await,
        KeyCode::BackTab => app.cycle_view_prev().await,
        KeyCode::Char('/') => app.start_search(),
        KeyCode::Char('n') => app.search_step(true).await,
        KeyCode::Char('N') => app.search_step(false).await,
        KeyCode::Char('v') => {
            if let Err(e) = app.open_selected_email().await {
                set_error(app, e);
            }
        }
        KeyCode::Enter => {
            app.convert_selected_email_to_task().await;
        }
        KeyCode::Char('s') => app.start_email_sync(true),
        KeyCode::Char('o') => {
            if let Err(e) = app.toggle_email_sort().await {
                app.status_message = Some((format!("Error: {e}"), std::time::Instant::now()));
            }
        }
        KeyCode::Char('r') => {
            if let Err(e) = app.mark_selected_email_read().await {
                set_error(app, e);
            }
        }
        KeyCode::Char('u') => {
            if let Err(e) = app.mark_selected_email_unread().await {
                set_error(app, e);
            }
        }
        KeyCode::Char('f') => {
            if let Err(e) = app.toggle_selected_star().await {
                set_error(app, e);
            }
        }
        KeyCode::Char('t') => {
            if let Err(e) = app.cycle_selected_category().await {
                set_error(app, e);
            }
        }
        KeyCode::Char('c') => app.start_compose_new(),
        KeyCode::Char('d') => app.delete_selected_email(),
        KeyCode::Char('a') => app.archive_selected_email(),
        KeyCode::Char('M') => {
            if let Err(e) = app.accept_meeting_invite().await {
                set_error(app, e);
            }
        }
        KeyCode::Char('D') => app.open_drafts_list().await,
        KeyCode::Char('B') => app.open_folder_browser(),
        KeyCode::Char('R') => app.open_rules_list().await,
        KeyCode::Char('z') => app.start_snooze_prompt(),
        KeyCode::Char('Z') => {
            if let Err(e) = app.toggle_show_snoozed().await {
                set_error(app, e);
            }
        }
        KeyCode::Char('x') => {
            if let Err(e) = app.unsnooze_selected_email().await {
                set_error(app, e);
            }
        }
        _ => handle_email_filter_key(app, code).await,
    }
    KeyOutcome::Continue
}

/// The email list's seven filter-cycle keys (`A`/`F`/`I`/`H`/`U`/`S`/`@`), split out of
/// `handle_email_key` to keep it under clippy's line-count lint. `@` (not a letter — every free
/// uppercase letter that reads naturally for "domain" is already claimed, `G` by the vim-motion
/// `gg`/`G` top/bottom pair) cycles the sender-domain filter. A no-op for any other key.
async fn handle_email_filter_key(app: &mut App, code: KeyCode) {
    let result = match code {
        KeyCode::Char('A') => app.cycle_account_filter().await,
        KeyCode::Char('F') => app.cycle_folder_filter().await,
        KeyCode::Char('I') => app.cycle_focus_filter().await,
        KeyCode::Char('H') => app.cycle_attachment_filter().await,
        KeyCode::Char('U') => app.cycle_unread_filter().await,
        KeyCode::Char('S') => app.cycle_starred_filter().await,
        KeyCode::Char('@') => app.cycle_domain_filter().await,
        _ => return,
    };
    if let Err(e) = result {
        set_error(app, e);
    }
}

/// Keys while typing a snooze spec (`z` in the email list), e.g. `10m`, `2h`, `3d`, `tomorrow`,
/// `nextweek`. Enter parses and applies it (`App::commit_snooze`); an invalid spec reports a status
/// message instead of applying anything. Esc cancels, matching `handle_search_key`.
async fn handle_email_snooze_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Enter => app.commit_snooze().await,
        KeyCode::Esc => app.cancel_snooze(),
        KeyCode::Char(c) => app.input_buffer.push(c),
        KeyCode::Backspace => {
            app.input_buffer.pop();
        }
        _ => {}
    }
}

/// Keys while the drafts list popup (`D` in the email list) is open: `j`/`k`/arrows move the
/// selection directly (not the vim-motion system — see `motions_active`), `Enter` reopens the
/// selected draft in the compose form, `d` deletes it, `Esc` closes the popup.
async fn handle_drafts_key(app: &mut App, code: KeyCode) -> KeyOutcome {
    match code {
        KeyCode::Esc => app.close_drafts_list(),
        KeyCode::Char('j') | KeyCode::Down => {
            if app.selected_draft + 1 < app.drafts.len() {
                app.selected_draft += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.selected_draft = app.selected_draft.saturating_sub(1);
        }
        KeyCode::Enter => app.resume_selected_draft(),
        KeyCode::Char('d') => app.delete_selected_draft().await,
        _ => {}
    }
    KeyOutcome::Continue
}

/// Keys while the rules popup (`R` in the email list) is open: `j`/`k`/arrows move the selection
/// directly (not vim motions — see `motions_active`), `n` starts typing a new rule spec
/// (`InputMode::EmailRuleInput`), `d` deletes the selected rule, `Esc` closes the popup.
async fn handle_rules_key(app: &mut App, code: KeyCode) -> KeyOutcome {
    match code {
        KeyCode::Esc => app.close_rules_list(),
        KeyCode::Char('j') | KeyCode::Down => {
            if app.selected_rule + 1 < app.rules.len() {
                app.selected_rule += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => app.selected_rule = app.selected_rule.saturating_sub(1),
        KeyCode::Char('n') => app.start_rule_input(),
        KeyCode::Char('d') => app.delete_selected_rule().await,
        _ => {}
    }
    KeyOutcome::Continue
}

/// Keys while typing a rule spec (`n` in the rules popup), e.g. `subject newsletter star`. Enter
/// parses and saves it (`App::commit_rule_input`); an invalid spec reports a status message
/// instead of adding anything. Esc cancels, matching `handle_email_snooze_key`.
async fn handle_email_rule_input_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Enter => app.commit_rule_input().await,
        KeyCode::Esc => app.cancel_rule_input(),
        KeyCode::Char(c) => app.input_buffer.push(c),
        KeyCode::Backspace => {
            app.input_buffer.pop();
        }
        _ => {}
    }
}

/// Keys while the folder-browser popup (`B` in the email list) is open: `j`/`k`/arrows move the
/// selection directly (not vim motions — see `motions_active`), `Enter` picks the highlighted
/// server folder and kicks off a one-shot sync of it (`App::browse_to_selected_folder`), `Esc`
/// closes the popup without picking anything.
fn handle_folder_browser_key(app: &mut App, code: KeyCode) -> KeyOutcome {
    match code {
        KeyCode::Esc => app.close_folder_browser(),
        KeyCode::Char('j') | KeyCode::Down => {
            if app.selected_discovered_folder + 1 < app.discovered_folders.len() {
                app.selected_discovered_folder += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.selected_discovered_folder = app.selected_discovered_folder.saturating_sub(1);
        }
        KeyCode::Enter => app.browse_to_selected_folder(),
        _ => {}
    }
    KeyOutcome::Continue
}

/// Keys while the email detail popup (`v`) is open: close it (scrolling is a motion, see
/// `handle_motion`), start a reply/reply-all/forward from the open message, save its
/// attachments to disk (`s`, no-ops if it has none), cycle its category tag (`t`, same as the
/// list), or accept a meeting invite as a task (`M`, `App::accept_meeting_invite`, no-ops if the
/// email has none). Doesn't fall through to the list keys below it, same as how
/// `CalendarInputMode::BlockForm` shadows `Navigate`'s bindings.
async fn handle_email_detail_key(app: &mut App, code: KeyCode) -> KeyOutcome {
    match code {
        KeyCode::Esc | KeyCode::Char('v') => app.close_email_detail(),
        KeyCode::Char('R') => app.start_reply(false),
        KeyCode::Char('A') => app.start_reply(true),
        KeyCode::Char('F') => app.start_forward(),
        KeyCode::Char('f') => {
            if let Err(e) = app.toggle_selected_star().await {
                set_error(app, e);
            }
        }
        KeyCode::Char('u') => {
            if let Err(e) = app.mark_selected_email_unread().await {
                set_error(app, e);
            }
        }
        KeyCode::Char('t') => {
            if let Err(e) = app.cycle_selected_category().await {
                set_error(app, e);
            }
        }
        KeyCode::Char('d') => app.delete_selected_email(),
        KeyCode::Char('a') => app.archive_selected_email(),
        KeyCode::Char('s') => app.save_selected_attachments(),
        KeyCode::Char('M') => {
            if let Err(e) = app.accept_meeting_invite().await {
                set_error(app, e);
            }
        }
        _ => {}
    }
    KeyOutcome::Continue
}

/// Keys while composing/replying/forwarding an email (`InputMode::EmailCompose`). `Ctrl-S` sends,
/// `Ctrl-D` saves as a draft and closes the form; every other Ctrl chord is dropped (matching the
/// top-level rule) so it never types into a field.
async fn handle_email_compose_key(app: &mut App, key: KeyEvent) -> KeyOutcome {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('s') => app.send_compose(),
            KeyCode::Char('d') => app.save_compose_as_draft().await,
            _ => {}
        }
        return KeyOutcome::Continue;
    }
    match key.code {
        KeyCode::Esc => app.cancel_compose(),
        KeyCode::Tab => app.compose_next_field(),
        KeyCode::BackTab => app.compose_prev_field(),
        KeyCode::Enter => app.compose_newline(),
        KeyCode::Char(c) => app.compose_push_char(c),
        KeyCode::Backspace => app.compose_backspace(),
        _ => {}
    }
    KeyOutcome::Continue
}

async fn handle_editing_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Enter => {
            let description = app.input_buffer.trim().to_string();
            if let Some(id) = app.editing_task_id.take() {
                if let Err(e) = app.commit_task_reword(id).await {
                    set_error(app, e);
                }
            } else if !description.is_empty() {
                app.submit_task(description, None, None, None);
            }
            app.input_mode = InputMode::Normal;
        }
        KeyCode::Char(c) => {
            app.input_buffer.push(c);
        }
        KeyCode::Backspace => {
            app.input_buffer.pop();
        }
        KeyCode::Esc => {
            app.editing_task_id = None;
            app.input_mode = InputMode::Normal;
        }
        _ => {}
    }
}

async fn handle_search_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Enter => app.commit_search().await,
        KeyCode::Esc => app.cancel_search(),
        KeyCode::Char(c) => app.input_buffer.push(c),
        // Backspace on an empty prompt leaves it, as in vim.
        KeyCode::Backspace if app.input_buffer.is_empty() => app.cancel_search(),
        KeyCode::Backspace => {
            app.input_buffer.pop();
        }
        _ => {}
    }
}
