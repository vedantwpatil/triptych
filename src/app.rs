//! Core application state (`App`) and its business logic, split by concern into `app/*.rs`;
//! see `src/app/CLAUDE.md`.

use chrono::NaiveDate;
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
mod motion;
mod placement;
mod schedule_io;
mod search;
mod tasks;
mod textedit;
mod time;

pub use allocation::*;
pub use calendar::*;
pub use mail::{
    ArchiveResult, AttachmentSaveResult, CATEGORY_ORDER, DeleteResult, FolderListResult,
    FolderSyncResult, MailSync, SendResult, Summary, SummaryDone, TriageDone, chain_references,
    compose_full_body, forward_subject, match_rule, merge_reply_all_cc, next_category,
    normalize_subject, parse_rule_spec, parse_snooze_spec, quote_original, reply_subject,
    sanitize_filename, thread_count,
};
pub use model::*;
pub use motion::*;
pub use tasks::*;
pub use textedit::{Edit, apply as apply_edit, before_cursor, cursor_col};
pub use time::*;

const DB_URL: &str = "sqlite:todo.db";

/// Resolves the active database URL. Honors `DATABASE_URL` (matching
/// `src/bin/import_schedule.rs` and every other config value in this project) so
/// tests/tooling can point at an isolated database; falls back to `DB_URL` when unset.
fn db_url() -> String {
    std::env::var("DATABASE_URL").unwrap_or_else(|_| DB_URL.to_string())
}

#[derive(Debug)]
#[allow(clippy::struct_excessive_bools)]
pub struct App {
    pub db_pool: SqlitePool,
    pub tasks: Vec<Task>,
    pub selected: usize,
    /// Row where visual selection (`v`/`V`) started in the todo list; the selection is this row
    /// through `selected`. `None` when not selecting.
    pub visual_anchor: Option<usize>,
    pub input_mode: InputMode,
    /// Count and `g` typed so far for a vim motion (`5j`, `gg`).
    pub key_prefix: KeyPrefix,
    /// Last `/` query, repeated by `n`/`N`.
    pub search_query: String,
    /// Rows the todo or email list showed at the last draw, for `Ctrl-d`/`Ctrl-u`.
    pub list_rows: usize,
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
    /// Caret in whichever text is being typed (byte offset, `None` = end); see `app/textedit.rs`.
    pub edit_cursor: Option<usize>,
    /// Task being reworded by the todo `r`/`R` prompt; `None` when the prompt adds a new task.
    pub editing_task_id: Option<i64>,
    nlp_parser: Arc<NLPParser>,
    pub cached_schedule_blocks: Vec<(NaiveDate, ScheduleBlock)>,
    /// Manually scheduled tasks this week; minutes is `duration_minutes` (see `CellEntry`).
    pub cached_scheduled_tasks: Vec<CellEntry>,
    /// Deadline allocations this week; minutes is `allocated_minutes`.
    pub cached_task_allocations: Vec<CellEntry>,
    pub status_message: Option<(String, std::time::Instant)>,
    /// Task picked up from the calendar with `m`, awaiting a drop cell.
    pub held_task: Option<i64>,
    /// Task whose deadline is being edited via `CalendarInputMode::DeadlineInput`.
    pub deadline_edit_task_id: Option<i64>,
    deadline_tx: tokio::sync::mpsc::UnboundedSender<DeadlineParse>,
    /// Results of background deadline parses; drained by `run_app` so the UI never blocks on NLP.
    pub deadline_rx: tokio::sync::mpsc::UnboundedReceiver<DeadlineParse>,
    task_tx: tokio::sync::mpsc::UnboundedSender<TaskParse>,
    /// Results of background task parses; drained by `run_app`, like `deadline_rx`.
    pub task_rx: tokio::sync::mpsc::UnboundedReceiver<TaskParse>,
    mail_tx: tokio::sync::mpsc::UnboundedSender<MailSync>,
    /// Results of background mail syncs; drained by `run_app`, like `task_rx`.
    pub mail_rx: tokio::sync::mpsc::UnboundedReceiver<MailSync>,
    /// A background sync is running; a second one is not started.
    mail_syncing: bool,
    /// `s` asked for the running sync's outcome to be reported.
    mail_manual: bool,
    summary_tx: tokio::sync::mpsc::UnboundedSender<SummaryDone>,
    /// Results of background email summaries; drained by `run_app`.
    pub summary_rx: tokio::sync::mpsc::UnboundedReceiver<SummaryDone>,
    /// AI summary state per email id, for emails opened this session.
    pub email_summaries: std::collections::HashMap<i64, Summary>,
    triage_tx: tokio::sync::mpsc::UnboundedSender<TriageDone>,
    /// Results of background email triage classifications; drained by `run_app`.
    pub triage_rx: tokio::sync::mpsc::UnboundedReceiver<TriageDone>,
    /// A background triage pass is running; a second one is not started.
    triage_running: bool,
    send_tx: tokio::sync::mpsc::UnboundedSender<SendResult>,
    /// Result of a background compose/reply/forward send; drained by `run_app`.
    pub send_rx: tokio::sync::mpsc::UnboundedReceiver<SendResult>,
    delete_tx: tokio::sync::mpsc::UnboundedSender<DeleteResult>,
    /// Result of a background email delete; drained by `run_app`.
    pub delete_rx: tokio::sync::mpsc::UnboundedReceiver<DeleteResult>,
    archive_tx: tokio::sync::mpsc::UnboundedSender<ArchiveResult>,
    /// Result of a background email archive; drained by `run_app`.
    pub archive_rx: tokio::sync::mpsc::UnboundedReceiver<ArchiveResult>,
    attachment_tx: tokio::sync::mpsc::UnboundedSender<AttachmentSaveResult>,
    /// Result of a background attachment save; drained by `run_app`.
    pub attachment_rx: tokio::sync::mpsc::UnboundedReceiver<AttachmentSaveResult>,
    /// Attachment metadata for emails opened this session (no bytes — see
    /// `MailSource::fetch_attachments`), keyed by email id. Populated by
    /// `App::open_selected_email`, mirroring `email_summaries`.
    pub email_attachments: std::collections::HashMap<i64, Vec<crate::email::EmailAttachment>>,
    /// The open compose/reply/forward form, when `input_mode` is `InputMode::EmailCompose`.
    pub email_compose: Option<ComposeState>,
    /// Loaded fresh from the DB each time the drafts list popup (`D` in the email view) opens.
    pub drafts: Vec<crate::email::Draft>,
    pub drafts_open: bool,
    pub selected_draft: usize,
    /// Loaded fresh from the DB each time the rules popup (`R` in the email view) opens.
    pub rules: Vec<crate::email::EmailRule>,
    pub rules_open: bool,
    pub selected_rule: usize,
    pub emails: Vec<crate::email::EmailMessage>,
    /// `None` shows every configured account's mail merged (the default); `Some(label)` restricts
    /// `refresh_emails` to that one account. Cycled with `A` in the email list.
    pub account_filter: Option<String>,
    /// `None` shows every synced folder merged (the default); `Some(name)` restricts
    /// `refresh_emails` to that one folder (e.g. `"INBOX"`, `"Archive"`). Cycled with `F` in the
    /// email list; see Slice 13 (folder browsing) in `docs/roadmap-email.md`.
    pub folder_filter: Option<String>,
    /// `false` (the default) shows the normal inbox, hiding any message with a future
    /// `snoozed_until`; `true` shows only those, so a snooze can be reviewed or cleared early.
    /// Toggled with `Z` in the email list.
    pub show_snoozed: bool,
    /// `None` shows every email regardless of triage (the default); `Some(true)`/`Some(false)`
    /// restricts `refresh_emails` to Focused/Other only (Outlook's Focused Inbox split). Cycled
    /// with `I` in the email list.
    pub focus_filter: Option<bool>,
    /// `None` shows every email regardless of attachments (the default); `Some(true)`/`Some(false)`
    /// restricts `refresh_emails` to messages with/without at least one attachment. Cycled with `H`
    /// in the email list (Slice 23).
    pub attachment_filter: Option<bool>,
    /// `None` shows every email regardless of read state (the default); `Some(true)`/`Some(false)`
    /// restricts `refresh_emails` to unread/read only. Cycled with `U` in the email list (Slice 24).
    pub unread_filter: Option<bool>,
    /// `None` shows every email regardless of star (the default); `Some(true)`/`Some(false)`
    /// restricts `refresh_emails` to starred/unstarred only. Cycled with `S` in the email list
    /// (Slice 24).
    pub starred_filter: Option<bool>,
    /// `None` shows every sender domain merged (the default); `Some(domain)` restricts
    /// `refresh_emails` to senders at that domain (e.g. `"github.com"`). Cycled with `@` in the
    /// email list (Slice 25).
    pub domain_filter: Option<String>,
    folder_list_tx: tokio::sync::mpsc::UnboundedSender<FolderListResult>,
    /// Result of a background folder-discovery LIST pass; drained by `run_app`.
    pub folder_list_rx: tokio::sync::mpsc::UnboundedReceiver<FolderListResult>,
    folder_sync_tx: tokio::sync::mpsc::UnboundedSender<FolderSyncResult>,
    /// Result of a background one-folder sync kicked off from the folder browser; drained by
    /// `run_app`.
    pub folder_sync_rx: tokio::sync::mpsc::UnboundedReceiver<FolderSyncResult>,
    /// Set when the folder-browser popup (`B` in the email list) is open.
    pub folder_browser_open: bool,
    /// `(account, folder)` pairs found by the last `App::open_folder_browser` LIST pass,
    /// alphabetical by account then folder. Empty until that finishes.
    pub discovered_folders: Vec<(String, String)>,
    pub selected_discovered_folder: usize,
    pub email_sort: crate::email::EmailSort,
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
        let (task_tx, task_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mail_tx, mail_rx) = tokio::sync::mpsc::unbounded_channel();
        let (summary_tx, summary_rx) = tokio::sync::mpsc::unbounded_channel();
        let (send_tx, send_rx) = tokio::sync::mpsc::unbounded_channel();
        let (delete_tx, delete_rx) = tokio::sync::mpsc::unbounded_channel();
        let (archive_tx, archive_rx) = tokio::sync::mpsc::unbounded_channel();
        let (attachment_tx, attachment_rx) = tokio::sync::mpsc::unbounded_channel();
        let (triage_tx, triage_rx) = tokio::sync::mpsc::unbounded_channel();
        let (folder_list_tx, folder_list_rx) = tokio::sync::mpsc::unbounded_channel();
        let (folder_sync_tx, folder_sync_rx) = tokio::sync::mpsc::unbounded_channel();

        Self {
            db_pool: pool,
            tasks: Vec::new(),
            selected: 0,
            visual_anchor: None,
            input_mode: InputMode::Normal,
            key_prefix: KeyPrefix::default(),
            search_query: String::new(),
            list_rows: 10,
            view_mode: ViewMode::TodoList,
            calendar_week_offset: None,
            selected_day: 0,
            selected_time_slot: 0,
            stack_index: 0,
            calendar_input_mode: CalendarInputMode::Navigate,
            block_form: BlockFormState::new_at(0),
            task_picker_selected: 0,
            input_buffer: String::new(),
            edit_cursor: None,
            editing_task_id: None,
            nlp_parser,
            cached_schedule_blocks: Vec::new(),
            cached_scheduled_tasks: Vec::new(),
            cached_task_allocations: Vec::new(),
            status_message: None,
            held_task: None,
            deadline_edit_task_id: None,
            deadline_tx,
            deadline_rx,
            task_tx,
            task_rx,
            mail_tx,
            mail_rx,
            mail_syncing: false,
            mail_manual: false,
            summary_tx,
            summary_rx,
            email_summaries: std::collections::HashMap::new(),
            triage_tx,
            triage_rx,
            triage_running: false,
            send_tx,
            send_rx,
            delete_tx,
            delete_rx,
            archive_tx,
            archive_rx,
            attachment_tx,
            attachment_rx,
            email_attachments: std::collections::HashMap::new(),
            email_compose: None,
            drafts: Vec::new(),
            drafts_open: false,
            selected_draft: 0,
            rules: Vec::new(),
            rules_open: false,
            selected_rule: 0,
            emails: Vec::new(),
            account_filter: None,
            folder_filter: None,
            show_snoozed: false,
            focus_filter: None,
            attachment_filter: None,
            unread_filter: None,
            starred_filter: None,
            domain_filter: None,
            folder_list_tx,
            folder_list_rx,
            folder_sync_tx,
            folder_sync_rx,
            folder_browser_open: false,
            discovered_folders: Vec::new(),
            selected_discovered_folder: 0,
            email_sort: crate::email::EmailSort::default(),
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
        self.start_email_sync(false);
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
