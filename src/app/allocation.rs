//! Deep-work capacity allocation: conflict classification, task-to-block allocation and reallocation.

use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Utc};

use super::{
    App,
    model::{BlockInstance, ScheduleBlock, TASK_COLUMNS, Task},
    time::{day_of_week_i32, parse_time_string, resolve_local_datetime},
};
use sqlx::sqlite::Sqlite;

/// How far ahead `reallocate_all_tasks` looks for free blocks.
///
/// A deadline past this horizon is never even considered, so a conflict on such a task doesn't mean
/// the schedule is full - see `ConflictReason::BeyondWindow`.
pub const ALLOCATION_WINDOW_DAYS: i64 = 14;

/// Why a deadline-bearing task didn't get all the minutes it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictReason {
    /// Deadline falls past the planning horizon, so blocks that might have fit
    /// it were never considered. Not necessarily a capacity problem.
    BeyondWindow,
    /// Deadline is inside the horizon and every eligible deepwork/admin block
    /// before it is already full (or none exists).
    OutOfCapacity,
}

impl std::fmt::Display for ConflictReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BeyondWindow => {
                write!(
                    f,
                    "deadline past the {ALLOCATION_WINDOW_DAYS}-day planning window"
                )
            }
            Self::OutOfCapacity => {
                write!(f, "no free deepwork/admin time before the deadline")
            }
        }
    }
}

/// Exclusive UTC end of the window `get_available_deepwork_blocks(ALLOCATION_WINDOW_DAYS)` covers.
///
/// Instances exist for `today ..= today + (ALLOCATION_WINDOW_DAYS - 1)`, so the first uncovered
/// instant is local midnight starting day `ALLOCATION_WINDOW_DAYS`.
#[must_use]
pub fn allocation_window_end(today: NaiveDate) -> DateTime<Utc> {
    resolve_local_datetime(
        (today + Duration::days(ALLOCATION_WINDOW_DAYS)).and_time(NaiveTime::MIN),
    )
}

#[must_use]
pub fn classify_conflict(deadline: DateTime<Utc>, window_end: DateTime<Utc>) -> ConflictReason {
    if deadline >= window_end {
        ConflictReason::BeyondWindow
    } else {
        ConflictReason::OutOfCapacity
    }
}

#[derive(Debug)]
pub struct TaskConflict {
    pub task_id: i64,
    pub description: String,
    pub needed_minutes: i32,
    pub allocated_minutes: i32,
    pub deadline: DateTime<Utc>,
    pub reason: ConflictReason,
}

#[derive(Debug, Default)]
pub struct AllocationResult {
    pub conflicts: Vec<TaskConflict>,
}

impl AllocationResult {
    /// A one-line summary of every conflict, grouped by reason, or `None` when
    /// there weren't any. Kept here so the TUI status message and the CLI's
    /// `schedule reallocate` output can't drift into different wording.
    #[must_use]
    pub fn conflict_summary(&self) -> Option<String> {
        if self.conflicts.is_empty() {
            return None;
        }

        let beyond_window = self
            .conflicts
            .iter()
            .filter(|c| c.reason == ConflictReason::BeyondWindow)
            .count();
        let out_of_capacity = self.conflicts.len() - beyond_window;

        let mut parts = Vec::new();
        if beyond_window > 0 {
            parts.push(format!(
                "{beyond_window} past the {ALLOCATION_WINDOW_DAYS}-day window"
            ));
        }
        if out_of_capacity > 0 {
            parts.push(format!("{out_of_capacity} out of block capacity"));
        }

        Some(format!(
            "{} task(s) not scheduled: {}",
            self.conflicts.len(),
            parts.join(", ")
        ))
    }
}

/// Block types eligible to receive task allocations: deepwork blocks primarily,
/// admin blocks as a fallback for low-cognitive tasks.
///
/// This is a deliberate subset of `BlockFormState::BLOCK_TYPES`, not a full
/// mirror of it — most block types (class, training, meal, ...) are correctly
/// never schedulable. If you add a new block type that SHOULD receive task
/// allocations (behaving like deepwork/admin), you must add it here too;
/// nothing keeps the two lists in sync automatically.
fn is_allocatable_block_type(block_type: &str) -> bool {
    matches!(
        block_type,
        "deepwork" | "deepwork_input" | "deepwork_output" | "admin"
    )
}

impl App {
    /// Get incomplete tasks with a deadline, earliest deadline first.
    /// Allocations are deadline-driven and additive: they never touch `scheduled_at`,
    /// which remains under manual/direct-scheduling control (`auto_schedule_task`,
    /// `schedule_task_to_selected_cell`). Tasks without a deadline are never reallocated.
    async fn get_tasks_by_deadline(&self) -> Result<Vec<Task>, sqlx::Error> {
        // Tasks already manually scheduled (scheduled_at set) are excluded: they're
        // under the user's direct control and must never be double-booked into a
        // second, deadline-driven allocation.
        let query = format!(
            "SELECT {TASK_COLUMNS} FROM tasks WHERE completed = 0 AND deadline IS NOT NULL AND scheduled_at IS NULL ORDER BY deadline ASC"
        );
        sqlx::query_as::<_, Task>(&query)
            .fetch_all(&self.db_pool)
            .await
    }

    /// Expand recurring `schedule_blocks` into concrete per-date instances over the
    /// next `days` days, keeping only block types eligible for task allocation.
    async fn get_available_deepwork_blocks(
        &self,
        days: i64,
    ) -> Result<Vec<BlockInstance>, sqlx::Error> {
        let blocks = sqlx::query_as::<_, ScheduleBlock>(
            "SELECT id, day_of_week, start_time, end_time, block_type, title, description, priority FROM schedule_blocks"
        )
        .fetch_all(&self.db_pool)
        .await?;

        let now = chrono::Local::now();
        let today = now.naive_local().date();
        let current_time = now.time();
        let mut instances: Vec<BlockInstance> = Vec::new();

        for day_offset in 0..days {
            let date = today + Duration::days(day_offset);
            let dow = day_of_week_i32(date);

            for block in &blocks {
                if block.day_of_week != dow || !is_allocatable_block_type(&block.block_type) {
                    continue;
                }
                let (Some(start_time), Some(end_time)) = (
                    parse_time_string(&block.start_time),
                    parse_time_string(&block.end_time),
                ) else {
                    continue;
                };

                // Skip blocks on today that have already fully elapsed.
                if date == today && end_time <= current_time {
                    continue;
                }

                instances.push(BlockInstance {
                    date,
                    start_time,
                    end_time,
                });
            }
        }

        instances.sort_by_key(|b| (b.date, b.start_time));
        Ok(instances)
    }

    /// Remaining free minutes in this block instance, given minutes already used.
    fn block_has_capacity(
        usage: &std::collections::HashMap<(NaiveDate, NaiveTime), i64>,
        block: &BlockInstance,
        needed_minutes: i64,
    ) -> bool {
        let used = usage
            .get(&(block.date, block.start_time))
            .copied()
            .unwrap_or(0);
        block.capacity_minutes() - used >= needed_minutes.min(1)
    }

    async fn clear_all_allocations(
        tx: &mut sqlx::Transaction<'_, Sqlite>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM task_block_allocations")
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    /// Greedily allocate `needed_minutes` of a task across the given blocks (in order),
    /// recording usage as it goes. Returns the number of minutes actually allocated.
    async fn allocate_task_to_blocks(
        tx: &mut sqlx::Transaction<'_, Sqlite>,
        task_id: i64,
        needed_minutes: i64,
        available: &[&BlockInstance],
        usage: &mut std::collections::HashMap<(NaiveDate, NaiveTime), i64>,
    ) -> Result<i64, sqlx::Error> {
        let mut remaining = needed_minutes;

        for block in available {
            if remaining <= 0 {
                break;
            }

            let used = usage
                .get(&(block.date, block.start_time))
                .copied()
                .unwrap_or(0);
            let free = block.capacity_minutes() - used;
            if free <= 0 {
                continue;
            }

            let take = remaining.min(free);
            // This task's own slice of the block, not the block's overall bounds -
            // sequential per allocation so multiple tasks sharing one block land on
            // different start times instead of every one stacking on the block's
            // own start (see `span_covers_hour`/`cell_task_displays`, which
            // read these back and expect a real per-task start/duration).
            let allocation_start = block.start_time + Duration::minutes(used);
            let allocation_end = allocation_start + Duration::minutes(take);

            sqlx::query(
                "INSERT INTO task_block_allocations (task_id, block_date, block_start_time, block_end_time, allocated_minutes) VALUES (?, ?, ?, ?, ?)"
            )
            .bind(task_id)
            .bind(block.date.to_string())
            .bind(allocation_start.format("%H:%M").to_string())
            .bind(allocation_end.format("%H:%M").to_string())
            .bind(i32::try_from(take).unwrap_or(i32::MAX))
            .execute(&mut **tx)
            .await?;

            usage.insert((block.date, block.start_time), used + take);
            remaining -= take;
        }

        Ok(needed_minutes - remaining)
    }

    /// Reallocate every incomplete, deadline-bearing task to available deepwork/admin
    /// blocks in the next two weeks, earliest-deadline-first. Additive to the existing
    /// `scheduled_at`-based flow: this only ever writes `task_block_allocations` rows.
    /// The clear-and-rebuild runs inside a transaction so a mid-run error leaves the
    /// previous allocations intact rather than a half-rewritten table.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn reallocate_all_tasks(&mut self) -> Result<AllocationResult, sqlx::Error> {
        let tasks = self.get_tasks_by_deadline().await?;
        let blocks = self
            .get_available_deepwork_blocks(ALLOCATION_WINDOW_DAYS)
            .await?;
        let window_end = allocation_window_end(chrono::Local::now().naive_local().date());

        let mut tx = self.db_pool.begin().await?;
        Self::clear_all_allocations(&mut tx).await?;

        let mut block_usage: std::collections::HashMap<(NaiveDate, NaiveTime), i64> =
            std::collections::HashMap::new();
        let mut conflicts = Vec::new();

        for task in &tasks {
            let Some(deadline) = task.deadline else {
                continue;
            };
            let needed_minutes = i64::from(task.duration_minutes.unwrap_or(90));

            let available: Vec<&BlockInstance> = blocks
                .iter()
                .filter(|b| b.start_datetime_utc() < deadline)
                .filter(|b| Self::block_has_capacity(&block_usage, b, 1))
                .collect();

            let allocated = Self::allocate_task_to_blocks(
                &mut tx,
                task.id,
                needed_minutes,
                &available,
                &mut block_usage,
            )
            .await?;

            if allocated < needed_minutes {
                conflicts.push(TaskConflict {
                    task_id: task.id,
                    description: task.description.clone(),
                    needed_minutes: i32::try_from(needed_minutes).unwrap_or(i32::MAX),
                    allocated_minutes: i32::try_from(allocated).unwrap_or(i32::MAX),
                    deadline,
                    reason: classify_conflict(deadline, window_end),
                });
            }
        }

        tx.commit().await?;

        self.load_tasks().await?;
        self.refresh_calendar_data().await;

        Ok(AllocationResult { conflicts })
    }

    /// Called after a task with a deadline is added, so the schedule stays current.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn on_task_changed(&mut self) -> Result<(), sqlx::Error> {
        let result = self.reallocate_all_tasks().await?;

        if let Some(summary) = result.conflict_summary() {
            self.status_message = Some((format!("Warning: {summary}"), std::time::Instant::now()));
        }

        Ok(())
    }
}
