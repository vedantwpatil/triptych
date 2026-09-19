//! Placing tasks on the calendar: picker, cell scheduling and next-free-slot search.

use chrono::{DateTime, Duration, NaiveDate, NaiveTime, Timelike, Utc};

use super::{
    App,
    model::{CalendarInputMode, ScheduleBlock, TASK_COLUMNS, Task},
    tasks::classify_task,
    time::{day_end, day_of_week_i32, day_start, parse_time_string, resolve_local_datetime},
};

impl App {
    // Task scheduling methods
    #[must_use]
    pub fn unscheduled_tasks(&self) -> Vec<&Task> {
        self.tasks
            .iter()
            .filter(|t| !t.completed && t.scheduled_at.is_none())
            .collect()
    }

    /// Schedules the task chosen in the picker at the selected calendar cell.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn schedule_task_to_selected_cell(&mut self) -> Result<(), sqlx::Error> {
        let unscheduled: Vec<i64> = self.unscheduled_tasks().iter().map(|t| t.id).collect();
        if self.task_picker_selected >= unscheduled.len() {
            return Ok(());
        }

        let task_id = unscheduled[self.task_picker_selected];
        let date = self.selected_cell_date();
        let time = self.selected_cell_time();
        let datetime = resolve_local_datetime(date.and_time(time));

        sqlx::query("UPDATE tasks SET scheduled_at = ? WHERE id = ?")
            .bind(datetime)
            .bind(task_id)
            .execute(&self.db_pool)
            .await?;

        self.load_tasks().await?;
        self.refresh_calendar_data().await;
        self.calendar_input_mode = CalendarInputMode::Navigate;
        self.task_picker_selected = 0;
        Ok(())
    }

    /// Adds a task from `description`, scheduled at the selected calendar cell.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn add_task_at_selected_cell(
        &mut self,
        description: &str,
    ) -> Result<(), sqlx::Error> {
        let scheduled_at = resolve_local_datetime(
            self.selected_cell_date()
                .and_time(self.selected_cell_time()),
        );
        let category = classify_task(description).to_string();
        let new_order = i64::try_from(self.tasks.len()).unwrap_or(i64::MAX);

        sqlx::query(
            "INSERT INTO tasks (description, completed, item_order, priority, scheduled_at, task_category) VALUES (?, ?, ?, ?, ?, ?)"
        )
        .bind(description)
        .bind(false)
        .bind(new_order)
        .bind(1i32)
        .bind(scheduled_at)
        .bind(&category)
        .execute(&self.db_pool)
        .await?;

        self.load_tasks().await?;
        self.refresh_calendar_data().await;
        Ok(())
    }

    /// Schedules the selected todo at the next free slot.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn auto_schedule_task(&mut self) -> Result<(), sqlx::Error> {
        if self.tasks.is_empty() {
            return Ok(());
        }

        let task = &self.tasks[self.selected];

        // Skip completed or already scheduled tasks
        if task.completed || task.scheduled_at.is_some() {
            self.status_message = Some((
                "Task is already scheduled or completed".to_string(),
                std::time::Instant::now(),
            ));
            return Ok(());
        }

        let task_category = task
            .task_category
            .clone()
            .unwrap_or_else(|| "general".to_string());
        let task_id = task.id;

        if let Some(slot) = self.find_next_available_slot(&task_category).await? {
            sqlx::query("UPDATE tasks SET scheduled_at = ? WHERE id = ?")
                .bind(slot)
                .bind(task_id)
                .execute(&self.db_pool)
                .await?;

            let local_time = slot.with_timezone(&chrono::Local);
            let msg = format!(
                "Scheduled for {}",
                local_time
                    .format("%a %m/%d %I:%M%p")
                    .to_string()
                    .to_lowercase()
            );
            self.status_message = Some((msg, std::time::Instant::now()));
        } else {
            self.status_message = Some((
                "No available slot found".to_string(),
                std::time::Instant::now(),
            ));
        }

        self.load_tasks().await?;
        Ok(())
    }

    async fn find_next_available_slot(
        &self,
        task_category: &str,
    ) -> Result<Option<DateTime<Utc>>, sqlx::Error> {
        let now = chrono::Local::now();
        let today = now.naive_local().date();

        // Look at current week + next week (14 days)
        let days: Vec<NaiveDate> = (0..14).map(|i| today + Duration::days(i)).collect();

        // Get all schedule blocks
        let blocks = sqlx::query_as::<_, ScheduleBlock>(
            "SELECT id, day_of_week, start_time, end_time, block_type, title, description, priority FROM schedule_blocks"
        )
        .fetch_all(&self.db_pool)
        .await?;

        // Get all scheduled tasks in this range
        let range_start = resolve_local_datetime(day_start(days[0]));
        let range_end = resolve_local_datetime(day_end(days[days.len() - 1]));

        let query = format!(
            "SELECT {TASK_COLUMNS} FROM tasks WHERE scheduled_at >= ? AND scheduled_at < ? AND completed = 0"
        );
        let scheduled_tasks = sqlx::query_as::<_, Task>(&query)
            .bind(range_start)
            .bind(range_end)
            .fetch_all(&self.db_pool)
            .await?;

        let occupied_slots: Vec<(NaiveDate, u32)> = scheduled_tasks
            .iter()
            .filter_map(|t| {
                t.scheduled_at.map(|dt| {
                    let local = dt.with_timezone(&chrono::Local);
                    (local.date_naive(), local.time().hour())
                })
            })
            .collect();

        // Strategy 1: Find a matching block type with a free hour
        for day in &days {
            let dow = day_of_week_i32(*day);
            for block in &blocks {
                if block.day_of_week != dow {
                    continue;
                }
                if block.block_type != task_category {
                    continue;
                }
                let Some(start) = parse_time_string(&block.start_time) else {
                    continue;
                };
                let Some(end) = parse_time_string(&block.end_time) else {
                    continue;
                };

                let mut hour = start.hour();
                while hour < end.hour() {
                    // Skip past hours for today
                    if *day == today && hour <= now.hour() {
                        hour += 1;
                        continue;
                    }
                    // Check if slot is free
                    if !occupied_slots.contains(&(*day, hour)) {
                        // `hour` is always in-range for a time-of-day, so this is never `None`.
                        let time = NaiveTime::from_hms_opt(hour, 0, 0).unwrap_or(NaiveTime::MIN);
                        return Ok(Some(resolve_local_datetime(day.and_time(time))));
                    }
                    hour += 1;
                }
            }
        }

        // Strategy 2: Find any free hour (7am-11pm) not inside a different-type block
        for day in &days {
            let dow = day_of_week_i32(*day);
            for hour in 7u32..23 {
                // Skip past hours for today
                if *day == today && hour <= now.hour() {
                    continue;
                }

                // Check if this hour is inside a different-type block. `hour` is
                // always in-range for a time-of-day, so this is never `None`.
                let time = NaiveTime::from_hms_opt(hour, 0, 0).unwrap_or(NaiveTime::MIN);
                let in_different_block = blocks.iter().any(|block| {
                    if block.day_of_week != dow {
                        return false;
                    }
                    if block.block_type == task_category {
                        return false; // same type is fine
                    }
                    if let (Some(start), Some(end)) = (
                        parse_time_string(&block.start_time),
                        parse_time_string(&block.end_time),
                    ) {
                        start <= time && end > time
                    } else {
                        false
                    }
                });

                if in_different_block {
                    continue;
                }

                // Check if slot is free
                if !occupied_slots.contains(&(*day, hour)) {
                    return Ok(Some(resolve_local_datetime(day.and_time(time))));
                }
            }
        }

        Ok(None)
    }
}
