//! Local/UTC date and time helpers shared by the calendar and allocator.

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, Timelike, Utc};

/// Resolve a naive local wall-clock datetime to UTC without panicking on a DST transition.
///
/// An ambiguous time (fall-back) resolves to its earlier instant; a nonexistent time
/// (spring-forward gap) is nudged forward in hourly steps until a valid local time is found.
#[must_use]
pub fn resolve_local_datetime(naive: chrono::NaiveDateTime) -> DateTime<Utc> {
    for offset_hours in 0..=4 {
        if let Some(dt) = (naive + Duration::hours(offset_hours))
            .and_local_timezone(chrono::Local)
            .earliest()
        {
            return dt.with_timezone(&Utc);
        }
    }
    // Should be unreachable (DST gaps are at most a couple hours); fail safe.
    naive.and_utc()
}

/// `day` at 00:00:00. `and_hms_opt` only returns `None` for an out-of-range
/// hour/min/sec, never for literal 0/0/0, so the fallback is dead code kept
/// only to satisfy the no-`unwrap`-in-production-code lint.
pub(super) fn day_start(day: NaiveDate) -> chrono::NaiveDateTime {
    day.and_hms_opt(0, 0, 0)
        .unwrap_or_else(|| day.and_time(NaiveTime::MIN))
}

/// `day` at 23:59:59 - see [`day_start`].
pub(super) fn day_end(day: NaiveDate) -> chrono::NaiveDateTime {
    day.and_hms_opt(23, 59, 59)
        .unwrap_or_else(|| day.and_time(NaiveTime::MIN))
}

/// `day`'s weekday as the `schedule_blocks.day_of_week` column's `i32` encoding
/// (Monday = 0). `num_days_from_monday()` returns `0..=6`, so the cast never
/// truncates or wraps; centralized here so that fact is justified once.
pub(super) fn day_of_week_i32(day: NaiveDate) -> i32 {
    i32::try_from(day.weekday().num_days_from_monday()).unwrap_or(0)
}

/// Whether a span starting at `start` and lasting `minutes` overlaps the hour cell
/// `[hour:00, hour+1:00)`.
///
/// A multi-hour task or allocation (180 minutes from 7:00, or 60 from 12:30) shows in every hour
/// cell it touches, not just its start hour. A span of 0 minutes or less still occupies its
/// start hour. Counted in minutes from midnight, so a span running past midnight never wraps back
/// onto the morning. Shared by `cell_tasks`, `tui/ui/grid.rs`'s `cell_task_displays` and the
/// auto-scheduler's free-slot check, so all three agree.
#[must_use]
pub fn span_covers_hour(start: NaiveTime, minutes: i32, hour: u32) -> bool {
    let start = i64::from(start.num_seconds_from_midnight() / 60);
    let slot = i64::from(hour) * 60;
    start < slot + 60 && slot < start + i64::from(minutes.max(1))
}

#[must_use]
pub fn parse_time_string(time_str: &str) -> Option<NaiveTime> {
    if time_str.contains(':') {
        let parts: Vec<&str> = time_str.split(':').collect();
        if parts.len() >= 2 {
            let hour: u32 = parts[0].parse().ok()?;
            let minute: u32 = parts[1].parse().ok()?;
            let second: u32 = if parts.len() > 2 {
                parts[2].parse().ok()?
            } else {
                0
            };
            NaiveTime::from_hms_opt(hour, minute, second)
        } else {
            None
        }
    } else {
        None
    }
}
