use crate::nlp::types::{Event, ParsedItem, Priority, Task};
use chrono::Duration;
use reqwest::{Client, Error as ReqwestError};
use serde::{Deserialize, Serialize};
use tokio::time::timeout;

const OLLAMA_BASE_URL: &str = "http://localhost:11434";
const OLLAMA_TIMEOUT_MS: u64 = 15000;

#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    stream: bool,
    format: String,
}

#[derive(Deserialize)]
struct OllamaResponse {
    response: String,
}

#[derive(Deserialize)]
struct StructuredOutput {
    #[serde(rename = "type")]
    item_type: String,
    title: String,
    datetime: Option<String>,
    tags: Option<Vec<String>>,
    priority: Option<String>,
}

pub struct OllamaClient {
    client: Client,
    model: String,
}

impl OllamaClient {
    pub fn new(model: Option<String>) -> Self {
        Self {
            client: Client::new(),
            model: model.unwrap_or_else(|| "qwen2.5:7b".to_string()),
        }
    }

    pub async fn parse(&self, input: &str) -> Result<ParsedItem, OllamaError> {
        let prompt = self.build_prompt(input);

        let request = OllamaRequest {
            model: self.model.clone(),
            prompt,
            stream: false,
            format: "json".to_string(),
        };

        // Apply timeout to prevent hanging (use std::time::Duration for tokio)
        let response = timeout(
            std::time::Duration::from_millis(OLLAMA_TIMEOUT_MS),
            self.client
                .post(format!("{}/api/generate", OLLAMA_BASE_URL))
                .json(&request)
                .send(),
        )
        .await
        .map_err(|_| OllamaError::Timeout)?
        .map_err(OllamaError::Request)?;

        let ollama_response: OllamaResponse =
            response.json().await.map_err(OllamaError::Request)?;

        self.parse_response(&ollama_response.response)
    }

    fn build_prompt(&self, input: &str) -> String {
        // Get current date for context
        let now = chrono::Local::now();
        let today = now.format("%Y-%m-%d").to_string();
        let tomorrow = (now + Duration::days(1)).format("%Y-%m-%d").to_string();

        format!(
            r#"Today is {}. Parse the following natural language input into structured JSON.

CRITICAL TIME PARSING RULES:
- "4:12 PM" or "4:12 pm" → use 16:12:00 (afternoon)
- "4:12 AM" or "4:12 am" → use 04:12:00 (morning)  
- "12:00 PM" → use 12:00:00 (noon)
- "12:00 AM" → use 00:00:00 (midnight)
- Always output datetime in ISO 8601 format with timezone: YYYY-MM-DDTHH:MM:SS+00:00

Extract: type (task/event), title, datetime (ISO 8601 with UTC timezone), tags (array), priority (low/medium/high/urgent).

Examples:
Input: "Submit report tomorrow at 3pm #work"
Output: {{"type": "task", "title": "Submit report", "datetime": "{}T15:00:00+00:00", "tags": ["work"], "priority": "medium"}}

Input: "Meeting at 4:12 PM #important"
Output: {{"type": "task", "title": "Meeting", "datetime": "{}T16:12:00+00:00", "tags": ["important"], "priority": "medium"}}

Input: "Call John at 9:30 AM tomorrow"
Output: {{"type": "task", "title": "Call John", "datetime": "{}T09:30:00+00:00", "tags": [], "priority": "medium"}}

Now parse: "{}"
Output (ONLY valid JSON, no explanations):"#,
            today, tomorrow, today, tomorrow, input
        )
    }

    fn parse_response(&self, response: &str) -> Result<ParsedItem, OllamaError> {
        let structured: StructuredOutput =
            serde_json::from_str(response).map_err(|e| OllamaError::ParseError(e.to_string()))?;

        let datetime = structured
            .datetime
            .and_then(|dt| chrono::DateTime::parse_from_rfc3339(&dt).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));

        let priority = match structured.priority.as_deref() {
            Some("urgent") => Priority::Urgent,
            Some("high") => Priority::High,
            Some("low") => Priority::Low,
            _ => Priority::Medium,
        };

        let tags = structured.tags.unwrap_or_default();

        match structured.item_type.as_str() {
            "task" => Ok(ParsedItem::Task(Task {
                title: structured.title,
                due_date: datetime,
                tags,
                priority,
                is_scheduled: datetime.is_some(),
            })),
            "event" => Ok(ParsedItem::Event(Event {
                title: structured.title,
                start_time: datetime.ok_or_else(|| {
                    OllamaError::ParseError("Events require a datetime".to_string())
                })?,
                end_time: None,
                location: None,
                tags,
            })),
            _ => Err(OllamaError::ParseError(format!(
                "Unknown type: {}",
                structured.item_type
            ))),
        }
    }

    pub async fn health_check(&self) -> bool {
        self.client
            .get(format!("{}/api/tags", OLLAMA_BASE_URL))
            .send()
            .await
            .is_ok()
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum OllamaError {
    Timeout,
    Request(ReqwestError),
    ParseError(String),
    ServiceUnavailable,
}

impl std::fmt::Display for OllamaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OllamaError::Timeout => write!(f, "Ollama request timed out"),
            OllamaError::Request(e) => write!(f, "Request error: {}", e),
            OllamaError::ParseError(e) => write!(f, "Parse error: {}", e),
            OllamaError::ServiceUnavailable => write!(f, "Ollama service unavailable"),
        }
    }
}

impl std::error::Error for OllamaError {}
