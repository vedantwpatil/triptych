//! Calendar cursor, week navigation, cell task math and held-task/deadline editing.

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, Timelike, Utc};
use std::sync::Arc;

use super::{
    App,
    model::{CalendarInputMode, ScheduleBlock, TASK_COLUMNS, Task},
    time::{
        allocation_covers_hour, day_end, day_of_week_i32, day_start, parse_time_string,
        resolve_local_datetime,
    },
};
use crate::nlp::ParsedItem;

/// A task occupying a calendar cell, resolved from the current week's cached data.
#[derive(Debug)]
pub struct CellTask {
    pub id: i64,
    /// True if this is a deadline-driven allocation rather than a manually
    /// scheduled task (see `reallocate_all_tasks`).
    pub is_allocation: bool,
}

/// Every task (manual + allocation) occupying a given day/hour, manual tasks
/// first.
///
/// Kept free of `&self` so it's testable without a DB pool or NLP parser. Callers that just want
/// "is there anything here" use `.next()`; `App::selected_cell_task` indexes into it with
/// `stack_index` when more than one task shares an hour (see
/// `App::cycle_stack_next`/`cycle_stack_prev`).
#[must_use]
pub fn cell_tasks(
    scheduled_tasks: &[(NaiveDate, NaiveTime, i64, String, i32)],
    task_allocations: &[(NaiveDate, NaiveTime, i64, String, i32, i32)],
    day: NaiveDate,
    hour: u32,
) -> Vec<CellTask> {
    scheduled_tasks
        .iter()
        .filter(|(d, t, ..)| *d == day && t.hour() == hour)
        .map(|(_, _, id, ..)| CellTask {
            id: *id,
            is_allocation: false,
        })
        .chain(
            task_allocations
                .iter()
                .filter(|(d, start, _, _, minutes, _)| {
                    *d == day && allocation_covers_hour(*start, *minutes, hour)
                })
                .map(|(_, _, id, ..)| CellTask {
                    id: *id,
                    is_allocation: true,
                }),
        )
        .collect()
}

/// Outcome of a background deadline parse (see `App::submit_deadline_edit`).
#[derive(Debug)]
pub struct DeadlineParse {
    task_id: i64,
    text: String,
    deadline: Option<DateTime<Utc>>,
}

impl App {
    pub async fn refresh_calendar_data(&mut self) {
        let today = chrono::Local::now().naive_local().date();
        let week_offset = self.calendar_week_offset.unwrap_or(0);
        let start_of_week = today + Duration::weeks(week_offset)
            - Duration::days(i64::from(today.weekday().num_days_from_monday()));

        let days: Vec<NaiveDate> = (0..7).map(|i| start_of_week + Duration::days(i)).collect();

        self.cached_schedule_blocks = self
            .get_week_schedule_internal(&days)
            .await
            .unwrap_or_default();

        self.cached_scheduled_tasks = self
            .get_scheduled_tasks_internal(&days)
            .await
            .unwrap_or_default();

        self.cached_task_allocations = self
            .get_week_allocations_internal(&days)
            .await
            .unwrap_or_default();
    }

    async fn get_week_allocations_internal(
        &self,
        days: &[NaiveDate],
    ) -> Result<Vec<(NaiveDate, NaiveTime, i64, String, i32, i32)>, sqlx::Error> {
        let start = days[0].to_string();
        let end = days[days.len() - 1].to_string();

        let rows: Vec<(String, String, i64, String, i32, i32)> = sqlx::query_as(
            r"
            SELECT a.block_date, a.block_start_time, t.id, t.description, a.allocated_minutes, t.priority
            FROM task_block_allocations a
            JOIN tasks t ON a.task_id = t.id
            WHERE a.block_date BETWEEN ? AND ? AND t.completed = 0
            ORDER BY a.block_date, a.block_start_time, t.id
            ",
        )
        .bind(start)
        .bind(end)
        .fetch_all(&self.db_pool)
        .await?;

        Ok(rows
            .into_iter()
            .filter_map(|(date_str, time_str, task_id, desc, minutes, priority)| {
                let date = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d").ok()?;
                let time = parse_time_string(&time_str)?;
                Some((date, time, task_id, desc, minutes, priority))
            })
            .collect())
    }

    async fn get_week_schedule_internal(
        &self,
        days: &[NaiveDate],
    ) -> Result<Vec<(NaiveDate, ScheduleBlock)>, sqlx::Error> {
        let blocks = sqlx::query_as::<_, ScheduleBlock>(
            "SELECT id, day_of_week, start_time, end_time, block_type, title, description, priority FROM schedule_blocks"
        )
        .fetch_all(&self.db_pool)
        .await?;

        let mut result = Vec::new();
        for block in blocks {
            for day in days {
                if day_of_week_i32(*day) == block.day_of_week {
                    result.push((*day, block.clone()));
                    break;
                }
            }
        }

        Ok(result)
    }

    async fn get_scheduled_tasks_internal(
        &self,
        days: &[NaiveDate],
    ) -> Result<Vec<(NaiveDate, NaiveTime, i64, String, i32)>, sqlx::Error> {
        let start = resolve_local_datetime(day_start(days[0]));
        let end = resolve_local_datetime(day_end(days[days.len() - 1]));

        let query = format!(
            "SELECT {TASK_COLUMNS} FROM tasks WHERE scheduled_at >= ? AND scheduled_at < ? AND completed = 0 ORDER BY scheduled_at, id"
        );
        let tasks = sqlx::query_as::<_, Task>(&query)
            .bind(start)
            .bind(end)
            .fetch_all(&self.db_pool)
            .await?;

        Ok(tasks
            .iter()
            .filter_map(|t| {
                t.scheduled_at.map(|dt| {
                    let local = dt.with_timezone(&chrono::Local);
                    (
                        local.date_naive(),
                        local.time(),
                        t.id,
                        t.description.clone(),
                        t.priority,
                    )
                })
            })
            .collect())
    }

    pub async fn next_week(&mut self) {
        let offset = self.calendar_week_offset.unwrap_or(0);
        self.calendar_week_offset = Some(offset + 1);
        self.stack_index = 0;
        self.refresh_calendar_data().await;
    }

    pub async fn prev_week(&mut self) {
        let offset = self.calendar_week_offset.unwrap_or(0);
        self.calendar_week_offset = Some(offset - 1);
        self.stack_index = 0;
        self.refresh_calendar_data().await;
    }

    // Calendar navigation methods
    pub const fn calendar_move_up(&mut self) {
        self.selected_time_slot = self.selected_time_slot.saturating_sub(1);
        self.stack_index = 0;
    }

    pub const fn calendar_move_down(&mut self) {
        if self.selected_time_slot < 15 {
            self.selected_time_slot += 1;
        }
        self.stack_index = 0;
    }

    pub const fn calendar_move_left(&mut self) {
        self.selected_day = self.selected_day.saturating_sub(1);
        self.stack_index = 0;
    }

    pub const fn calendar_move_right(&mut self) {
        if self.selected_day < 6 {
            self.selected_day += 1;
        }
        self.stack_index = 0;
    }

    /// Number of tasks (manual + allocation) occupying the selected cell -
    /// bounds `stack_index` when cycling with `[`/`]`.
    fn selected_cell_task_count(&self) -> usize {
        let day = self.selected_cell_date();
        let hour = self.selected_cell_time().hour();
        cell_tasks(
            &self.cached_scheduled_tasks,
            &self.cached_task_allocations,
            day,
            hour,
        )
        .len()
    }

    /// Move `stack_index` to the next task in the selected cell, wrapping
    /// around. No-op on a cell with 0 or 1 tasks - there's nothing to cycle to.
    pub fn cycle_stack_next(&mut self) {
        let count = self.selected_cell_task_count();
        if count > 1 {
            self.stack_index = (self.stack_index + 1) % count;
        }
    }

    /// Move `stack_index` to the previous task in the selected cell, wrapping
    /// around. No-op on a cell with 0 or 1 tasks.
    pub fn cycle_stack_prev(&mut self) {
        let count = self.selected_cell_task_count();
        if count > 1 {
            self.stack_index = (self.stack_index + count - 1) % count;
        }
    }

    #[must_use]
    pub fn selected_cell_date(&self) -> NaiveDate {
        let today = chrono::Local::now().naive_local().date();
        let week_offset = self.calendar_week_offset.unwrap_or(0);
        let start_of_week = today + Duration::weeks(week_offset)
            - Duration::days(i64::from(today.weekday().num_days_from_monday()));
        start_of_week + Duration::days(i64::try_from(self.selected_day).unwrap_or(0))
    }

    #[must_use]
    pub fn selected_cell_time(&self) -> NaiveTime {
        let hour = 7 + u32::try_from(self.selected_time_slot).unwrap_or(0);
        // `hour` is always in-range for a time-of-day, so this is never `None`.
        NaiveTime::from_hms_opt(hour, 0, 0).unwrap_or(NaiveTime::MIN)
    }

    /// The task `m`/`u`/`e` act on: the one at `stack_index` within the
    /// selected cell, not always the first - see `cycle_stack_next`/`_prev`.
    fn selected_cell_task(&self) -> Option<CellTask> {
        let day = self.selected_cell_date();
        let hour = self.selected_cell_time().hour();
        let tasks = cell_tasks(
            &self.cached_scheduled_tasks,
            &self.cached_task_allocations,
            day,
            hour,
        );
        let idx = self.stack_index.min(tasks.len().saturating_sub(1));
        tasks.into_iter().nth(idx)
    }

    /// Pick up the manually-scheduled task at the selected cell so it can be
    /// dropped on a new cell with `drop_held_task`. Deadline-driven allocations
    /// aren't draggable this way - move their deadline instead (`e`).
    pub fn pick_up_task_at_selected_cell(&mut self) {
        match self.selected_cell_task() {
            Some(CellTask {
                id,
                is_allocation: false,
                ..
            }) => {
                self.held_task = Some(id);
                self.status_message = Some((
                    "Task picked up - move cursor, m to drop, Esc to cancel".to_string(),
                    std::time::Instant::now(),
                ));
            }
            Some(CellTask {
                is_allocation: true,
                ..
            }) => {
                self.status_message = Some((
                    "Can't move a deadline allocation directly - edit its deadline with 'e'"
                        .to_string(),
                    std::time::Instant::now(),
                ));
            }
            None => {
                self.status_message = Some((
                    "No scheduled task here".to_string(),
                    std::time::Instant::now(),
                ));
            }
        }
    }

    /// Drop the held task (see `pick_up_task_at_selected_cell`) onto the selected cell.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn drop_held_task(&mut self) -> Result<(), sqlx::Error> {
        let Some(task_id) = self.held_task.take() else {
            return Ok(());
        };

        let datetime = resolve_local_datetime(
            self.selected_cell_date()
                .and_time(self.selected_cell_time()),
        );

        sqlx::query("UPDATE tasks SET scheduled_at = ? WHERE id = ?")
            .bind(datetime)
            .bind(task_id)
            .execute(&self.db_pool)
            .await?;

        self.load_tasks().await?;
        self.refresh_calendar_data().await;
        self.status_message = Some(("Task moved".to_string(), std::time::Instant::now()));
        Ok(())
    }

    /// Cancel an in-progress task move without changing its schedule.
    pub fn cancel_held_task(&mut self) {
        if self.held_task.take().is_some() {
            self.status_message = Some(("Move cancelled".to_string(), std::time::Instant::now()));
        }
    }

    /// Clear the manually-set schedule of the task at the selected cell, returning
    /// it to the unscheduled pool. Deadline-driven allocations are left alone -
    /// they're recomputed by `reallocate_all_tasks`, not directly unscheduled.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn unschedule_task_at_selected_cell(&mut self) -> Result<(), sqlx::Error> {
        match self.selected_cell_task() {
            Some(CellTask {
                id,
                is_allocation: false,
                ..
            }) => {
                sqlx::query("UPDATE tasks SET scheduled_at = NULL WHERE id = ?")
                    .bind(id)
                    .execute(&self.db_pool)
                    .await?;
                self.load_tasks().await?;
                self.refresh_calendar_data().await;
                self.status_message =
                    Some(("Task unscheduled".to_string(), std::time::Instant::now()));
            }
            Some(CellTask {
                is_allocation: true,
                ..
            }) => {
                self.status_message = Some((
                    "This is a deadline allocation, not a manual schedule".to_string(),
                    std::time::Instant::now(),
                ));
            }
            None => {
                self.status_message = Some((
                    "No scheduled task here".to_string(),
                    std::time::Instant::now(),
                ));
            }
        }
        Ok(())
    }

    /// Begin editing the deadline of the task at the selected cell (works for
    /// both manually-scheduled tasks and deadline allocations).
    pub fn start_deadline_edit_at_selected_cell(&mut self) {
        match self.selected_cell_task() {
            Some(CellTask { id, .. }) => {
                self.deadline_edit_task_id = Some(id);
                self.input_buffer.clear();
                self.calendar_input_mode = CalendarInputMode::DeadlineInput;
            }
            None => {
                self.status_message = Some((
                    "No task here to set a deadline for".to_string(),
                    std::time::Instant::now(),
                ));
            }
        }
    }

    /// Start parsing the pending deadline-edit input (e.g. "friday", "tomorrow") in the background
    /// and close the popup at once: the parser may wait on Ollama for up to 15s per call, which
    /// would otherwise freeze the whole TUI. Reuses the existing "by <word>" deadline grammar. The
    /// result arrives on `deadline_rx` and is applied by `apply_deadline_parse`.
    pub fn submit_deadline_edit(&mut self) {
        let Some(task_id) = self.deadline_edit_task_id.take() else {
            self.calendar_input_mode = CalendarInputMode::Navigate;
            return;
        };

        let text = self.input_buffer.trim().to_string();
        self.input_buffer.clear();
        self.calendar_input_mode = CalendarInputMode::Navigate;

        if text.is_empty() {
            return;
        }

        self.status_message = Some(("Parsing deadline...".to_string(), std::time::Instant::now()));

        let parser = Arc::clone(&self.nlp_parser);
        let tx = self.deadline_tx.clone();
        tokio::spawn(async move {
            let extract_deadline = |item: ParsedItem| match item {
                ParsedItem::Task(t) => t.deadline,
                ParsedItem::Event(_) => None,
            };

            let mut deadline = parser
                .parse(&format!("by {text}"))
                .await
                .ok()
                .and_then(|r| extract_deadline(r.item));
            if deadline.is_none() {
                deadline = parser
                    .parse(&text)
                    .await
                    .ok()
                    .and_then(|r| extract_deadline(r.item));
            }
            let _ = tx.send(DeadlineParse {
                task_id,
                text,
                deadline,
            });
        });
    }

    /// Store a finished background parse, then re-run allocation so the calendar reflects the new
    /// deadline immediately.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn apply_deadline_parse(&mut self, parsed: DeadlineParse) -> Result<(), sqlx::Error> {
        let DeadlineParse {
            task_id,
            text,
            deadline,
        } = parsed;
        let Some(deadline) = deadline else {
            self.status_message = Some((
                format!("Couldn't parse deadline '{text}' - try 'tomorrow' or a weekday"),
                std::time::Instant::now(),
            ));
            return Ok(());
        };

        sqlx::query("UPDATE tasks SET deadline = ? WHERE id = ?")
            .bind(deadline)
            .bind(task_id)
            .execute(&self.db_pool)
            .await?;

        self.load_tasks().await?;
        self.on_task_changed().await?;
        self.status_message = Some(("Deadline updated".to_string(), std::time::Instant::now()));
        Ok(())
    }
}
