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
            handle_editing_key(app, key.code);
            KeyOutcome::Continue
        }
        InputMode::Search if !ctrl => {
            handle_search_key(app, key.code);
            KeyOutcome::Continue
        }
        InputMode::Editing | InputMode::Search => KeyOutcome::Continue,
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
/// not while one of its forms is open).
fn motions_active(app: &App) -> bool {
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
            if let Some(row) = list_target(app.selected_email, app.emails.len(), rows, motion, count) {
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
            app.input_buffer.clear();
        }
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
        KeyCode::Char('n') => app.search_step(true),
        KeyCode::Char('N') => app.search_step(false),
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
    if app.email_detail_open {
        return handle_email_detail_key(app, code);
    }

    match code {
        KeyCode::Char('q') => return KeyOutcome::Quit,
        KeyCode::Char('m') | KeyCode::Esc => {
            app.toggle_to_todo().await;
        }
        KeyCode::Tab => app.cycle_view_next().await,
        KeyCode::BackTab => app.cycle_view_prev().await,
        KeyCode::Char('/') => app.start_search(),
        KeyCode::Char('n') => app.search_step(true),
        KeyCode::Char('N') => app.search_step(false),
        KeyCode::Char('v') => {
            if let Err(e) = app.open_selected_email().await {
                set_error(app, e);
            }
        }
        KeyCode::Enter => {
            app.convert_selected_email_to_task();
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
        _ => {}
    }
    KeyOutcome::Continue
}

/// Keys while the email detail popup (`v`) is open: close it (scrolling is a motion, see
/// `handle_motion`). Doesn't fall through to the list keys below it, same as how
/// `CalendarInputMode::BlockForm` shadows `Navigate`'s bindings.
const fn handle_email_detail_key(app: &mut App, code: KeyCode) -> KeyOutcome {
    if matches!(code, KeyCode::Esc | KeyCode::Char('v')) {
        app.close_email_detail();
    }
    KeyOutcome::Continue
}

fn handle_editing_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Enter => {
            let description = app.input_buffer.trim().to_string();
            if !description.is_empty() {
                app.submit_task(description, None);
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
            app.input_mode = InputMode::Normal;
        }
        _ => {}
    }
}

fn handle_search_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Enter => app.commit_search(),
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
