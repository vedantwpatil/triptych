//! Schedule blocks: create/delete, time and day parsing, TOML import/export, summary.

use chrono::Timelike;
use std::path::Path;

use super::{
    App,
    model::{BlockDefinition, CalendarInputMode, ScheduleBlock, ScheduleToml},
    time::day_of_week_i32,
};

impl App {
    // Schedule block creation
    /// Creates a block from the block form and closes the form.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn create_schedule_block(&mut self) -> Result<(), sqlx::Error> {
        let day_of_week = day_of_week_i32(self.selected_cell_date());

        // Validate times
        if Self::validate_time_format(&self.block_form.start_time).is_err() {
            self.status_message = Some((
                "Invalid start time format".to_string(),
                std::time::Instant::now(),
            ));
            return Ok(());
        }
        if Self::validate_time_format(&self.block_form.end_time).is_err() {
            self.status_message = Some((
                "Invalid end time format".to_string(),
                std::time::Instant::now(),
            ));
            return Ok(());
        }
        if Self::time_to_minutes(&self.block_form.end_time)
            <= Self::time_to_minutes(&self.block_form.start_time)
        {
            self.status_message = Some((
                "End time must be after start time".to_string(),
                std::time::Instant::now(),
            ));
            return Ok(());
        }

        // Check for conflicts
        if self
            .has_block_conflict(
                day_of_week,
                &self.block_form.start_time,
                &self.block_form.end_time,
            )
            .await?
        {
            self.status_message = Some((
                "Block overlaps with existing block".to_string(),
                std::time::Instant::now(),
            ));
            return Ok(());
        }

        sqlx::query(
            "INSERT INTO schedule_blocks (day_of_week, start_time, end_time, block_type, title) VALUES (?, ?, ?, ?, ?)"
        )
        .bind(day_of_week)
        .bind(&self.block_form.start_time)
        .bind(&self.block_form.end_time)
        .bind(&self.block_form.block_type)
        .bind(&self.block_form.title)
        .execute(&self.db_pool)
        .await?;

        self.refresh_calendar_data().await;
        self.calendar_input_mode = CalendarInputMode::Navigate;
        self.status_message = Some(("Block created".to_string(), std::time::Instant::now()));
        Ok(())
    }

    /// Validate time string format "HH:MM"
    ///
    /// # Errors
    ///
    /// Returns an error if `time` is not a valid `HH:MM` 24-hour time.
    pub fn validate_time_format(time: &str) -> Result<(), crate::BoxError> {
        let parts: Vec<&str> = time.split(':').collect();
        if parts.len() != 2 {
            return Err(format!("Invalid time format: {time}").into());
        }

        let hour: u32 = parts[0]
            .parse()
            .map_err(|_| format!("Invalid hour in: {time}"))?;
        let minute: u32 = parts[1]
            .parse()
            .map_err(|_| format!("Invalid minute in: {time}"))?;

        if hour > 23 {
            return Err(format!("Hour out of range: {time}").into());
        }
        if minute > 59 {
            return Err(format!("Minute out of range: {time}").into());
        }

        Ok(())
    }

    /// Validate both times and that the range ends after it starts
    fn validate_time_range(start: &str, end: &str) -> Result<(), crate::BoxError> {
        Self::validate_time_format(start)?;
        Self::validate_time_format(end)?;
        if Self::time_to_minutes(end) <= Self::time_to_minutes(start) {
            return Err(format!("End time {end} must be after start time {start}").into());
        }
        Ok(())
    }

    /// Parse "HH:MM" to minutes since midnight
    #[must_use]
    pub fn time_to_minutes(time: &str) -> Option<u32> {
        let parts: Vec<&str> = time.split(':').collect();
        if parts.len() >= 2 {
            let hour: u32 = parts[0].parse().ok()?;
            let minute: u32 = parts[1].parse().ok()?;
            Some(hour * 60 + minute)
        } else {
            None
        }
    }

    /// Parse day name(s) to day numbers. Supports:
    /// - Single days: "monday", "tuesday", etc.
    /// - Compound days: "`monday_wednesday`", "`tuesday_thursday`"
    /// - Special groups: "weekdays", "weekends", "everyday"
    ///
    /// Uses Monday-first numbering to match chrono's `num_days_from_monday()`:
    /// Monday = 0, Tuesday = 1, ..., Sunday = 6
    ///
    /// # Errors
    ///
    /// Returns an error if a day name is not recognised.
    pub fn parse_days(name: &str) -> Result<Vec<i32>, crate::BoxError> {
        let name_lower = name.to_lowercase();

        // Check for special group names first (Monday-first: Mon=0, Sun=6)
        match name_lower.as_str() {
            "weekdays" => return Ok(vec![0, 1, 2, 3, 4]), // Mon-Fri
            "weekends" => return Ok(vec![5, 6]),          // Sat-Sun
            "everyday" | "daily" => return Ok(vec![0, 1, 2, 3, 4, 5, 6]),
            _ => {}
        }

        // Parse potentially compound day names (e.g., "monday_wednesday")
        let day_parts: Vec<&str> = name_lower.split('_').collect();
        let mut days = Vec::new();

        for part in day_parts {
            // Monday-first numbering to match chrono's num_days_from_monday()
            let day_num = match part {
                "monday" | "mon" => 0,
                "tuesday" | "tue" | "tues" => 1,
                "wednesday" | "wed" => 2,
                "thursday" | "thu" | "thurs" => 3,
                "friday" | "fri" => 4,
                "saturday" | "sat" => 5,
                "sunday" | "sun" => 6,
                _ => return Err(format!("Invalid day name: {part} (in '{name}')").into()),
            };
            if !days.contains(&day_num) {
                days.push(day_num);
            }
        }

        if days.is_empty() {
            return Err(format!("Invalid day name: {name}").into());
        }

        days.sort_unstable();
        Ok(days)
    }

    /// Convert day number to name (Monday-first: Mon=0, Sun=6)
    #[must_use]
    pub fn day_number_to_name(num: i32) -> String {
        match num {
            0 => "monday",
            1 => "tuesday",
            2 => "wednesday",
            3 => "thursday",
            4 => "friday",
            5 => "saturday",
            6 => "sunday",
            _ => "unknown",
        }
        .to_string()
    }

    /// Check if a new block would overlap with existing blocks
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn has_block_conflict(
        &self,
        day_of_week: i32,
        start_time: &str,
        end_time: &str,
    ) -> Result<bool, sqlx::Error> {
        let new_start = Self::time_to_minutes(start_time).unwrap_or(0);
        let new_end = Self::time_to_minutes(end_time).unwrap_or(0);

        let existing = sqlx::query_as::<_, ScheduleBlock>(
            "SELECT id, day_of_week, start_time, end_time, block_type, title, description, priority
             FROM schedule_blocks WHERE day_of_week = ?",
        )
        .bind(day_of_week)
        .fetch_all(&self.db_pool)
        .await?;

        for block in existing {
            let block_start = Self::time_to_minutes(&block.start_time).unwrap_or(0);
            let block_end = Self::time_to_minutes(&block.end_time).unwrap_or(0);

            // Check overlap: NOT (new_end <= block_start OR new_start >= block_end)
            if !(new_end <= block_start || new_start >= block_end) {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Imports the blocks in the TOML file at `path`, skipping any that overlap; returns how many were added.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or parsed, a block is invalid, or a database write fails.
    pub async fn import_schedule_from_toml(
        &mut self,
        path: &Path,
    ) -> Result<usize, crate::BoxError> {
        let content = std::fs::read_to_string(path)?;
        let schedule: ScheduleToml = toml::from_str(&content)?;

        let mut imported = 0;

        for block in schedule.blocks {
            // Parse day name(s) - supports compound days like "monday_wednesday"
            let days = Self::parse_days(&block.day)?;

            Self::validate_time_range(&block.start, &block.end)?;

            // Create a block for each day
            for day_of_week in days {
                // Check for conflicts
                if self
                    .has_block_conflict(day_of_week, &block.start, &block.end)
                    .await?
                {
                    let day_name = Self::day_number_to_name(day_of_week);
                    eprintln!(
                        "Warning: Skipping overlapping block '{}' on {}",
                        block.title, day_name
                    );
                    continue;
                }

                sqlx::query(
                    "INSERT INTO schedule_blocks (day_of_week, start_time, end_time, block_type, title, description, priority)
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(day_of_week)
                .bind(&block.start)
                .bind(&block.end)
                .bind(&block.block_type)
                .bind(&block.title)
                .bind(&block.description)
                .bind(block.priority)
                .execute(&self.db_pool)
                .await?;

                imported += 1;
            }
        }

        self.refresh_calendar_data().await;
        Ok(imported)
    }

    /// Writes every schedule block to `path` as TOML and returns how many were written.
    ///
    /// # Errors
    ///
    /// Returns an error if the query fails, or the schedule cannot be serialised or written to `path`.
    pub async fn export_schedule_to_toml(&self, path: &Path) -> Result<usize, crate::BoxError> {
        let blocks = sqlx::query_as::<_, ScheduleBlock>(
            "SELECT id, day_of_week, start_time, end_time, block_type, title, description, priority
             FROM schedule_blocks ORDER BY day_of_week, start_time",
        )
        .fetch_all(&self.db_pool)
        .await?;

        let block_defs: Vec<BlockDefinition> = blocks
            .iter()
            .map(|b| BlockDefinition {
                day: Self::day_number_to_name(b.day_of_week),
                block_type: b.block_type.clone(),
                start: b.start_time.clone(),
                end: b.end_time.clone(),
                title: b.title.clone(),
                description: b.description.clone(),
                priority: b.priority,
            })
            .collect();

        let schedule = ScheduleToml { blocks: block_defs };
        let toml_string = toml::to_string_pretty(&schedule)?;
        std::fs::write(path, toml_string)?;

        Ok(blocks.len())
    }

    /// Deletes every schedule block and returns how many were removed.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn clear_all_schedule_blocks(&mut self) -> Result<u64, sqlx::Error> {
        let result = sqlx::query("DELETE FROM schedule_blocks")
            .execute(&self.db_pool)
            .await?;
        self.refresh_calendar_data().await;
        Ok(result.rows_affected())
    }

    /// Prints the weekly schedule, grouped by day, to stdout.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn print_schedule_summary(&self) -> Result<(), sqlx::Error> {
        let blocks = sqlx::query_as::<_, ScheduleBlock>(
            "SELECT id, day_of_week, start_time, end_time, block_type, title, description, priority
             FROM schedule_blocks ORDER BY day_of_week, start_time",
        )
        .fetch_all(&self.db_pool)
        .await?;

        if blocks.is_empty() {
            println!("No schedule blocks defined.");
            println!("Import with: triptych schedule import <file.toml>");
            return Ok(());
        }

        // Monday-first ordering to match chrono's num_days_from_monday()
        let days = [
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday",
            "Sunday",
        ];
        let mut current_day = -1;

        for block in blocks {
            if block.day_of_week != current_day {
                current_day = block.day_of_week;
                // `day_of_week` is always 0..=6 by DB constraint.
                println!("\n{}:", days[usize::try_from(current_day).unwrap_or(0)]);
            }
            println!(
                "  {} - {} [{}] {}",
                block.start_time, block.end_time, block.block_type, block.title
            );
        }

        let allocations: Vec<(String, String, i32, String)> = sqlx::query_as(
            "SELECT a.block_date, a.block_start_time, a.allocated_minutes, t.description
             FROM task_block_allocations a JOIN tasks t ON a.task_id = t.id
             WHERE t.completed = 0 ORDER BY a.block_date, a.block_start_time, t.id",
        )
        .fetch_all(&self.db_pool)
        .await?;
        if !allocations.is_empty() {
            println!("\nAllocated tasks:");
            for (date, start, minutes, description) in allocations {
                println!("  {date} {start} ({minutes}m) {description}");
            }
        }

        Ok(())
    }

    /// Deletes the schedule block under the calendar cursor, if any.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn delete_block_at_selected_cell(&mut self) -> Result<(), sqlx::Error> {
        let date = self.selected_cell_date();
        let day_of_week = day_of_week_i32(date);
        let time = self.selected_cell_time();
        let time_str = format!("{:02}:{:02}", time.hour(), time.minute());

        // Find block that contains this time
        let blocks = sqlx::query_as::<_, ScheduleBlock>(
            "SELECT id, day_of_week, start_time, end_time, block_type, title, description, priority
             FROM schedule_blocks WHERE day_of_week = ?",
        )
        .bind(day_of_week)
        .fetch_all(&self.db_pool)
        .await?;

        let time_minutes = Self::time_to_minutes(&time_str).unwrap_or(0);

        for block in blocks {
            let start = Self::time_to_minutes(&block.start_time).unwrap_or(0);
            let end = Self::time_to_minutes(&block.end_time).unwrap_or(0);

            if time_minutes >= start && time_minutes < end {
                sqlx::query("DELETE FROM schedule_blocks WHERE id = ?")
                    .bind(block.id)
                    .execute(&self.db_pool)
                    .await?;

                self.refresh_calendar_data().await;
                self.status_message = Some((
                    format!("Deleted block: {}", block.title),
                    std::time::Instant::now(),
                ));
                return Ok(());
            }
        }

        self.status_message = Some((
            "No block at this time".to_string(),
            std::time::Instant::now(),
        ));
        Ok(())
    }
}
