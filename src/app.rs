use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

use crate::nlp::{NLPParser, ParsedItem, Priority};
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
    pub deadline: Option<DateTime<Utc>>,
    pub duration_minutes: Option<i32>,
    pub priority: i32,
    pub tags: Option<String>,
    pub task_category: Option<String>,
}

const TASK_COLUMNS: &str = "id, description, completed, item_order, scheduled_at, deadline, duration_minutes, priority, tags, task_category";

/// A concrete occurrence of a recurring schedule block on a specific date
#[derive(Debug, Clone)]
pub struct BlockInstance {
    pub date: NaiveDate,
    pub start_time: NaiveTime,
    pub end_time: NaiveTime,
}

impl BlockInstance {
    fn capacity_minutes(&self) -> i64 {
        (self.end_time - self.start_time).num_minutes()
    }

    fn start_datetime_utc(&self) -> DateTime<Utc> {
        resolve_local_datetime(self.date.and_time(self.start_time))
    }
}

/// Resolve a naive local wall-clock datetime to UTC without ever panicking on a
/// DST transition: an ambiguous time (fall-back) resolves to its earlier instant,
/// a nonexistent time (spring-forward gap) is nudged forward in hourly steps
/// until a valid local time is found.
pub(crate) fn resolve_local_datetime(naive: chrono::NaiveDateTime) -> DateTime<Utc> {
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

#[derive(Debug)]
pub struct TaskConflict {
    pub task_id: i64,
    pub description: String,
    pub needed_minutes: i32,
    pub allocated_minutes: i32,
    pub deadline: DateTime<Utc>,
}

#[derive(Debug, Default)]
pub struct AllocationResult {
    pub conflicts: Vec<TaskConflict>,
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

pub fn default_duration_for_category(category: &str) -> i32 {
    match category {
        "deepwork" => 90,
        "admin" => 30,
        "learning" => 60,
        _ => 60,
    }
}

/// (title, scheduled_at, priority, tags, deadline, duration_minutes)
pub type ExtractedTaskFields = (
    String,
    Option<DateTime<Utc>>,
    i32,
    Vec<String>,
    Option<DateTime<Utc>>,
    Option<i32>,
);

/// Extract the fields needed to insert a task from a parsed NLP result. Shared
/// between `App::add_task` (TUI/CLI path) and the daemon's fast-add path so the
/// two never drift on priority mapping or Task/Event handling.
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
        ParsedItem::Event(event) => (
            event.title,
            Some(event.start_time),
            1,
            event.tags,
            None,
            None,
        ),
    }
}

#[derive(Debug)]
pub struct EnhancedTaskInfo {
    pub task: Task,
    pub tags: Vec<String>,
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
    pub cached_task_allocations: Vec<(NaiveDate, NaiveTime, String, i32, i32)>,
    pub status_message: Option<(String, std::time::Instant)>,
}

pub(crate) fn parse_time_string(time_str: &str) -> Option<NaiveTime> {
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
            cached_task_allocations: Vec::new(),
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

        self.cached_task_allocations = self
            .get_week_allocations_internal(&days)
            .await
            .unwrap_or_default();
    }

    async fn get_week_allocations_internal(
        &self,
        days: &[NaiveDate],
    ) -> Result<Vec<(NaiveDate, NaiveTime, String, i32, i32)>, sqlx::Error> {
        let start = days[0].to_string();
        let end = days[days.len() - 1].to_string();

        let rows: Vec<(String, String, String, i32, i32)> = sqlx::query_as(
            r#"
            SELECT a.block_date, a.block_start_time, t.description, a.allocated_minutes, t.priority
            FROM task_block_allocations a
            JOIN tasks t ON a.task_id = t.id
            WHERE a.block_date BETWEEN ? AND ? AND t.completed = 0
            ORDER BY a.block_date, a.block_start_time
            "#,
        )
        .bind(start)
        .bind(end)
        .fetch_all(&self.db_pool)
        .await?;

        Ok(rows
            .into_iter()
            .filter_map(|(date_str, time_str, desc, minutes, priority)| {
                let date = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d").ok()?;
                let time = parse_time_string(&time_str)?;
                Some((date, time, desc, minutes, priority))
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

        let query = format!(
            "SELECT {TASK_COLUMNS} FROM tasks WHERE scheduled_at >= ? AND scheduled_at < ? AND completed = 0 ORDER BY scheduled_at"
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
        let query = format!("SELECT {TASK_COLUMNS} FROM tasks ORDER BY item_order ASC");
        self.tasks = sqlx::query_as::<_, Task>(&query)
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

        let (task_title, scheduled_at, priority_value, tags_list, deadline, duration_minutes) =
            extract_task_fields(parse_result.item);

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

        let category = classify_task(&task_title).to_string();
        let duration_minutes =
            duration_minutes.unwrap_or_else(|| default_duration_for_category(&category));

        sqlx::query(
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
        .await?;

        self.load_tasks().await?;

        self.selected = self
            .tasks
            .iter()
            .position(|t| t.item_order == Some(new_order))
            .unwrap_or(0);

        if deadline.is_some() {
            self.on_task_changed().await?;
        }

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
        self.on_task_changed().await?;
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
        self.on_task_changed().await?;
        Ok(())
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

        if rows_affected > 0 {
            self.on_task_changed().await?;
        }
        Ok(rows_affected > 0)
    }

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

    pub async fn get_task_by_id(&self, id: i64) -> Result<Option<Task>, sqlx::Error> {
        let query = format!("SELECT {TASK_COLUMNS} FROM tasks WHERE id = ?");
        let task = sqlx::query_as::<_, Task>(&query)
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
        let category = classify_task(description).to_string();
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

    // ===== Smart Task Scheduling =====

    /// Get incomplete tasks with a deadline, earliest deadline first.
    /// Allocations are deadline-driven and additive: they never touch `scheduled_at`,
    /// which remains under manual/direct-scheduling control (auto_schedule_task,
    /// schedule_task_to_selected_cell). Tasks without a deadline are never reallocated.
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

    /// Expand recurring schedule_blocks into concrete per-date instances over the
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
            let dow = date.weekday().num_days_from_monday() as i32;

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

            sqlx::query(
                "INSERT INTO task_block_allocations (task_id, block_date, block_start_time, block_end_time, allocated_minutes) VALUES (?, ?, ?, ?, ?)"
            )
            .bind(task_id)
            .bind(block.date.to_string())
            .bind(block.start_time.format("%H:%M").to_string())
            .bind(block.end_time.format("%H:%M").to_string())
            .bind(take as i32)
            .execute(&mut **tx)
            .await?;

            usage.insert((block.date, block.start_time), used + take);
            remaining -= take;
        }

        Ok(needed_minutes - remaining)
    }

    /// Reallocate every incomplete, deadline-bearing task to available deepwork/admin
    /// blocks in the next two weeks, earliest-deadline-first. Additive to the existing
    /// `scheduled_at`-based flow: this only ever writes task_block_allocations rows.
    /// The clear-and-rebuild runs inside a transaction so a mid-run error leaves the
    /// previous allocations intact rather than a half-rewritten table.
    pub async fn reallocate_all_tasks(&mut self) -> Result<AllocationResult, sqlx::Error> {
        let tasks = self.get_tasks_by_deadline().await?;
        let blocks = self.get_available_deepwork_blocks(14).await?;

        let mut tx = self.db_pool.begin().await?;
        Self::clear_all_allocations(&mut tx).await?;

        let mut block_usage: std::collections::HashMap<(NaiveDate, NaiveTime), i64> =
            std::collections::HashMap::new();
        let mut conflicts = Vec::new();

        for task in &tasks {
            let Some(deadline) = task.deadline else {
                continue;
            };
            let needed_minutes = task.duration_minutes.unwrap_or(90) as i64;

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
                    needed_minutes: needed_minutes as i32,
                    allocated_minutes: allocated as i32,
                    deadline,
                });
            }
        }

        tx.commit().await?;

        self.load_tasks().await?;
        self.refresh_calendar_data().await;

        Ok(AllocationResult { conflicts })
    }

    /// Called after a task with a deadline is added, so the schedule stays current.
    pub async fn on_task_changed(&mut self) -> Result<(), sqlx::Error> {
        let result = self.reallocate_all_tasks().await?;

        if !result.conflicts.is_empty() {
            self.status_message = Some((
                format!(
                    "Warning: {} task(s) cannot fit before deadline",
                    result.conflicts.len()
                ),
                std::time::Instant::now(),
            ));
        }

        Ok(())
    }
}
