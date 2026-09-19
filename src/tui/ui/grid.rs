//! The calendar's per-cell model: what each hour cell shows, independent of the terminal.

use crate::app::{ScheduleBlock, allocation_covers_hour, parse_time_string};
use chrono::{NaiveDate, NaiveTime, Timelike};
use ratatui::style::{Color, Modifier, Style};

/// Per-frame render view over the app's cached weekly data.
///
/// Borrows rather than clones the cached vectors - they're rebuilt on every keystroke's redraw, so
/// a clone here would copy the whole week's schedule/tasks every frame for no reason.
#[derive(Debug)]
pub struct CalendarGrid<'a> {
    pub days: Vec<NaiveDate>,
    pub time_slots: Vec<TimeSlot>,
    pub schedule_blocks: &'a [(NaiveDate, ScheduleBlock)],
    pub scheduled_tasks: &'a [(NaiveDate, NaiveTime, i64, String, i32)],
    pub task_allocations: &'a [(NaiveDate, NaiveTime, i64, String, i32, i32)],
}

#[derive(Debug)]
pub struct TimeSlot {
    pub time: NaiveTime,
    pub time_label: String,
}

/// What one calendar cell shows, before per-frame emphasis (cursor selection,
/// current-hour accent) is patched on by the caller in `render_calendar_view`.
#[derive(Debug)]
pub struct CellView {
    pub headline: String,
    /// `Some("position/total")` when more than one task shares this cell's
    /// hour - the grid is hour-granularity, so a second task at the same
    /// day+hour (two manual tasks, or a manual task and a deadline
    /// allocation, or two allocations whose blocks/durations happen to
    /// overlap the same hour) would otherwise be silently invisible. Which
    /// one is addressable via `m`/`u`/`e` is `App::stack_index`, cycled with
    /// `[`/`]` - see `App::cycle_stack_next`/`_prev`.
    pub overflow: Option<String>,
    pub style: Style,
}

/// Every task occupying one calendar cell: description, priority, and whether
/// it's a deadline allocation (true) or a manually-scheduled task (false).
///
/// Manual tasks first, matching `App::selected_cell_task`'s precedence.
#[must_use]
pub fn cell_task_displays<'a>(
    grid: &CalendarGrid<'a>,
    day: NaiveDate,
    slot_time: NaiveTime,
) -> Vec<(&'a str, i32, bool)> {
    grid.scheduled_tasks
        .iter()
        .filter(|(d, t, ..)| *d == day && t.hour() == slot_time.hour())
        .map(|(_, _, _, desc, priority)| (desc.as_str(), *priority, false))
        .chain(
            grid.task_allocations
                .iter()
                .filter(|(d, start, _, _, minutes, _)| {
                    *d == day && allocation_covers_hour(*start, *minutes, slot_time.hour())
                })
                .map(|(_, _, _, desc, _, priority)| (desc.as_str(), *priority, true)),
        )
        .collect()
}

/// Build the render view of one calendar cell.
///
/// `task_index` selects which of the cell's tasks (if more than one shares this hour) supplies the
/// headline - `0` for every cell except the one the cursor is on, which can cycle further with
/// `[`/`]`. `overflow` always reports "position/total" so a stack is visible even before cycling.
#[must_use]
pub fn build_cell_view(
    grid: &CalendarGrid<'_>,
    day_idx: usize,
    slot_time: NaiveTime,
    task_index: usize,
) -> CellView {
    let day = grid.days[day_idx];

    // Parse time strings to NaiveTime for comparison
    let schedule_block = grid.schedule_blocks.iter().find(|(d, block)| {
        *d == day && {
            // Parse start_time and end_time strings to NaiveTime
            if let (Some(start), Some(end)) = (
                parse_time_string(&block.start_time),
                parse_time_string(&block.end_time),
            ) {
                start <= slot_time && end > slot_time
            } else {
                false
            }
        }
    });

    let tasks = cell_task_displays(grid, day, slot_time);
    let overflow = (tasks.len() > 1)
        .then(|| format!("{}/{}", task_index.min(tasks.len() - 1) + 1, tasks.len()));
    let selected_task = tasks
        .get(task_index.min(tasks.len().saturating_sub(1)))
        .copied();

    match (schedule_block, selected_task) {
        (Some((_, block)), Some((task_desc, priority, is_allocation))) => {
            // Task scheduled in this block - high priority overrides block color
            let style = if priority >= 3 {
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            } else {
                get_block_style(&block.block_type).add_modifier(Modifier::BOLD)
            };
            let symbol = if is_allocation { "◆" } else { "●" };
            CellView {
                headline: format!("{} {}", symbol, truncate_text(task_desc, 12)),
                overflow,
                style,
            }
        }
        (Some((_, block)), None) => CellView {
            // Empty schedule block
            headline: format!("[{}]", block.block_type),
            overflow,
            style: get_block_style(&block.block_type),
        },
        (None, Some((task_desc, priority, is_allocation))) => {
            // Task without schedule block - use priority color
            let color = match priority {
                3 => Color::Red,
                2 => Color::Yellow,
                _ => Color::White,
            };
            let symbol = if is_allocation { "◇" } else { "•" };
            CellView {
                headline: format!("{} {}", symbol, truncate_text(task_desc, 12)),
                overflow,
                style: Style::default().fg(color),
            }
        }
        (None, None) => CellView {
            // Empty cell
            headline: String::new(),
            overflow: None,
            style: Style::default(),
        },
    }
}

fn get_block_style(block_type: &str) -> Style {
    let color = match block_type {
        "deepwork" | "deepwork_input" | "deepwork_output" => Color::Blue,
        "class" => Color::Green,
        "training" | "fitness" => Color::Red,
        "learning" => Color::Cyan,
        "admin" => Color::Yellow,
        "bio-maintenance" | "meal" => Color::Magenta,
        "break" => Color::Gray,
        "social" => Color::LightBlue,
        "planning" => Color::LightYellow,
        "project" => Color::LightMagenta,
        _ => Color::White,
    };
    Style::default().fg(color).bg(Color::Reset)
}

/// Truncate to at most `max_len` characters, appending "...". Counts chars, not
/// bytes, so it never splits a multi-byte UTF-8 character (unlike byte slicing).
fn truncate_text(text: &str, max_len: usize) -> String {
    if text.chars().count() > max_len {
        let truncated: String = text.chars().take(max_len.saturating_sub(3)).collect();
        format!("{truncated}...")
    } else {
        text.to_string()
    }
}
