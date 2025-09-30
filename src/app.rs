use crate::nlp::{ParsedTask, TaskParser};
use chrono::{DateTime, Utc};
use serde_json;
use sqlx::{
    FromRow,
    migrate::MigrateDatabase,
    sqlite::{Sqlite, SqlitePool},
}; // Import from chrono directly, not sqlx::types

const DB_URL: &str = "sqlite:todo.db";

#[derive(Clone, FromRow, Debug)]
pub struct TimelineEntry {
    pub id: i64,
    pub entity_type: String, // 'task', 'event', 'email'
    pub entity_id: i64,
    pub created_at: DateTime<Utc>,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub priority: i32,
    pub tags: Option<String>, // JSON array
}

#[derive(Clone, FromRow, Debug)]
pub struct Event {
    pub id: i64,
    pub title: String,
    pub description: Option<String>,
    pub start_time: DateTime<Utc>,
    pub end_time: DateTime<Utc>,
    pub location: Option<String>,
    pub calendar_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Clone, FromRow, Debug)]
pub struct Email {
    pub id: i64,
    pub message_id: String,
    pub subject: String,
    pub sender: String,
    pub recipients: String, // JSON
    pub body_text: Option<String>,
    pub body_html: Option<String>,
    pub received_at: DateTime<Utc>,
    pub folder_name: String, // Changed from 'folder' to match SQL
    pub is_read: bool,
    pub is_flagged: bool,
}

// Enhanced Task with timeline integration
#[derive(Clone, FromRow, Debug)]
pub struct Task {
    pub id: i64,
    pub description: String,
    pub completed: bool,
    pub item_order: Option<i64>,
    pub scheduled_at: Option<DateTime<Utc>>, // When it becomes an event
    pub priority: i32,
    pub tags: Option<String>,
    pub natural_language_input: Option<String>,
}

// Helper struct for enhanced task information
#[derive(Debug)]
pub struct EnhancedTaskInfo {
    pub task: Task,
    pub tags: Vec<String>,
    pub is_scheduled: bool,
}

pub enum InputMode {
    Normal,
    Editing,
}

pub struct App {
    pub db_pool: SqlitePool,
    pub tasks: Vec<Task>,
    pub selected: usize,
    pub input_mode: InputMode,
    pub input_buffer: String,
}

impl App {
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            db_pool: pool,
            tasks: Vec::new(),
            selected: 0,
            input_mode: InputMode::Normal,
            input_buffer: String::new(),
        }
    }

    pub async fn build() -> Result<Self, sqlx::Error> {
        // Create the database if it doesn't exist
        if !Sqlite::database_exists(DB_URL).await.unwrap_or(false) {
            Sqlite::create_database(DB_URL).await?;
        }

        // Create the connection pool
        let db_pool = SqlitePool::connect(DB_URL).await?;

        // Run migrations
        sqlx::migrate!("./migrations").run(&db_pool).await?;

        // Return a new App instance using the synchronous `new` method
        Ok(Self::new(db_pool))
    }

    // Load tasks from the database into app state
    pub async fn load_tasks(&mut self) -> Result<(), sqlx::Error> {
        self.tasks = sqlx::query_as::<_, Task>(
            "SELECT id, description, completed, item_order, scheduled_at, priority, tags, natural_language_input FROM tasks ORDER BY item_order ASC",
        )
        .fetch_all(&self.db_pool)
        .await?;

        if self.selected >= self.tasks.len() {
            self.selected = self.tasks.len().saturating_sub(1);
        }
        Ok(())
    }

    // Enhanced add_task method with NLP parsing
    pub async fn add_task(&mut self, description: &str) -> Result<(), sqlx::Error> {
        // Parse the natural language input
        let parser = TaskParser::new();
        let parsed = parser.parse(description);

        let new_order: i64;

        if self.tasks.is_empty() {
            new_order = 0;
        } else if self.selected == 0 {
            sqlx::query("UPDATE tasks SET item_order = item_order + 1 WHERE item_order >= 0")
                .execute(&self.db_pool)
                .await?;
            new_order = 0;
        } else {
            let current_order = self.tasks[self.selected]
                .item_order
                .unwrap_or(self.tasks.len() as i64);

            sqlx::query("UPDATE tasks SET item_order = item_order + 1 WHERE item_order > ?")
                .bind(current_order)
                .execute(&self.db_pool)
                .await?;

            new_order = current_order + 1;
        }

        // Serialize tags to JSON
        let tags_json = if parsed.tags.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&parsed.tags).unwrap_or_default())
        };

        // Insert the new task with NLP-parsed data
        sqlx::query(
            "INSERT INTO tasks (description, completed, item_order, priority, natural_language_input, tags, scheduled_at) VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&parsed.description)  // Use cleaned description
        .bind(false)
        .bind(new_order)
        .bind(parsed.priority)      // Use NLP-detected priority
        .bind(description)          // Store original input
        .bind(tags_json)            // Store tags as JSON
        .bind(parsed.scheduled_at)  // Store scheduled time if detected
        .execute(&self.db_pool)
        .await?;

        self.load_tasks().await?;

        self.selected = self
            .tasks
            .iter()
            .position(|t| t.item_order == Some(new_order))
            .unwrap_or(0);

        Ok(())
    }

    // Delete a selected task from the database
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
        Ok(())
    }

    // Toggle the completion status of the currently selected task
    pub async fn toggle_completed(&mut self) -> Result<(), sqlx::Error> {
        if self.tasks.is_empty() {
            return Ok(());
        }

        // Get the selected task and its new status
        let task = &self.tasks[self.selected];
        let new_status = !task.completed; // Flip the boolean status

        // Update the task in the database
        sqlx::query("UPDATE tasks SET completed = ? WHERE id = ?")
            .bind(new_status)
            .bind(task.id)
            .execute(&self.db_pool)
            .await?;

        // Reload tasks to reflect the change in the UI
        self.load_tasks().await?;
        Ok(())
    }
    //
    // Converts tasks to events when they have scheduled times
    pub async fn convert_task_to_event(&self, task_id: i64) -> Result<Option<i64>, sqlx::Error> {
        // Get the task
        if let Some(task) = self.get_task_by_id(task_id).await? {
            if let Some(scheduled_time) = task.scheduled_at {
                // Create an event from the task
                let event_result = sqlx::query(
                    "INSERT INTO events (title, description, start_time, end_time, created_at) VALUES (?, ?, ?, ?, ?)"
                )
                .bind(&task.description)
                .bind("Converted from task")
                .bind(scheduled_time)
                .bind(scheduled_time) // Default 1-hour duration, you can customize this
                .bind(chrono::Utc::now())
                .execute(&self.db_pool)
                .await?;

                return Ok(Some(event_result.last_insert_rowid()));
            }
        }
        Ok(None)
    }
    // Enhanced listing method that shows more details
    pub async fn get_enhanced_task_list(&mut self) -> Result<Vec<EnhancedTaskInfo>, sqlx::Error> {
        self.load_tasks().await?;

        let mut enhanced_tasks = Vec::new();

        for task in &self.tasks {
            let tags: Vec<String> = if let Some(tags_json) = &task.tags {
                serde_json::from_str(tags_json).unwrap_or_default()
            } else {
                Vec::new()
            };

            enhanced_tasks.push(EnhancedTaskInfo {
                task: task.clone(),
                tags,
                is_scheduled: task.scheduled_at.is_some(),
            });
        }

        Ok(enhanced_tasks)
    }

    /// Mark a task as completed by its database ID
    pub async fn complete_task_by_id(&mut self, id: i64) -> Result<bool, sqlx::Error> {
        let rows_affected = sqlx::query("UPDATE tasks SET completed = true WHERE id = ?")
            .bind(id)
            .execute(&self.db_pool)
            .await?
            .rows_affected();

        if rows_affected == 0 {
            Ok(false) // Task with this ID was not found
        } else {
            Ok(true) // Task was successfully marked as done
        }
    }

    /// Remove a task by its database ID
    pub async fn remove_task_by_id(&mut self, id: i64) -> Result<bool, sqlx::Error> {
        let rows_affected = sqlx::query("DELETE FROM tasks WHERE id = ?")
            .bind(id)
            .execute(&self.db_pool)
            .await?
            .rows_affected();

        if rows_affected == 0 {
            Ok(false) // Task with this ID was not found
        } else {
            Ok(true) // Task was successfully removed
        }
    }

    /// Remove all completed tasks from the database
    pub async fn clear_completed_tasks(&mut self) -> Result<u64, sqlx::Error> {
        let rows_affected = sqlx::query("DELETE FROM tasks WHERE completed = true")
            .execute(&self.db_pool)
            .await?
            .rows_affected();

        Ok(rows_affected)
    }

    /// Get a specific task by ID (useful for CLI operations)
    pub async fn get_task_by_id(&self, id: i64) -> Result<Option<Task>, sqlx::Error> {
        let task = sqlx::query_as::<_, Task>(
            "SELECT id, description, completed, item_order, scheduled_at, priority, tags, natural_language_input FROM tasks WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.db_pool)
        .await?;

        Ok(task)
    }
}
