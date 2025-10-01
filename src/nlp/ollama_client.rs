use crate::nlp::types::{Event, ParsedItem, Priority, Task};
use reqwest::{Client, Error as ReqwestError};
use serde::{Deserialize, Serialize};
use tokio::time::{Duration, timeout};

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

        // Apply timeout to prevent hanging
        let response = timeout(
            Duration::from_millis(OLLAMA_TIMEOUT_MS),
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
        format!(
            r#"Parse the following natural language input into structured JSON. 
Extract: type (task/event), title, datetime (ISO 8601), tags (array), priority (low/medium/high/urgent).

Examples:
Input: "Submit report tomorrow at 3pm #work"
Output: {{"type": "task", "title": "Submit report", "datetime": "2025-10-02T15:00:00Z", "tags": ["work"], "priority": "medium"}}

Input: "Team meeting next Monday 10am"
Output: {{"type": "event", "title": "Team meeting", "datetime": "2025-10-07T10:00:00Z", "tags": [], "priority": "medium"}}

Now parse: "{}"
Output:"#,
            input
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
