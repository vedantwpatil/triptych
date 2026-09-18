use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

use crate::email::{EmailConfig, ImapMailSource, MailSource, message, store as email_store};
use crate::nlp::{NLPParser, ParsedItem, Priority};
use ratatui::widgets::ListState;
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

/// Resolves the active database URL. Honors `DATABASE_URL` (matching
/// `src/bin/import_schedule.rs` and every other config value in this project) so
/// tests/tooling can point at an isolated database; falls back to `DB_URL` when unset.
fn db_url() -> String {
    std::env::var("DATABASE_URL").unwrap_or_else(|_| DB_URL.to_string())
}

#[derive(Debug, Clone, PartialEq)]
pub enum ViewMode {
    TodoList,
    Calendar,
    Email,
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

/// How far ahead `reallocate_all_tasks` looks for free blocks. A deadline past
/// this horizon is never even considered, so a conflict on such a task doesn't
/// mean the schedule is full - see `ConflictReason::BeyondWindow`.
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
            ConflictReason::BeyondWindow => {
                write!(f, "deadline past the {ALLOCATION_WINDOW_DAYS}-day planning window")
            }
            ConflictReason::OutOfCapacity => {
                write!(f, "no free deepwork/admin time before the deadline")
            }
        }
    }
}

/// Exclusive UTC end of the window `get_available_deepwork_blocks(ALLOCATION_WINDOW_DAYS)`
/// covers: instances exist for `today ..= today + (ALLOCATION_WINDOW_DAYS - 1)`, so the
/// first uncovered instant is local midnight starting day `ALLOCATION_WINDOW_DAYS`.
fn allocation_window_end(today: NaiveDate) -> DateTime<Utc> {
    resolve_local_datetime(
        (today + Duration::days(ALLOCATION_WINDOW_DAYS)).and_time(NaiveTime::MIN),
    )
}

fn classify_conflict(deadline: DateTime<Utc>, window_end: DateTime<Utc>) -> ConflictReason {
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
    DeadlineInput,
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
    /// Which task `m`/`u`/`e` act on when the selected cell holds more than
    /// one (see the same-hour-collision Known Issue) - `0` is the first
    /// (topmost) task, cycled with `[`/`]`. Reset to `0` on any cursor move so
    /// it never silently points at a task in a cell the cursor has left.
    pub stack_index: usize,
    pub calendar_input_mode: CalendarInputMode,
    pub block_form: BlockFormState,
    pub task_picker_selected: usize,
    pub input_buffer: String,
    nlp_parser: Arc<NLPParser>,
    pub cached_schedule_blocks: Vec<(NaiveDate, ScheduleBlock)>,
    /// (date, time, task_id, description, priority)
    pub cached_scheduled_tasks: Vec<(NaiveDate, NaiveTime, i64, String, i32)>,
    /// (date, time, task_id, description, allocated_minutes, priority)
    pub cached_task_allocations: Vec<(NaiveDate, NaiveTime, i64, String, i32, i32)>,
    pub status_message: Option<(String, std::time::Instant)>,
    /// Task picked up from the calendar with `m`, awaiting a drop cell.
    pub held_task: Option<i64>,
    /// Task whose deadline is being edited via `CalendarInputMode::DeadlineInput`.
    pub deadline_edit_task_id: Option<i64>,
    pub emails: Vec<crate::email::EmailMessage>,
    pub selected_email: usize,
    /// Set when the email detail popup is open (`v` on a selected email in
    /// the Email view); scroll resets to 0 each time it's opened.
    pub email_detail_open: bool,
    pub email_detail_scroll: u16,
    /// Persisted across frames so ratatui's viewport-scroll offset carries
    /// over between renders instead of recomputing from a fresh (offset 0)
    /// state every frame, which pins the selected row to the last visible
    /// line whenever the list is taller than one page.
    pub todo_list_state: ListState,
    pub email_list_state: ListState,
}

/// A task occupying a calendar cell, resolved from the current week's cached data.
pub struct CellTask {
    pub id: i64,
    /// True if this is a deadline-driven allocation rather than a manually
    /// scheduled task (see `reallocate_all_tasks`).
    pub is_allocation: bool,
}

/// Every task (manual + allocation) occupying a given day/hour, manual tasks
/// first. Kept free of `&self` so it's testable without a DB pool or NLP
/// parser. Callers that just want "is there anything here" use `.next()`;
/// `App::selected_cell_task` indexes into it with `stack_index` when more
/// than one task shares an hour (see `App::cycle_stack_next`/`cycle_stack_prev`).
fn cell_tasks(
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

/// Whether an allocation starting at `start` and lasting `minutes` covers the
/// on-the-hour instant `hour:00` - a multi-hour allocation (e.g. 90 minutes
/// starting at 9:00) must show in every hour cell it spans, not just the one
/// matching its exact start time. `pub(crate)` so `ui.rs`'s `cell_task_displays`
/// shares this exact rule rather than re-deriving it.
pub(crate) fn allocation_covers_hour(start: NaiveTime, minutes: i32, hour: u32) -> bool {
    let Some(slot) = NaiveTime::from_hms_opt(hour, 0, 0) else {
        return false;
    };
    let end = start + Duration::minutes(minutes as i64);
    start <= slot && slot < end
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
            stack_index: 0,
            calendar_input_mode: CalendarInputMode::Navigate,
            block_form: BlockFormState::new_at(0),
            task_picker_selected: 0,
            input_buffer: String::new(),
            nlp_parser,
            cached_schedule_blocks: Vec::new(),
            cached_scheduled_tasks: Vec::new(),
            cached_task_allocations: Vec::new(),
            status_message: None,
            held_task: None,
            deadline_edit_task_id: None,
            emails: Vec::new(),
            selected_email: 0,
            email_detail_open: false,
            email_detail_scroll: 0,
            todo_list_state: ListState::default(),
            email_list_state: ListState::default(),
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
    ) -> Result<Vec<(NaiveDate, NaiveTime, i64, String, i32, i32)>, sqlx::Error> {
        let start = days[0].to_string();
        let end = days[days.len() - 1].to_string();

        let rows: Vec<(String, String, i64, String, i32, i32)> = sqlx::query_as(
            r#"
            SELECT a.block_date, a.block_start_time, t.id, t.description, a.allocated_minutes, t.priority
            FROM task_block_allocations a
            JOIN tasks t ON a.task_id = t.id
            WHERE a.block_date BETWEEN ? AND ? AND t.completed = 0
            ORDER BY a.block_date, a.block_start_time, t.id
            "#,
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
    ) -> Result<Vec<(NaiveDate, NaiveTime, i64, String, i32)>, sqlx::Error> {
        let start = resolve_local_datetime(days[0].and_hms_opt(0, 0, 0).unwrap());
        let end = resolve_local_datetime(
            days[days.len() - 1].and_hms_opt(23, 59, 59).unwrap(),
        );

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

    pub async fn toggle_to_calendar(&mut self) {
        self.view_mode = ViewMode::Calendar;
        self.stack_index = 0;
        self.calendar_input_mode = CalendarInputMode::Navigate;
        let _ = self.load_tasks().await;
        self.refresh_calendar_data().await;
    }

    pub async fn toggle_to_todo(&mut self) {
        self.view_mode = ViewMode::TodoList;
        self.held_task = None;
        let _ = self.load_tasks().await;
    }

    pub async fn toggle_to_email(&mut self) {
        self.view_mode = ViewMode::Email;
        self.email_detail_open = false;
        self.sync_email_accounts();
        let _ = self.refresh_emails().await;
    }

    /// Kicks off a background pull of new mail from IMAP for every configured
    /// account, so the Email view catches up sooner than the next 60s
    /// `src/sync/mail.rs` poll instead of waiting on it. Fire-and-forget
    /// (`tokio::spawn`, not awaited) rather than the blocking call this used
    /// to be: run inline, a slow/unreachable IMAP server stalled the whole
    /// TUI (no redraw, no key input) until every account's TCP+TLS round-trip
    /// finished or failed. No-ops silently if email isn't configured;
    /// per-account failures are logged to stderr, same as the background
    /// poller, since there's no `&mut self` left to post a status_message to
    /// once the task is spawned.
    fn sync_email_accounts(&self) {
        let configs = EmailConfig::all_from_env();
        if configs.is_empty() {
            return;
        }

        let db_pool = self.db_pool.clone();
        tokio::spawn(async move {
            for config in &configs {
                let cursor = match email_store::get_sync_cursor(
                    &db_pool,
                    &config.account,
                    &config.imap_folder,
                )
                .await
                {
                    Ok(cursor) => cursor,
                    Err(e) => {
                        eprintln!("[Email] sync failed for '{}': {}", config.account, e);
                        continue;
                    }
                };
                let source = ImapMailSource::new(config.clone());
                match source.fetch_new(cursor).await {
                    Ok((uid_validity, raw_messages)) => {
                        let epoch_changed = match (cursor, uid_validity) {
                            (Some(c), Some(current)) => c.uid_validity as u32 != current,
                            _ => false,
                        };
                        if epoch_changed {
                            eprintln!(
                                "[Email] UIDVALIDITY changed for '{}'; resyncing recent mail instead of resuming",
                                config.account
                            );
                        }

                        let fetched_max_uid = raw_messages.iter().map(|(uid, _)| *uid).max();

                        let new_emails: Vec<_> = raw_messages
                            .into_iter()
                            .filter_map(|(uid, raw)| {
                                message::parse_raw(&config.account, uid, &config.imap_folder, &raw)
                                    .ok()
                            })
                            .collect();
                        if let Err(e) = email_store::insert_new(&db_pool, &new_emails).await {
                            eprintln!("[Email] sync failed for '{}': {}", config.account, e);
                        }
                        // See sync/mail.rs's sync_mail: skip persisting a synthetic
                        // `last_uid = 0` when the epoch changed but nothing came back, so
                        // the next sync retries the properly-capped catch-up.
                        if let Some(validity) = uid_validity
                            && !(epoch_changed && fetched_max_uid.is_none())
                        {
                            let prior_uid =
                                if epoch_changed { 0 } else { cursor.map_or(0, |c| c.last_uid) };
                            let last_uid =
                                fetched_max_uid.map_or(prior_uid, |uid| (uid as i64).max(prior_uid));
                            if let Err(e) = email_store::set_sync_cursor(
                                &db_pool,
                                &config.account,
                                &config.imap_folder,
                                email_store::SyncCursor { uid_validity: validity as i64, last_uid },
                            )
                            .await
                            {
                                eprintln!("[Email] sync failed for '{}': {}", config.account, e);
                            }
                        }
                    }
                    Err(e) => eprintln!("[Email] sync failed for '{}': {}", config.account, e),
                }
            }
        });
    }

    /// Tab: TodoList -> Calendar -> Email -> TodoList.
    pub async fn cycle_view_next(&mut self) {
        match self.view_mode {
            ViewMode::TodoList => self.toggle_to_calendar().await,
            ViewMode::Calendar => self.toggle_to_email().await,
            ViewMode::Email => self.toggle_to_todo().await,
        }
    }

    /// Shift+Tab: reverse of cycle_view_next.
    pub async fn cycle_view_prev(&mut self) {
        match self.view_mode {
            ViewMode::TodoList => self.toggle_to_email().await,
            ViewMode::Calendar => self.toggle_to_todo().await,
            ViewMode::Email => self.toggle_to_calendar().await,
        }
    }

    pub async fn refresh_emails(&mut self) -> Result<(), sqlx::Error> {
        self.emails = email_store::get_recent(&self.db_pool, 100)
            .await
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;

        if self.selected_email >= self.emails.len() {
            self.selected_email = self.emails.len().saturating_sub(1);
        }
        Ok(())
    }

    pub async fn mark_selected_email_read(&mut self) -> Result<(), sqlx::Error> {
        let Some(email) = self.emails.get(self.selected_email) else {
            return Ok(());
        };
        let email_id = email.id;

        crate::email::store::mark_read(&self.db_pool, email_id)
            .await
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;

        self.refresh_emails().await
    }

    /// Opens the email detail popup on the selected email and marks it read,
    /// same as most mail clients do on open.
    pub async fn open_selected_email(&mut self) -> Result<(), sqlx::Error> {
        if self.emails.get(self.selected_email).is_none() {
            return Ok(());
        }
        self.email_detail_open = true;
        self.email_detail_scroll = 0;
        self.mark_selected_email_read().await
    }

    pub fn close_email_detail(&mut self) {
        self.email_detail_open = false;
        self.email_detail_scroll = 0;
    }

    pub async fn convert_selected_email_to_task(&mut self) -> Result<(), sqlx::Error> {
        let Some(email) = self.emails.get(self.selected_email) else {
            return Ok(());
        };
        let email_id = email.id;
        let subject = email.subject.clone();

        self.add_task(&subject).await?;
        let task_id = self.tasks.get(self.selected).map(|t| t.id);

        if let Some(task_id) = task_id {
            crate::email::store::link_task(&self.db_pool, email_id, task_id)
                .await
                .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        }

        crate::email::store::mark_read(&self.db_pool, email_id)
            .await
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;

        self.status_message = Some(("Email converted to task".to_string(), std::time::Instant::now()));
        self.refresh_emails().await
    }

    pub async fn build() -> Result<Self, sqlx::Error> {
        let db_url = db_url();
        if !Sqlite::database_exists(&db_url).await.unwrap_or(false) {
            Sqlite::create_database(&db_url).await?;
        }

        let db_pool = SqlitePool::connect(&db_url).await?;
        sqlx::migrate!("./migrations").run(&db_pool).await?;

        let app = Self::new(db_pool).await;

        if app.nlp_parser.is_ollama_available() {
            println!("✓ NLP parsing ready");
        } else {
            println!("⚠ Ollama unavailable - limited parsing");
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

    pub async fn add_task(&mut self, description: &str) -> Result<i64, sqlx::Error> {
        let parse_result = self
            .nlp_parser
            .parse(description)
            .await
            .map_err(|e| sqlx::Error::Protocol(format!("NLP parsing failed: {}", e)))?;

        let (task_title, scheduled_at, priority_value, tags_list, deadline, duration_minutes) =
            extract_task_fields(parse_result.item);

        
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
                .unwrap_or(self.tasks.len() as i64);

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
        self.stack_index = 0;
    }

    pub fn calendar_move_down(&mut self) {
        if self.selected_time_slot < 15 {
            self.selected_time_slot += 1;
        }
        self.stack_index = 0;
    }

    pub fn calendar_move_left(&mut self) {
        self.selected_day = self.selected_day.saturating_sub(1);
        self.stack_index = 0;
    }

    pub fn calendar_move_right(&mut self) {
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
        cell_tasks(&self.cached_scheduled_tasks, &self.cached_task_allocations, day, hour).len()
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

    /// The task `m`/`u`/`e` act on: the one at `stack_index` within the
    /// selected cell, not always the first - see `cycle_stack_next`/`_prev`.
    fn selected_cell_task(&self) -> Option<CellTask> {
        let day = self.selected_cell_date();
        let hour = self.selected_cell_time().hour();
        let tasks = cell_tasks(&self.cached_scheduled_tasks, &self.cached_task_allocations, day, hour);
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
                self.status_message =
                    Some(("No scheduled task here".to_string(), std::time::Instant::now()));
            }
        }
    }

    /// Drop the held task (see `pick_up_task_at_selected_cell`) onto the selected cell.
    pub async fn drop_held_task(&mut self) -> Result<(), sqlx::Error> {
        let Some(task_id) = self.held_task.take() else {
            return Ok(());
        };

        let datetime = resolve_local_datetime(
            self.selected_cell_date().and_time(self.selected_cell_time()),
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
            self.status_message =
                Some(("Move cancelled".to_string(), std::time::Instant::now()));
        }
    }

    /// Clear the manually-set schedule of the task at the selected cell, returning
    /// it to the unscheduled pool. Deadline-driven allocations are left alone -
    /// they're recomputed by `reallocate_all_tasks`, not directly unscheduled.
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
                self.status_message =
                    Some(("No scheduled task here".to_string(), std::time::Instant::now()));
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

    /// Parse the pending deadline-edit input (e.g. "friday", "tomorrow") and
    /// apply it to the target task, then re-run allocation so the calendar
    /// reflects the new deadline immediately. Reuses the existing "by <word>"
    /// deadline grammar rather than adding a second date parser.
    pub async fn submit_deadline_edit(&mut self) -> Result<(), sqlx::Error> {
        let Some(task_id) = self.deadline_edit_task_id.take() else {
            self.calendar_input_mode = CalendarInputMode::Navigate;
            return Ok(());
        };

        let text = self.input_buffer.trim().to_string();
        self.input_buffer.clear();
        self.calendar_input_mode = CalendarInputMode::Navigate;

        if text.is_empty() {
            return Ok(());
        }

        let extract_deadline = |item: ParsedItem| match item {
            ParsedItem::Task(t) => t.deadline,
            ParsedItem::Event(_) => None,
        };

        let mut deadline = self
            .nlp_parser
            .parse(&format!("by {}", text))
            .await
            .ok()
            .and_then(|r| extract_deadline(r.item));

        if deadline.is_none() {
            deadline = self
                .nlp_parser
                .parse(&text)
                .await
                .ok()
                .and_then(|r| extract_deadline(r.item));
        }

        let Some(deadline) = deadline else {
            self.status_message = Some((
                format!("Couldn't parse deadline '{}' - try 'tomorrow' or a weekday", text),
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

    pub async fn add_task_at_selected_cell(
        &mut self,
        description: &str,
    ) -> Result<(), sqlx::Error> {
        let scheduled_at = resolve_local_datetime(
            self.selected_cell_date().and_time(self.selected_cell_time()),
        );
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
        let range_start = resolve_local_datetime(days[0].and_hms_opt(0, 0, 0).unwrap());
        let range_end = resolve_local_datetime(
            days[days.len() - 1].and_hms_opt(23, 59, 59).unwrap(),
        );

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
                        return Ok(Some(resolve_local_datetime(day.and_time(time))));
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
                    return Ok(Some(resolve_local_datetime(day.and_time(time))));
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
            // This task's own slice of the block, not the block's overall bounds -
            // sequential per allocation so multiple tasks sharing one block land on
            // different start times instead of every one stacking on the block's
            // own start (see `allocation_covers_hour`/`cell_task_displays`, which
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
        let blocks = self.get_available_deepwork_blocks(ALLOCATION_WINDOW_DAYS).await?;
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
    pub async fn on_task_changed(&mut self) -> Result<(), sqlx::Error> {
        let result = self.reallocate_all_tasks().await?;

        if let Some(summary) = result.conflict_summary() {
            self.status_message = Some((format!("Warning: {summary}"), std::time::Instant::now()));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_task_picks_deepwork_for_coding_keywords() {
        assert_eq!(classify_task("implement the new parser"), "deepwork");
        assert_eq!(classify_task("leetcode grind"), "deepwork");
    }

    #[test]
    fn classify_task_picks_admin_for_scheduling_keywords() {
        assert_eq!(classify_task("schedule dentist call"), "admin");
    }

    #[test]
    fn classify_task_picks_learning_for_reading_keywords() {
        assert_eq!(classify_task("read chapter 3"), "learning");
    }

    #[test]
    fn classify_task_falls_back_to_general() {
        assert_eq!(classify_task("buy groceries"), "general");
    }

    #[test]
    fn default_duration_matches_category() {
        assert_eq!(default_duration_for_category("deepwork"), 90);
        assert_eq!(default_duration_for_category("admin"), 30);
        assert_eq!(default_duration_for_category("learning"), 60);
        assert_eq!(default_duration_for_category("general"), 60);
    }

    #[test]
    fn resolve_local_datetime_roundtrips_a_normal_time() {
        let naive = NaiveDate::from_ymd_opt(2026, 3, 10)
            .unwrap()
            .and_hms_opt(9, 30, 0)
            .unwrap();
        let resolved = resolve_local_datetime(naive);
        let local = resolved.with_timezone(&chrono::Local);
        assert_eq!(local.naive_local(), naive);
    }

    #[test]
    fn allocation_window_end_is_local_midnight_after_the_last_covered_day() {
        let today = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
        let expected = resolve_local_datetime(
            (today + Duration::days(ALLOCATION_WINDOW_DAYS)).and_hms_opt(0, 0, 0).unwrap(),
        );
        assert_eq!(allocation_window_end(today), expected);
    }

    #[test]
    fn classify_conflict_flags_deadline_past_the_allocation_window() {
        let today = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
        let window_end = allocation_window_end(today);
        let far_deadline = window_end + Duration::days(1);
        assert_eq!(
            classify_conflict(far_deadline, window_end),
            ConflictReason::BeyondWindow
        );
        // Boundary: a deadline exactly at window_end wasn't considered either
        // (get_available_deepwork_blocks stops the day before), so it counts
        // as beyond the window rather than a capacity shortfall.
        assert_eq!(
            classify_conflict(window_end, window_end),
            ConflictReason::BeyondWindow
        );
    }

    #[test]
    fn classify_conflict_flags_capacity_when_deadline_is_inside_the_window() {
        let today = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
        let window_end = allocation_window_end(today);
        let near_deadline = window_end - Duration::days(1);
        assert_eq!(
            classify_conflict(near_deadline, window_end),
            ConflictReason::OutOfCapacity
        );
    }

    fn sample_conflict(reason: ConflictReason) -> TaskConflict {
        TaskConflict {
            task_id: 1,
            description: "sample".to_string(),
            needed_minutes: 90,
            allocated_minutes: 0,
            deadline: resolve_local_datetime(
                NaiveDate::from_ymd_opt(2026, 3, 10)
                    .unwrap()
                    .and_hms_opt(9, 0, 0)
                    .unwrap(),
            ),
            reason,
        }
    }

    #[test]
    fn conflict_summary_is_none_without_conflicts() {
        let result = AllocationResult::default();
        assert!(result.conflict_summary().is_none());
    }

    #[test]
    fn conflict_summary_reports_both_reason_counts() {
        let result = AllocationResult {
            conflicts: vec![
                sample_conflict(ConflictReason::BeyondWindow),
                sample_conflict(ConflictReason::OutOfCapacity),
                sample_conflict(ConflictReason::OutOfCapacity),
            ],
        };
        let summary = result.conflict_summary().expect("has conflicts");
        assert!(summary.contains("3 task(s) not scheduled"));
        assert!(summary.contains("1 past the 14-day window"));
        assert!(summary.contains("2 out of block capacity"));
    }

    #[test]
    fn parse_days_expands_special_groups() {
        assert_eq!(App::parse_days("weekdays").unwrap(), vec![0, 1, 2, 3, 4]);
        assert_eq!(App::parse_days("weekends").unwrap(), vec![5, 6]);
        assert_eq!(
            App::parse_days("everyday").unwrap(),
            vec![0, 1, 2, 3, 4, 5, 6]
        );
    }

    #[test]
    fn parse_days_expands_compound_names() {
        assert_eq!(App::parse_days("monday_wednesday").unwrap(), vec![0, 2]);
    }

    #[test]
    fn parse_days_rejects_unknown_names() {
        assert!(App::parse_days("someday").is_err());
    }

    #[test]
    fn time_to_minutes_parses_hh_mm() {
        assert_eq!(App::time_to_minutes("09:30"), Some(570));
        assert_eq!(App::time_to_minutes("00:00"), Some(0));
        assert_eq!(App::time_to_minutes("23:59"), Some(1439));
    }

    #[test]
    fn time_to_minutes_rejects_malformed_input() {
        assert_eq!(App::time_to_minutes("bogus"), None);
    }

    #[test]
    fn validate_time_format_accepts_in_range_times() {
        assert!(App::validate_time_format("09:30").is_ok());
        assert!(App::validate_time_format("23:59").is_ok());
    }

    #[test]
    fn validate_time_format_rejects_out_of_range_times() {
        assert!(App::validate_time_format("24:00").is_err());
        assert!(App::validate_time_format("09:60").is_err());
        assert!(App::validate_time_format("not-a-time").is_err());
    }

    #[test]
    fn day_number_to_name_matches_monday_first_numbering() {
        assert_eq!(App::day_number_to_name(0), "monday");
        assert_eq!(App::day_number_to_name(6), "sunday");
    }

    #[test]
    fn parse_time_string_handles_optional_seconds() {
        assert_eq!(
            parse_time_string("09:30"),
            NaiveTime::from_hms_opt(9, 30, 0)
        );
        assert_eq!(
            parse_time_string("09:30:15"),
            NaiveTime::from_hms_opt(9, 30, 15)
        );
        assert_eq!(parse_time_string("not-a-time"), None);
    }

    #[test]
    fn cell_tasks_finds_manual_and_allocated_tasks() {
        let day = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
        let other_day = NaiveDate::from_ymd_opt(2026, 3, 11).unwrap();
        let manual_time = NaiveTime::from_hms_opt(9, 0, 0).unwrap();
        let alloc_time = NaiveTime::from_hms_opt(14, 0, 0).unwrap();

        let scheduled_tasks = vec![(day, manual_time, 1i64, "write report".to_string(), 2i32)];
        let task_allocations = vec![(day, alloc_time, 2i64, "study rust".to_string(), 90i32, 1i32)];

        let manual = cell_tasks(&scheduled_tasks, &task_allocations, day, 9)
            .into_iter()
            .next()
            .expect("manual task at 9am");
        assert_eq!(manual.id, 1);
        assert!(!manual.is_allocation);

        let alloc = cell_tasks(&scheduled_tasks, &task_allocations, day, 14)
            .into_iter()
            .next()
            .expect("allocation at 2pm");
        assert_eq!(alloc.id, 2);
        assert!(alloc.is_allocation);

        assert!(cell_tasks(&scheduled_tasks, &task_allocations, day, 10).is_empty());
        assert!(cell_tasks(&scheduled_tasks, &task_allocations, other_day, 9).is_empty());
    }

    /// Regression for the "deadline allocations only ever render in their
    /// block's start hour" Known Issue: two tasks allocated into the same
    /// block must land on different, sequential start times, not both stack
    /// onto the block's own start.
    #[test]
    fn cell_tasks_separates_two_allocations_in_the_same_block() {
        let day = NaiveDate::from_ymd_opt(2026, 3, 10).unwrap();
        let block_start = NaiveTime::from_hms_opt(9, 0, 0).unwrap();
        let second_start = NaiveTime::from_hms_opt(10, 0, 0).unwrap();
        let scheduled_tasks = vec![];
        // First task takes the block's first 60 minutes, second task the next 30.
        let task_allocations = vec![
            (day, block_start, 1i64, "first task".to_string(), 60i32, 1i32),
            (day, second_start, 2i64, "second task".to_string(), 30i32, 1i32),
        ];

        let at_9am = cell_tasks(&scheduled_tasks, &task_allocations, day, 9);
        assert_eq!(at_9am.len(), 1);
        assert_eq!(at_9am[0].id, 1);

        let at_10am = cell_tasks(&scheduled_tasks, &task_allocations, day, 10);
        assert_eq!(at_10am.len(), 1);
        assert_eq!(at_10am[0].id, 2);
    }

    /// A single-connection in-memory DB, fully migrated the same way `App::build`
    /// migrates the real one, so these tests exercise the real read/write paths
    /// instead of a hand-rolled stand-in schema.
    async fn test_pool() -> SqlitePool {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory pool");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("base schema migration");
        crate::migrations::run_calendar_migration(&pool)
            .await
            .expect("calendar schema migration");
        pool
    }

    async fn insert_task(pool: &SqlitePool, description: &str) -> i64 {
        sqlx::query("INSERT INTO tasks (description, completed, item_order, priority) VALUES (?, false, 0, 1)")
            .bind(description)
            .execute(pool)
            .await
            .expect("insert task")
            .last_insert_rowid()
    }

    async fn insert_task_with_deadline(
        pool: &SqlitePool,
        description: &str,
        deadline: DateTime<Utc>,
        duration_minutes: i32,
    ) -> i64 {
        sqlx::query(
            "INSERT INTO tasks (description, completed, item_order, priority, deadline, duration_minutes) VALUES (?, false, 0, 1, ?, ?)"
        )
        .bind(description)
        .bind(deadline)
        .bind(duration_minutes)
        .execute(pool)
        .await
        .expect("insert task with deadline")
        .last_insert_rowid()
    }

    /// A deadline task with no eligible schedule blocks at all always misses its
    /// deadline; which `ConflictReason` it gets depends on whether the deadline
    /// itself falls inside or beyond the allocation window - see
    /// `reallocate_marks_far_deadline_as_beyond_window` and
    /// `reallocate_marks_near_deadline_as_out_of_capacity` below.
    #[tokio::test]
    async fn reallocate_marks_far_deadline_as_beyond_window() {
        let pool = test_pool().await;
        let far_deadline = resolve_local_datetime(
            (chrono::Local::now().naive_local().date() + Duration::days(30))
                .and_hms_opt(9, 0, 0)
                .unwrap(),
        );
        insert_task_with_deadline(&pool, "distant report", far_deadline, 90).await;

        let mut app = App::new(pool).await;
        let result = app.reallocate_all_tasks().await.expect("reallocate");

        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(result.conflicts[0].reason, ConflictReason::BeyondWindow);
    }

    #[tokio::test]
    async fn reallocate_marks_near_deadline_as_out_of_capacity() {
        let pool = test_pool().await;
        let near_deadline = resolve_local_datetime(
            (chrono::Local::now().naive_local().date() + Duration::days(1))
                .and_hms_opt(9, 0, 0)
                .unwrap(),
        );
        insert_task_with_deadline(&pool, "urgent report", near_deadline, 90).await;

        let mut app = App::new(pool).await;
        let result = app.reallocate_all_tasks().await.expect("reallocate");

        assert_eq!(result.conflicts.len(), 1);
        assert_eq!(result.conflicts[0].reason, ConflictReason::OutOfCapacity);
    }

    /// Round-trips a task through pick-up/drop: the goal of the "move a
    /// scheduled task in the calendar" half of Task 2. Regression guard for the
    /// write path (`drop_held_task`, naive-local-as-UTC) and read path
    /// (`get_scheduled_tasks_internal`) staying on the same convention - if they
    /// ever drift, the task would land in the wrong cell after a reload.
    #[tokio::test]
    async fn drop_held_task_moves_task_to_selected_cell_and_reload_agrees() {
        let pool = test_pool().await;
        let task_id = insert_task(&pool, "write report").await;

        let mut app = App::new(pool).await;
        app.held_task = Some(task_id);
        app.calendar_week_offset = Some(0);
        app.selected_day = 2;
        app.selected_time_slot = 3; // 7 + 3 = 10:00

        app.drop_held_task().await.expect("drop task");

        let target_day = app.selected_cell_date();
        let cell = cell_tasks(&app.cached_scheduled_tasks, &app.cached_task_allocations, target_day, 10)
            .into_iter()
            .next()
            .expect("task lands in dropped cell");
        assert_eq!(cell.id, task_id);
        assert!(!cell.is_allocation);
        assert!(app.held_task.is_none());
    }

    /// Every calendar-grid write path (schedule/drop/add-at-cell/auto-schedule)
    /// must store `scheduled_at` via `resolve_local_datetime`, the same
    /// conversion `nlp::rules` uses for NLP-parsed times - not a naive
    /// local-wall-clock value mislabeled as UTC. Pins the stored value itself
    /// (not just grid-cell self-consistency) so the two write paths can't
    /// silently drift back apart.
    #[tokio::test]
    async fn schedule_task_to_selected_cell_stores_true_utc_not_naive_local() {
        let pool = test_pool().await;
        let task_id = insert_task(&pool, "write report").await;

        let mut app = App::new(pool).await;
        app.load_tasks().await.expect("load tasks");
        app.calendar_week_offset = Some(0);
        app.selected_day = 2;
        app.selected_time_slot = 3; // 7 + 3 = 10:00

        app.schedule_task_to_selected_cell().await.expect("schedule task");

        let task = app.get_task_by_id(task_id).await.expect("query task").expect("task exists");
        let expected = resolve_local_datetime(
            app.selected_cell_date().and_time(app.selected_cell_time()),
        );
        assert_eq!(task.scheduled_at, Some(expected));
    }

    /// Round-trips a task through schedule -> unschedule: it must disappear from
    /// the calendar cell and reappear in the todolist's unscheduled pool - the
    /// other half of "reflect in the todolist".
    #[tokio::test]
    async fn unschedule_task_clears_cell_and_returns_task_to_unscheduled_pool() {
        let pool = test_pool().await;
        let task_id = insert_task(&pool, "study rust").await;

        let mut app = App::new(pool).await;
        app.calendar_week_offset = Some(0);
        app.selected_day = 1;
        app.selected_time_slot = 0; // 7:00
        app.held_task = Some(task_id);
        app.drop_held_task().await.expect("schedule task");

        let day = app.selected_cell_date();
        assert!(!cell_tasks(&app.cached_scheduled_tasks, &app.cached_task_allocations, day, 7).is_empty());

        app.unschedule_task_at_selected_cell().await.expect("unschedule task");

        assert!(cell_tasks(&app.cached_scheduled_tasks, &app.cached_task_allocations, day, 7).is_empty());
        assert!(app.unscheduled_tasks().iter().any(|t| t.id == task_id));
    }

    /// Editing a deadline from the calendar (the "move a deadline" half of
    /// Task 2) must persist through the same regex parser `add_task` uses, and
    /// show up on reload - it must not just update the DB row silently.
    #[tokio::test]
    async fn submit_deadline_edit_persists_parsed_deadline_and_reloads_task() {
        let pool = test_pool().await;
        let task_id = insert_task(&pool, "renew license").await;

        let mut app = App::new(pool).await;
        app.deadline_edit_task_id = Some(task_id);
        app.input_buffer = "tomorrow".to_string();

        app.submit_deadline_edit().await.expect("submit deadline edit");

        let task = app.get_task_by_id(task_id).await.expect("query task").expect("task exists");
        let deadline = task.deadline.expect("deadline parsed and saved");

        let expected_date = chrono::Local::now().naive_local().date() + Duration::days(1);
        assert_eq!(deadline.with_timezone(&chrono::Local).date_naive(), expected_date);
    }
}
