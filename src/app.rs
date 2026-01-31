#![allow(dead_code)]
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

use crate::nlp::{NLPParser, ParseStrategy, ParsedItem, Priority};
use sqlx::{
    FromRow,
    migrate::MigrateDatabase,
    sqlite::{Sqlite, SqlitePool},
};

// TOML import/export types
#[derive(Debug, Deserialize, Serialize)]
pub struct ScheduleToml {
    pub blocks: Vec<BlockDefinition>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct BlockDefinition {
    pub day: String,
    #[serde(rename = "type")]
    pub block_type: String,
    pub start: String,
    pub end: String,
    pub title: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default = "default_priority")]
    pub priority: i32,
}

fn default_priority() -> i32 {
    1
}

const DB_URL: &str = "sqlite:todo.db";

#[derive(Clone, FromRow, Debug)]
pub struct TimelineEntry {
    pub id: i64,
    pub entity_type: String,
    pub entity_id: i64,
    pub created_at: DateTime<Utc>,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub priority: i32,
    pub tags: Option<String>,
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

#[derive(Debug, Clone, PartialEq)]
pub enum ViewMode {
    TodoList,
    Calendar,
}

#[derive(Debug, Clone, FromRow)]
pub struct ScheduleBlock {
    pub id: i64,
    pub day_of_week: i32,
    pub start_time: String,
    pub end_time: String,
    pub block_type: String,
    pub title: String,
    pub description: Option<String>,
    pub priority: i32,
}

#[derive(Clone, FromRow, Debug)]
pub struct Task {
    pub id: i64,
    pub description: String,
    pub completed: bool,
    pub item_order: Option<i64>,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub priority: i32,
    pub tags: Option<String>,
    pub natural_language_input: Option<String>,
    pub task_category: Option<String>,
}

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

#[derive(Debug, Clone, PartialEq)]
pub enum CalendarInputMode {
    Navigate,
    BlockForm,
    TaskPicker,
    TaskInput,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BlockFormField {
    BlockType,
    StartTime,
    EndTime,
    Title,
}

#[derive(Debug, Clone)]
pub struct BlockFormState {
    pub block_type: String,
    pub start_time: String,
    pub end_time: String,
    pub title: String,
    pub active_field: BlockFormField,
}

impl BlockFormState {
    pub const BLOCK_TYPES: &'static [&'static str] = &[
        "deepwork",
        "deepwork_input",
        "deepwork_output",
        "class",
        "training",
        "bio-maintenance",
        "admin",
        "social",
        "learning",
        "meal",
        "break",
        "planning",
        "project",
    ];

    pub fn new_at(time_slot: usize) -> Self {
        let start_hour = 7 + time_slot;
        let end_hour = start_hour + 1;
        Self {
            block_type: "deepwork".to_string(),
            start_time: format!("{:02}:00", start_hour),
            end_time: format!("{:02}:00", end_hour),
            title: String::new(),
            active_field: BlockFormField::BlockType,
        }
    }

    pub fn cycle_block_type(&mut self, forward: bool) {
        let current_idx = Self::BLOCK_TYPES
            .iter()
            .position(|t| *t == self.block_type)
            .unwrap_or(0);
        let new_idx = if forward {
            (current_idx + 1) % Self::BLOCK_TYPES.len()
        } else if current_idx == 0 {
            Self::BLOCK_TYPES.len() - 1
        } else {
            current_idx - 1
        };
        self.block_type = Self::BLOCK_TYPES[new_idx].to_string();
    }

    pub fn next_field(&mut self) {
        self.active_field = match self.active_field {
            BlockFormField::BlockType => BlockFormField::StartTime,
            BlockFormField::StartTime => BlockFormField::EndTime,
            BlockFormField::EndTime => BlockFormField::Title,
            BlockFormField::Title => BlockFormField::BlockType,
        };
    }

    pub fn prev_field(&mut self) {
        self.active_field = match self.active_field {
            BlockFormField::BlockType => BlockFormField::Title,
            BlockFormField::StartTime => BlockFormField::BlockType,
            BlockFormField::EndTime => BlockFormField::StartTime,
            BlockFormField::Title => BlockFormField::EndTime,
        };
    }
}

pub struct App {
    pub db_pool: SqlitePool,
    pub tasks: Vec<Task>,
    pub selected: usize,
    pub input_mode: InputMode,
    pub view_mode: ViewMode,
    pub calendar_week_offset: Option<i64>,
    pub selected_day: usize,
    pub selected_time_slot: usize,
    pub calendar_input_mode: CalendarInputMode,
    pub block_form: BlockFormState,
    pub task_picker_selected: usize,
    pub input_buffer: String,
    nlp_parser: Arc<NLPParser>,
    pub cached_schedule_blocks: Vec<(NaiveDate, ScheduleBlock)>,
    pub cached_scheduled_tasks: Vec<(NaiveDate, NaiveTime, String, i32)>,
    pub status_message: Option<(String, std::time::Instant)>,
}

fn parse_time_string(time_str: &str) -> Option<NaiveTime> {
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

impl App {
    pub async fn new(pool: SqlitePool) -> Self {
        let nlp_parser = Arc::new(NLPParser::new().await);

        Self {
            db_pool: pool,
            tasks: Vec::new(),
            selected: 0,
            input_mode: InputMode::Normal,
            view_mode: ViewMode::TodoList,
            calendar_week_offset: None,
            selected_day: 0,
            selected_time_slot: 0,
            calendar_input_mode: CalendarInputMode::Navigate,
            block_form: BlockFormState::new_at(0),
            task_picker_selected: 0,
            input_buffer: String::new(),
            nlp_parser,
            cached_schedule_blocks: Vec::new(),
            cached_scheduled_tasks: Vec::new(),
            status_message: None,
        }
    }

    pub async fn refresh_calendar_data(&mut self) {
        let today = chrono::Local::now().naive_local().date();
        let week_offset = self.calendar_week_offset.unwrap_or(0);
        let start_of_week = today + Duration::weeks(week_offset)
            - Duration::days(today.weekday().num_days_from_monday() as i64);

        let days: Vec<NaiveDate> = (0..7).map(|i| start_of_week + Duration::days(i)).collect();

        self.cached_schedule_blocks = self
            .get_week_schedule_internal(&days)
            .await
            .unwrap_or_default();

        self.cached_scheduled_tasks = self
            .get_scheduled_tasks_internal(&days)
            .await
            .unwrap_or_default();
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
                if day.weekday().num_days_from_monday() == block.day_of_week as u32 {
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
    ) -> Result<Vec<(NaiveDate, NaiveTime, String, i32)>, sqlx::Error> {
        let start = days[0].and_hms_opt(0, 0, 0).unwrap().and_utc();
        let end = days[days.len() - 1]
            .and_hms_opt(23, 59, 59)
            .unwrap()
            .and_utc();

        let tasks = sqlx::query_as::<_, Task>(
            r#"
            SELECT id, description, completed, item_order, scheduled_at, priority, tags, natural_language_input, task_category
            FROM tasks
            WHERE scheduled_at >= ? AND scheduled_at < ?
            AND completed = 0
            ORDER BY scheduled_at
            "#
        )
        .bind(start)
        .bind(end)
        .fetch_all(&self.db_pool)
        .await?;

        Ok(tasks
            .iter()
            .filter_map(|t| {
                t.scheduled_at.map(|dt| {
                    (
                        dt.date_naive(),
                        dt.time(),
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
        self.refresh_calendar_data().await;
    }

    pub async fn prev_week(&mut self) {
        let offset = self.calendar_week_offset.unwrap_or(0);
        self.calendar_week_offset = Some(offset - 1);
        self.refresh_calendar_data().await;
    }

    pub async fn toggle_to_calendar(&mut self) {
        self.view_mode = ViewMode::Calendar;
        self.calendar_input_mode = CalendarInputMode::Navigate;
        let _ = self.load_tasks().await;
        self.refresh_calendar_data().await;
    }

    pub async fn toggle_to_todo(&mut self) {
        self.view_mode = ViewMode::TodoList;
        let _ = self.load_tasks().await;
    }

    pub fn classify_task(&self, description: &str) -> &str {
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

        if lower.contains("schedule")
            || lower.contains("call")
            || lower.contains("quick")
        {
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

    pub async fn get_week_schedule(
        &self,
        _week_offset: i32,
    ) -> Result<Vec<(i32, Vec<ScheduleBlock>)>, sqlx::Error> {
        let mut schedule_by_day = Vec::new();

        for day in 0..7 {
            let blocks = sqlx::query_as::<_, ScheduleBlock>(
                r#"
            SELECT id, day_of_week, start_time, end_time, 
                   block_type, title, description, priority
            FROM schedule_blocks
            WHERE day_of_week = ?
            ORDER BY start_time
            "#,
            )
            .bind(day)
            .fetch_all(&self.db_pool)
            .await?;

            schedule_by_day.push((day, blocks));
        }

        Ok(schedule_by_day)
    }

    pub async fn build() -> Result<Self, sqlx::Error> {
        if !Sqlite::database_exists(DB_URL).await.unwrap_or(false) {
            Sqlite::create_database(DB_URL).await?;
        }

        let db_pool = SqlitePool::connect(DB_URL).await?;
        sqlx::migrate!("./migrations").run(&db_pool).await?;

        let app = Self::new(db_pool).await;

        if app.nlp_parser.is_ollama_available() {
            println!("✓ NLP parsing ready");
        } else {
            println!("⚠️  Ollama unavailable - limited parsing");
        }

        Ok(app)
    }

    pub fn nlp_parser_ref(&self) -> Arc<NLPParser> {
        Arc::clone(&self.nlp_parser)
    }

    pub async fn load_tasks(&mut self) -> Result<(), sqlx::Error> {
        self.tasks = sqlx::query_as::<_, Task>(
            "SELECT id, description, completed, item_order, scheduled_at, priority, tags, natural_language_input, task_category FROM tasks ORDER BY item_order ASC",
        )
        .fetch_all(&self.db_pool)
        .await?;

        if self.selected >= self.tasks.len() {
            self.selected = self.tasks.len().saturating_sub(1);
        }
        Ok(())
    }

    pub async fn add_task(&mut self, description: &str) -> Result<(), sqlx::Error> {
        let parse_result = self
            .nlp_parser
            .parse(description)
            .await
            .map_err(|e| sqlx::Error::Protocol(format!("NLP parsing failed: {}", e)))?;

        match parse_result.strategy {
            ParseStrategy::Cached => {
                println!("⚡ Cache hit! Result in {}ms", parse_result.parse_time_ms);
            }
            ParseStrategy::Regex => {
                println!(
                    "📊 Parsed using Regex in {}ms (confidence: {:.0}%)",
                    parse_result.parse_time_ms,
                    parse_result.confidence * 100.0
                );
            }
            ParseStrategy::Ollama => {
                println!(
                    "📊 Parsed using Ollama in {}ms (confidence: {:.0}%)",
                    parse_result.parse_time_ms,
                    parse_result.confidence * 100.0
                );
            }
            ParseStrategy::Fallback => {
                println!(
                    "⚠️  Using fallback in {}ms (confidence: {:.0}%)",
                    parse_result.parse_time_ms,
                    parse_result.confidence * 100.0
                );
            }
        }

        let (cache_size, cache_cap) = self.nlp_parser.cache_stats().await;
        if cache_size > 0 && cache_size % 5 == 0 {
            println!("📦 Cache: {}/{} entries", cache_size, cache_cap);
        }

        let (task_title, scheduled_at, priority_value, tags_list) = match parse_result.item {
            ParsedItem::Task(nlp_task) => {
                let priority = match nlp_task.priority {
                    Priority::Urgent => 3,
                    Priority::High => 2,
                    Priority::Medium => 1,
                    Priority::Low => 0,
                };

                (nlp_task.title, nlp_task.due_date, priority, nlp_task.tags)
            }
            ParsedItem::Event(event) => (event.title, Some(event.start_time), 1, event.tags),
        };

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

        let tags_json = if tags_list.is_empty() {
            None
        } else {
            Some(serde_json::to_string(&tags_list).unwrap_or_default())
        };

        let category = self.classify_task(&task_title).to_string();

        sqlx::query(
            "INSERT INTO tasks (description, completed, item_order, priority, natural_language_input, tags, scheduled_at, task_category) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&task_title)
        .bind(false)
        .bind(new_order)
        .bind(priority_value)
        .bind(description)
        .bind(tags_json)
        .bind(scheduled_at)
        .bind(&category)
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
        Ok(())
    }

    pub async fn convert_task_to_event(&self, task_id: i64) -> Result<Option<i64>, sqlx::Error> {
        if let Some(task) = self.get_task_by_id(task_id).await?
            && let Some(scheduled_time) = task.scheduled_at
        {
            let event_result = sqlx::query(
                    "INSERT INTO events (title, description, start_time, end_time, created_at) VALUES (?, ?, ?, ?, ?)"
                )
                .bind(&task.description)
                .bind("Converted from task")
                .bind(scheduled_time)
                .bind(scheduled_time)
                .bind(chrono::Utc::now())
                .execute(&self.db_pool)
                .await?;

            return Ok(Some(event_result.last_insert_rowid()));
        }
        Ok(None)
    }

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

    pub async fn complete_task_by_id(&mut self, id: i64) -> Result<bool, sqlx::Error> {
        let rows_affected = sqlx::query("UPDATE tasks SET completed = true WHERE id = ?")
            .bind(id)
            .execute(&self.db_pool)
            .await?
            .rows_affected();

        Ok(rows_affected > 0)
    }

    pub async fn remove_task_by_id(&mut self, id: i64) -> Result<bool, sqlx::Error> {
        let rows_affected = sqlx::query("DELETE FROM tasks WHERE id = ?")
            .bind(id)
            .execute(&self.db_pool)
            .await?
            .rows_affected();

        Ok(rows_affected > 0)
    }

    pub async fn clear_completed_tasks(&mut self) -> Result<u64, sqlx::Error> {
        let rows_affected = sqlx::query("DELETE FROM tasks WHERE completed = true")
            .execute(&self.db_pool)
            .await?
            .rows_affected();

        Ok(rows_affected)
    }

    pub async fn get_task_by_id(&self, id: i64) -> Result<Option<Task>, sqlx::Error> {
        let task = sqlx::query_as::<_, Task>(
            "SELECT id, description, completed, item_order, scheduled_at, priority, tags, natural_language_input, task_category FROM tasks WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.db_pool)
        .await?;

        Ok(task)
    }

    // Calendar navigation methods
    pub fn calendar_move_up(&mut self) {
        self.selected_time_slot = self.selected_time_slot.saturating_sub(1);
    }

    pub fn calendar_move_down(&mut self) {
        if self.selected_time_slot < 15 {
            self.selected_time_slot += 1;
        }
    }

    pub fn calendar_move_left(&mut self) {
        self.selected_day = self.selected_day.saturating_sub(1);
    }

    pub fn calendar_move_right(&mut self) {
        if self.selected_day < 6 {
            self.selected_day += 1;
        }
    }

    pub fn selected_cell_date(&self) -> NaiveDate {
        let today = chrono::Local::now().naive_local().date();
        let week_offset = self.calendar_week_offset.unwrap_or(0);
        let start_of_week = today + Duration::weeks(week_offset)
            - Duration::days(today.weekday().num_days_from_monday() as i64);
        start_of_week + Duration::days(self.selected_day as i64)
    }

    pub fn selected_cell_time(&self) -> NaiveTime {
        let hour = 7 + self.selected_time_slot as u32;
        NaiveTime::from_hms_opt(hour, 0, 0).unwrap()
    }

    // Schedule block creation
    pub async fn create_schedule_block(&mut self) -> Result<(), sqlx::Error> {
        let day_of_week = self.selected_cell_date().weekday().num_days_from_monday() as i32;

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

    // Task scheduling methods
    pub fn unscheduled_tasks(&self) -> Vec<&Task> {
        self.tasks
            .iter()
            .filter(|t| !t.completed && t.scheduled_at.is_none())
            .collect()
    }

    pub async fn schedule_task_to_selected_cell(&mut self) -> Result<(), sqlx::Error> {
        let unscheduled: Vec<i64> = self.unscheduled_tasks().iter().map(|t| t.id).collect();
        if self.task_picker_selected >= unscheduled.len() {
            return Ok(());
        }

        let task_id = unscheduled[self.task_picker_selected];
        let date = self.selected_cell_date();
        let time = self.selected_cell_time();
        let datetime = date.and_time(time).and_utc();

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

    pub async fn add_task_at_selected_cell(
        &mut self,
        description: &str,
    ) -> Result<(), sqlx::Error> {
        let scheduled_at = self
            .selected_cell_date()
            .and_time(self.selected_cell_time())
            .and_utc();
        let category = self.classify_task(description).to_string();
        let new_order = self.tasks.len() as i64;

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
        let range_start = days[0].and_hms_opt(0, 0, 0).unwrap().and_utc();
        let range_end = days[days.len() - 1]
            .and_hms_opt(23, 59, 59)
            .unwrap()
            .and_utc();

        let scheduled_tasks = sqlx::query_as::<_, Task>(
            "SELECT id, description, completed, item_order, scheduled_at, priority, tags, natural_language_input, task_category FROM tasks WHERE scheduled_at >= ? AND scheduled_at < ? AND completed = 0"
        )
        .bind(range_start)
        .bind(range_end)
        .fetch_all(&self.db_pool)
        .await?;

        let occupied_slots: Vec<(NaiveDate, u32)> = scheduled_tasks
            .iter()
            .filter_map(|t| t.scheduled_at.map(|dt| (dt.date_naive(), dt.time().hour())))
            .collect();

        // Strategy 1: Find a matching block type with a free hour
        for day in &days {
            let dow = day.weekday().num_days_from_monday() as i32;
            for block in &blocks {
                if block.day_of_week != dow {
                    continue;
                }
                if block.block_type != task_category {
                    continue;
                }
                let start = match parse_time_string(&block.start_time) {
                    Some(t) => t,
                    None => continue,
                };
                let end = match parse_time_string(&block.end_time) {
                    Some(t) => t,
                    None => continue,
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
                        let time = NaiveTime::from_hms_opt(hour, 0, 0).unwrap();
                        return Ok(Some(day.and_time(time).and_utc()));
                    }
                    hour += 1;
                }
            }
        }

        // Strategy 2: Find any free hour (7am-11pm) not inside a different-type block
        for day in &days {
            let dow = day.weekday().num_days_from_monday() as i32;
            for hour in 7u32..23 {
                // Skip past hours for today
                if *day == today && hour <= now.hour() {
                    continue;
                }

                // Check if this hour is inside a different-type block
                let time = NaiveTime::from_hms_opt(hour, 0, 0).unwrap();
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
                    return Ok(Some(day.and_time(time).and_utc()));
                }
            }
        }

        Ok(None)
    }

    // ===== Schedule TOML Import/Export =====

    /// Validate time string format "HH:MM"
    fn validate_time_format(time: &str) -> Result<(), Box<dyn std::error::Error>> {
        let parts: Vec<&str> = time.split(':').collect();
        if parts.len() != 2 {
            return Err(format!("Invalid time format: {}", time).into());
        }

        let hour: u32 = parts[0]
            .parse()
            .map_err(|_| format!("Invalid hour in: {}", time))?;
        let minute: u32 = parts[1]
            .parse()
            .map_err(|_| format!("Invalid minute in: {}", time))?;

        if hour > 23 {
            return Err(format!("Hour out of range: {}", time).into());
        }
        if minute > 59 {
            return Err(format!("Minute out of range: {}", time).into());
        }

        Ok(())
    }

    /// Parse "HH:MM" to minutes since midnight
    fn time_to_minutes(time: &str) -> Option<u32> {
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
    /// - Compound days: "monday_wednesday", "tuesday_thursday"
    /// - Special groups: "weekdays", "weekends", "everyday"
    ///
    /// Uses Monday-first numbering to match chrono's num_days_from_monday():
    /// Monday = 0, Tuesday = 1, ..., Sunday = 6
    fn parse_days(name: &str) -> Result<Vec<i32>, Box<dyn std::error::Error>> {
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
                _ => return Err(format!("Invalid day name: {} (in '{}')", part, name).into()),
            };
            if !days.contains(&day_num) {
                days.push(day_num);
            }
        }

        if days.is_empty() {
            return Err(format!("Invalid day name: {}", name).into());
        }

        days.sort();
        Ok(days)
    }

    /// Convert day number to name (Monday-first: Mon=0, Sun=6)
    fn day_number_to_name(num: i32) -> String {
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

    pub async fn import_schedule_from_toml(
        &mut self,
        path: &Path,
    ) -> Result<usize, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let schedule: ScheduleToml = toml::from_str(&content)?;

        let mut imported = 0;

        for block in schedule.blocks {
            // Parse day name(s) - supports compound days like "monday_wednesday"
            let days = Self::parse_days(&block.day)?;

            // Validate time format
            Self::validate_time_format(&block.start)?;
            Self::validate_time_format(&block.end)?;

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

    pub async fn export_schedule_to_toml(
        &self,
        path: &Path,
    ) -> Result<usize, Box<dyn std::error::Error>> {
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

    pub async fn clear_all_schedule_blocks(&mut self) -> Result<u64, sqlx::Error> {
        let result = sqlx::query("DELETE FROM schedule_blocks")
            .execute(&self.db_pool)
            .await?;
        self.refresh_calendar_data().await;
        Ok(result.rows_affected())
    }

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
                println!("\n{}:", days[current_day as usize]);
            }
            println!(
                "  {} - {} [{}] {}",
                block.start_time, block.end_time, block.block_type, block.title
            );
        }

        Ok(())
    }

    pub async fn delete_block_at_selected_cell(&mut self) -> Result<(), sqlx::Error> {
        let date = self.selected_cell_date();
        let day_of_week = date.weekday().num_days_from_monday() as i32;
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
