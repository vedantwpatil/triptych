//! Data types: DB rows, TOML schedule shapes, view/input modes and the block form.

use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use serde::{Deserialize, Serialize};

use super::time::resolve_local_datetime;
use sqlx::FromRow;

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

const fn default_priority() -> i32 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

// `task_category` mirrors the `tasks.task_category` DB column name (see `TASK_COLUMNS`) —
// renaming would need an sqlx column-rename shim, not worth it for a naming lint.
#[allow(clippy::struct_field_names)]
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

pub(super) const TASK_COLUMNS: &str = "id, description, completed, item_order, scheduled_at, deadline, duration_minutes, priority, tags, task_category";

/// A concrete occurrence of a recurring schedule block on a specific date
#[derive(Debug, Clone)]
pub struct BlockInstance {
    pub date: NaiveDate,
    pub start_time: NaiveTime,
    pub end_time: NaiveTime,
}

impl BlockInstance {
    pub(super) fn capacity_minutes(&self) -> i64 {
        (self.end_time - self.start_time).num_minutes()
    }

    pub(super) fn start_datetime_utc(&self) -> DateTime<Utc> {
        resolve_local_datetime(self.date.and_time(self.start_time))
    }
}

#[derive(Debug)]
pub struct EnhancedTaskInfo {
    pub task: Task,
    pub tags: Vec<String>,
}

#[derive(Debug)]
pub enum InputMode {
    Normal,
    Editing,
    /// Typing a `/` query in the todo or email list.
    Search,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CalendarInputMode {
    Navigate,
    BlockForm,
    TaskPicker,
    TaskInput,
    DeadlineInput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

    #[must_use]
    pub fn new_at(time_slot: usize) -> Self {
        let start_hour = 7 + time_slot;
        let end_hour = start_hour + 1;
        Self {
            block_type: "deepwork".to_string(),
            start_time: format!("{start_hour:02}:00"),
            end_time: format!("{end_hour:02}:00"),
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

    pub const fn next_field(&mut self) {
        self.active_field = match self.active_field {
            BlockFormField::BlockType => BlockFormField::StartTime,
            BlockFormField::StartTime => BlockFormField::EndTime,
            BlockFormField::EndTime => BlockFormField::Title,
            BlockFormField::Title => BlockFormField::BlockType,
        };
    }

    pub const fn prev_field(&mut self) {
        self.active_field = match self.active_field {
            BlockFormField::BlockType => BlockFormField::Title,
            BlockFormField::StartTime => BlockFormField::BlockType,
            BlockFormField::EndTime => BlockFormField::StartTime,
            BlockFormField::Title => BlockFormField::EndTime,
        };
    }
}
