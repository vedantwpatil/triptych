use sqlx::{
    FromRow,
    migrate::MigrateDatabase,
    sqlite::{Sqlite, SqlitePool},
};

const DB_URL: &str = "sqlite:todo.db";

// Make structs and enums public
#[derive(Clone, FromRow)]
pub struct Task {
    pub id: i64,
    pub description: String,
    pub completed: bool,
    pub item_order: Option<i64>,
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
            "SELECT id, description, completed, item_order FROM tasks ORDER BY item_order ASC",
        )
        .fetch_all(&self.db_pool)
        .await?;

        if self.selected >= self.tasks.len() {
            self.selected = self.tasks.len().saturating_sub(1);
        }
        Ok(())
    }

    // Add a new task to the database according to position (selecting a task using the cursor)
    pub async fn add_task(&mut self, description: &str) -> Result<(), sqlx::Error> {
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

            // Shift all tasks that come after the current one.
            sqlx::query("UPDATE tasks SET item_order = item_order + 1 WHERE item_order > ?")
                .bind(current_order)
                .execute(&self.db_pool)
                .await?;

            new_order = current_order + 1;
        }

        // Insert the new task with its calculated order.
        sqlx::query("INSERT INTO tasks (description, completed, item_order) VALUES (?, ?, ?)")
            .bind(description)
            .bind(false)
            .bind(new_order)
            .execute(&self.db_pool)
            .await?;

        // Reload tasks from the database.
        self.load_tasks().await?;

        // Find the new position of the inserted task and update the cursor.
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
            // Do nothing
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
            "SELECT id, description, completed, item_order FROM tasks WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.db_pool)
        .await?;

        Ok(task)
    }
}
