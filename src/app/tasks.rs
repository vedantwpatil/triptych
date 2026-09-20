//! Todo CRUD, task classification and extraction of fields from parsed NLP items.

use chrono::{DateTime, Utc};
use std::sync::Arc;

use super::{
    App,
    model::{EnhancedTaskInfo, TASK_COLUMNS, Task},
};
use crate::nlp::{ParsedItem, Priority};

/// Outcome of a background task parse (see `App::submit_task`).
#[derive(Debug)]
pub struct TaskParse {
    description: String,
    /// Set when the task comes from converting this email (`App::convert_selected_email_to_task`).
    email_id: Option<i64>,
    item: Result<ParsedItem, String>,
}

#[must_use]
pub fn classify_task(description: &str) -> &'static str {
    let lower = description.to_lowercase();

    if lower.contains("leetcode")
        || lower.contains("project")
        || lower.contains("code")
        || lower.contains("implement")
        || lower.contains("study")
        || lower.contains("homework")
    {
        return "deepwork";
    }

    if lower.contains("schedule") || lower.contains("call") || lower.contains("quick") {
        return "admin";
    }

    if lower.contains("read")
        || lower.contains("watch")
        || lower.contains("learn")
        || lower.contains("review")
    {
        return "learning";
    }

    "general"
}

#[must_use]
pub fn default_duration_for_category(category: &str) -> i32 {
    match category {
        "deepwork" => 90,
        "admin" => 30,
        _ => 60,
    }
}

/// (title, `scheduled_at`, priority, tags, deadline, `duration_minutes`)
pub type ExtractedTaskFields = (
    String,
    Option<DateTime<Utc>>,
    i32,
    Vec<String>,
    Option<DateTime<Utc>>,
    Option<i32>,
);

/// Extract the fields needed to insert a task from a parsed NLP result.
///
/// Shared between `App::add_task` (TUI/CLI path) and the daemon's fast-add path so the two never
/// drift on priority mapping or Task/Event handling.
#[must_use]
pub fn extract_task_fields(item: ParsedItem) -> ExtractedTaskFields {
    match item {
        ParsedItem::Task(nlp_task) => {
            let priority = match nlp_task.priority {
                Priority::Urgent => 3,
                Priority::High => 2,
                Priority::Medium => 1,
                Priority::Low => 0,
            };

            (
                nlp_task.title,
                nlp_task.due_date,
                priority,
                nlp_task.tags,
                nlp_task.deadline,
                nlp_task.duration_minutes,
            )
        }
        ParsedItem::Event(event) => {
            let duration = event
                .end_time
                .map(|end| {
                    i32::try_from((end - event.start_time).num_minutes()).unwrap_or(i32::MAX)
                })
                .filter(|&mins| mins > 0);
            (
                event.title,
                Some(event.start_time),
                1,
                event.tags,
                None,
                duration,
            )
        }
    }
}

impl App {
    /// Reloads the todo list from the database in display order.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn load_tasks(&mut self) -> Result<(), sqlx::Error> {
        let query = format!("SELECT {TASK_COLUMNS} FROM tasks ORDER BY item_order ASC");
        self.tasks = sqlx::query_as::<_, Task>(&query)
            .fetch_all(&self.db_pool)
            .await?;

        if self.selected >= self.tasks.len() {
            self.selected = self.tasks.len().saturating_sub(1);
        }
        Ok(())
    }

    /// Parses `description` and inserts the resulting task or event; returns its id.
    ///
    /// # Errors
    ///
    /// Returns an error if `description` is blank or a database write fails.
    pub async fn add_task(&mut self, description: &str) -> Result<i64, sqlx::Error> {
        if description.trim().is_empty() {
            return Err(sqlx::Error::Protocol(
                "task description is empty".to_string(),
            ));
        }
        let parse_result = self
            .nlp_parser
            .parse(description)
            .await
            .map_err(|e| sqlx::Error::Protocol(format!("NLP parsing failed: {e}")))?;
        self.insert_parsed_task(description, parse_result.item)
            .await
    }

    /// Parses `description` in the background, so the TUI never waits on the parser: a parse that
    /// reaches the LLM takes about a second, up to 15s if Ollama hangs. The result arrives on
    /// `task_rx` and `apply_task_parse` inserts it.
    pub fn submit_task(&mut self, description: String, email_id: Option<i64>) {
        self.status_message = Some(("Adding task...".to_string(), std::time::Instant::now()));
        let parser = Arc::clone(&self.nlp_parser);
        let tx = self.task_tx.clone();
        tokio::spawn(async move {
            let item = parser
                .parse(&description)
                .await
                .map(|r| r.item)
                .map_err(|e| format!("NLP parsing failed: {e}"));
            let _ = tx.send(TaskParse {
                description,
                email_id,
                item,
            });
        });
    }

    /// Inserts a finished background parse. For an email conversion, an email that was linked in the
    /// meantime (a second Enter before the first parse landed) is skipped, not duplicated.
    ///
    /// # Errors
    ///
    /// Returns an error if the parse failed or a database write fails.
    pub async fn apply_task_parse(&mut self, parsed: TaskParse) -> Result<(), sqlx::Error> {
        let TaskParse {
            description,
            email_id,
            item,
        } = parsed;
        let item = item.map_err(sqlx::Error::Protocol)?;

        if let Some(email_id) = email_id
            && self
                .emails
                .iter()
                .any(|e| e.id == email_id && e.task_id.is_some())
        {
            self.status_message = Some((
                "Email already converted to a task".to_string(),
                std::time::Instant::now(),
            ));
            return Ok(());
        }

        let task_id = self.insert_parsed_task(&description, item).await?;
        self.status_message = None;
        match email_id {
            Some(email_id) => self.finish_email_conversion(email_id, task_id).await,
            None => Ok(()),
        }
    }

    async fn insert_parsed_task(
        &mut self,
        description: &str,
        item: ParsedItem,
    ) -> Result<i64, sqlx::Error> {
        let (task_title, scheduled_at, priority_value, tags_list, deadline, duration_minutes) =
            extract_task_fields(item);

        let new_order: i64 = if self.tasks.is_empty() {
            0
        } else if self.selected == 0 {
            sqlx::query("UPDATE tasks SET item_order = item_order + 1 WHERE item_order >= 0")
                .execute(&self.db_pool)
                .await?;
            0
        } else {
            let current_order = self.tasks[self.selected]
                .item_order
                .unwrap_or_else(|| i64::try_from(self.tasks.len()).unwrap_or(i64::MAX));

            sqlx::query("UPDATE tasks SET item_order = item_order + 1 WHERE item_order > ?")
                .bind(current_order)
                .execute(&self.db_pool)
                .await?;

            current_order + 1
        };

        let tags_json = if tags_list.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&tags_list).unwrap_or_default())
        };

        let category = classify_task(&task_title).to_string();
        let duration_minutes =
            duration_minutes.unwrap_or_else(|| default_duration_for_category(&category));

        let new_task_id = sqlx::query(
            "INSERT INTO tasks (description, completed, item_order, priority, natural_language_input, tags, scheduled_at, deadline, duration_minutes, task_category) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&task_title)
        .bind(false)
        .bind(new_order)
        .bind(priority_value)
        .bind(description)
        .bind(tags_json)
        .bind(scheduled_at)
        .bind(deadline)
        .bind(duration_minutes)
        .bind(&category)
        .execute(&self.db_pool)
        .await?
        .last_insert_rowid();

        self.load_tasks().await?;

        self.selected = self
            .tasks
            .iter()
            .position(|t| t.item_order == Some(new_order))
            .unwrap_or(0);

        if deadline.is_some() {
            self.on_task_changed().await?;
        }

        Ok(new_task_id)
    }

    /// Deletes the selected task.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn delete_task(&mut self) -> Result<(), sqlx::Error> {
        if self.tasks.is_empty() {
            return Ok(());
        }

        let task_id = self.tasks[self.selected].id;

        sqlx::query("DELETE FROM tasks WHERE id = ?")
            .bind(task_id)
            .execute(&self.db_pool)
            .await?;
        self.load_tasks().await?;
        self.on_task_changed().await?;
        Ok(())
    }

    /// Todo rows covered by the visual selection, or `None` when not selecting.
    #[must_use]
    pub fn visual_range(&self) -> Option<std::ops::RangeInclusive<usize>> {
        let anchor = self.visual_anchor?;
        Some(anchor.min(self.selected)..=anchor.max(self.selected))
    }

    /// Starts visual selection at the cursor row, or cancels it if already active.
    pub const fn toggle_visual(&mut self) {
        self.visual_anchor = match self.visual_anchor {
            None if !self.tasks.is_empty() => Some(self.selected),
            _ => None,
        };
    }

    /// Deletes every task in the visual selection in one transaction, or just the cursor row
    /// when nothing is selected.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn delete_selected_tasks(&mut self) -> Result<(), sqlx::Error> {
        let Some(range) = self.visual_range() else {
            return self.delete_task().await;
        };
        self.visual_anchor = None;
        let ids: Vec<i64> = self
            .tasks
            .get(range.clone())
            .into_iter()
            .flatten()
            .map(|t| t.id)
            .collect();

        let mut tx = self.db_pool.begin().await?;
        for id in &ids {
            sqlx::query("DELETE FROM tasks WHERE id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;

        self.selected = *range.start();
        self.load_tasks().await?;
        self.status_message = Some((
            format!("Deleted {} task(s)", ids.len()),
            std::time::Instant::now(),
        ));
        self.on_task_changed().await
    }

    /// Toggles the selected task between done and not done.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn toggle_completed(&mut self) -> Result<(), sqlx::Error> {
        if self.tasks.is_empty() {
            return Ok(());
        }

        let task = &self.tasks[self.selected];
        let new_status = !task.completed;

        sqlx::query("UPDATE tasks SET completed = ? WHERE id = ?")
            .bind(new_status)
            .bind(task.id)
            .execute(&self.db_pool)
            .await?;

        self.load_tasks().await?;
        self.on_task_changed().await?;
        Ok(())
    }

    /// Reloads the todo list and returns each task with its decoded tags.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn get_enhanced_task_list(&mut self) -> Result<Vec<EnhancedTaskInfo>, sqlx::Error> {
        self.load_tasks().await?;

        let mut enhanced_tasks = Vec::new();

        for task in &self.tasks {
            let tags: Vec<String> = task.tags.as_ref().map_or_else(Vec::new, |tags_json| {
                serde_json::from_str(tags_json).unwrap_or_default()
            });

            enhanced_tasks.push(EnhancedTaskInfo {
                task: task.clone(),
                tags,
            });
        }

        Ok(enhanced_tasks)
    }

    /// Marks task `id` done; returns whether it existed.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn complete_task_by_id(&mut self, id: i64) -> Result<bool, sqlx::Error> {
        let rows_affected = sqlx::query("UPDATE tasks SET completed = true WHERE id = ?")
            .bind(id)
            .execute(&self.db_pool)
            .await?
            .rows_affected();

        if rows_affected > 0 {
            self.on_task_changed().await?;
        }
        Ok(rows_affected > 0)
    }

    /// Deletes task `id`; returns whether it existed.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn remove_task_by_id(&mut self, id: i64) -> Result<bool, sqlx::Error> {
        let rows_affected = sqlx::query("DELETE FROM tasks WHERE id = ?")
            .bind(id)
            .execute(&self.db_pool)
            .await?
            .rows_affected();

        if rows_affected > 0 {
            self.on_task_changed().await?;
        }
        Ok(rows_affected > 0)
    }

    /// Deletes every completed task and returns how many were removed.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn clear_completed_tasks(&mut self) -> Result<u64, sqlx::Error> {
        let rows_affected = sqlx::query("DELETE FROM tasks WHERE completed = true")
            .execute(&self.db_pool)
            .await?
            .rows_affected();

        if rows_affected > 0 {
            self.on_task_changed().await?;
        }
        Ok(rows_affected)
    }

    /// Returns task `id`, or `None` if it does not exist.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn get_task_by_id(&self, id: i64) -> Result<Option<Task>, sqlx::Error> {
        let query = format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?");
        let task = sqlx::query_as::<_, Task>(&query)
            .bind(id)
            .fetch_optional(&self.db_pool)
            .await?;

        Ok(task)
    }
}
