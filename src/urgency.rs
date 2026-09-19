//! Display-time urgency: the priority a task was typed with, raised as its date nears.
//! Nothing here writes to the database, so an escalation disappears when the date moves away.

use crate::app::Task;
use chrono::{DateTime, Duration, Local, Utc};

/// The higher of the stored priority and what the nearest date demands: due within 24h (or
/// overdue) is Urgent, within 3 days High, within 7 days Medium. Finished tasks keep their own.
#[must_use]
pub fn effective_priority(task: &Task, now: DateTime<Utc>) -> i32 {
    if task.completed {
        return task.priority;
    }
    let Some(due) = [task.scheduled_at, task.deadline]
        .into_iter()
        .flatten()
        .min()
    else {
        return task.priority;
    };
    let left = due - now;
    let tiers = [
        (Duration::hours(24), 3),
        (Duration::days(3), 2),
        (Duration::days(7), 1),
    ];
    let floor = tiers
        .iter()
        .find(|(within, _)| left <= *within)
        .map_or(0, |&(_, level)| level);
    task.priority.max(floor)
}

/// `(effective level, badge text)`, e.g. `(3, "[URGENT↑]")`; the arrow marks an automatic raise.
#[must_use]
pub fn priority_badge(task: &Task, now: DateTime<Utc>) -> Option<(i32, String)> {
    let level = effective_priority(task, now);
    let name = match level {
        3 => "URGENT",
        2 => "HIGH",
        1 => "MED",
        0 => "LOW",
        _ => return None,
    };
    let raised = if level > task.priority { "↑" } else { "" };
    Some((level, format!("[{name}{raised}]")))
}

/// `[OVERDUE]`, `[DUE TODAY]`, `[DUE TMR]`, `[DUE Fri]` (within a week) or `[DUE 09/30]`.
/// The time is shown only when the deadline is not the usual end of day.
#[must_use]
pub fn deadline_badge(deadline: DateTime<Utc>, now: DateTime<Local>) -> String {
    let due = deadline.with_timezone(&Local);
    if due < now {
        return "[OVERDUE]".to_string();
    }
    let day = match (due.date_naive() - now.date_naive()).num_days() {
        0 => "TODAY".to_string(),
        1 => "TMR".to_string(),
        2..=6 => due.format("%a").to_string(),
        _ => due.format("%m/%d").to_string(),
    };
    let time = if due.format("%H:%M").to_string() == "23:59" {
        String::new()
    } else {
        format!(" {}", due.format("%l:%M%P").to_string().trim())
    };
    format!("[DUE {day}{time}]")
}
