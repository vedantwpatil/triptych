use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseResult {
    pub item: ParsedItem,
    pub strategy: ParseStrategy,
    pub confidence: f32,
    pub parse_time_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParsedItem {
    Task(Task),
    Event(Event),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub title: String,
    pub due_date: Option<DateTime<Utc>>,
    pub tags: Vec<String>,
    pub priority: Priority,
    pub is_scheduled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub title: String,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
    pub location: Option<String>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Priority {
    Low,
    Medium,
    High,
    Urgent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParseStrategy {
    Cached,
    Regex,
    Ollama,
    Fallback,
}
