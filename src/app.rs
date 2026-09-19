//! Core application state (`App`) and its business logic, split by concern into `app/*.rs`;
//! see `src/app/CLAUDE.md`.

use chrono::{NaiveDate, NaiveTime};
use std::sync::Arc;

use crate::nlp::NLPParser;
use ratatui::widgets::ListState;
use sqlx::{
    migrate::MigrateDatabase,
    sqlite::{Sqlite, SqlitePool},
};

mod allocation;
mod calendar;
mod mail;
mod model;
mod placement;
mod schedule_io;
mod tasks;
mod time;

pub use allocation::*;
pub use calendar::*;
pub use model::*;
pub use tasks::*;
pub use time::*;

const DB_URL: &str = "sqlite:todo.db";

/// Resolves the active database URL. Honors `DATABASE_URL` (matching
/// `src/bin/import_schedule.rs` and every other config value in this project) so
/// tests/tooling can point at an isolated database; falls back to `DB_URL` when unset.
fn db_url() -> String {
    std::env::var("DATABASE_URL").unwrap_or_else(|_| DB_URL.to_string())
}

#[derive(Debug)]
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
    /// (date, time, `task_id`, description, priority)
    pub cached_scheduled_tasks: Vec<(NaiveDate, NaiveTime, i64, String, i32)>,
    /// (date, time, `task_id`, description, `allocated_minutes`, priority)
    pub cached_task_allocations: Vec<(NaiveDate, NaiveTime, i64, String, i32, i32)>,
    pub status_message: Option<(String, std::time::Instant)>,
    /// Task picked up from the calendar with `m`, awaiting a drop cell.
    pub held_task: Option<i64>,
    /// Task whose deadline is being edited via `CalendarInputMode::DeadlineInput`.
    pub deadline_edit_task_id: Option<i64>,
    deadline_tx: tokio::sync::mpsc::UnboundedSender<DeadlineParse>,
    /// Results of background deadline parses; drained by `run_app` so the UI never blocks on NLP.
    pub deadline_rx: tokio::sync::mpsc::UnboundedReceiver<DeadlineParse>,
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

impl App {
    pub async fn new(pool: SqlitePool) -> Self {
        let nlp_parser = Arc::new(NLPParser::new().await);
        let (deadline_tx, deadline_rx) = tokio::sync::mpsc::unbounded_channel();

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
            deadline_tx,
            deadline_rx,
            emails: Vec::new(),
            selected_email: 0,
            email_detail_open: false,
            email_detail_scroll: 0,
            todo_list_state: ListState::default(),
            email_list_state: ListState::default(),
        }
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
        self.cleanup_old_emails().await;
        let _ = self.refresh_emails().await;
    }

    /// Tab: `TodoList` -> Calendar -> Email -> `TodoList`.
    pub async fn cycle_view_next(&mut self) {
        match self.view_mode {
            ViewMode::TodoList => self.toggle_to_calendar().await,
            ViewMode::Calendar => self.toggle_to_email().await,
            ViewMode::Email => self.toggle_to_todo().await,
        }
    }

    /// Shift+Tab: reverse of `cycle_view_next`.
    pub async fn cycle_view_prev(&mut self) {
        match self.view_mode {
            ViewMode::TodoList => self.toggle_to_email().await,
            ViewMode::Calendar => self.toggle_to_todo().await,
            ViewMode::Email => self.toggle_to_calendar().await,
        }
    }

    /// Opens the database (creating and migrating it first if needed) and builds the `App`.
    ///
    /// # Errors
    ///
    /// Returns an error if the database cannot be created, opened or migrated.
    pub async fn build() -> Result<Self, sqlx::Error> {
        let db_url = db_url();
        if !Sqlite::database_exists(&db_url).await.unwrap_or(false) {
            Sqlite::create_database(&db_url).await?;
        }

        let db_pool = SqlitePool::connect(&db_url).await?;
        sqlx::migrate!("./migrations").run(&db_pool).await?;

        let app = Self::new(db_pool).await;

        if app.nlp_parser.is_ollama_available() {
            tracing::info!("NLP parsing ready");
        } else {
            tracing::warn!("Ollama unavailable; limited parsing");
        }

        Ok(app)
    }

    #[must_use]
    pub fn nlp_parser_ref(&self) -> Arc<NLPParser> {
        Arc::clone(&self.nlp_parser)
    }
}
