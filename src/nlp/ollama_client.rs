use crate::nlp::types::{Event, ParsedItem, Priority, Task};
use chrono::{DateTime, Datelike, Duration, Local, Months, NaiveDateTime, Utc};
use reqwest::{Client, Error as ReqwestError};
use serde::{Deserialize, Serialize};
use tokio::time::timeout;

const OLLAMA_BASE_URL: &str = "http://localhost:11434";
const OLLAMA_TIMEOUT_MS: u64 = 15000;
/// A first-ever model load took 17-26s, and hanging up mid-load aborts it, so it would restart from
/// zero on every attempt. A request that has to wait for the load gets this longer limit instead.
const OLLAMA_LOAD_TIMEOUT_MS: u64 = 90_000;

#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    stream: bool,
    format: String,
    /// Thinking models (qwen3.x, deepseek-r1) otherwise spend the output on a separate `thinking`
    /// field and leave `response` empty under `format: json`. Ignored by models without thinking.
    think: bool,
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
    deadline: Option<String>,
    duration_minutes: Option<i32>,
}

#[derive(Debug, Clone)]
pub struct OllamaClient {
    client: Client,
    model: String,
}

impl OllamaClient {
    #[must_use]
    pub fn new(model: Option<String>) -> Self {
        Self {
            client: Client::new(),
            model: model.unwrap_or_else(|| "qwen2.5:7b".to_string()),
        }
    }

    /// Asks the model to parse `input`.
    ///
    /// When the model is not resident, `wait_for_load` decides between waiting for the load (up to 90s)
    /// and failing at once with [`OllamaError::ModelNotLoaded`].
    ///
    /// # Errors
    ///
    /// Returns an error on timeout, a failed request, unusable model output, or an unloaded model
    /// when `wait_for_load` is false.
    pub async fn parse(&self, input: &str, wait_for_load: bool) -> Result<ParsedItem, OllamaError> {
        let prompt = Self::build_prompt(input);

        let request = OllamaRequest {
            model: self.model.clone(),
            prompt,
            stream: false,
            format: "json".to_string(),
            think: false,
        };

        let limit_ms = if self.is_loaded().await {
            OLLAMA_TIMEOUT_MS
        } else if wait_for_load {
            OLLAMA_LOAD_TIMEOUT_MS
        } else {
            return Err(OllamaError::ModelNotLoaded);
        };

        // Apply timeout to prevent hanging (use std::time::Duration for tokio)
        let response = timeout(
            std::time::Duration::from_millis(limit_ms),
            self.client
                .post(format!("{OLLAMA_BASE_URL}/api/generate"))
                .json(&request)
                .send(),
        )
        .await
        .map_err(|_| OllamaError::Timeout)?
        .map_err(OllamaError::Request)?;

        let ollama_response: OllamaResponse =
            response.json().await.map_err(OllamaError::Request)?;

        Self::parse_response(&ollama_response.response)
    }

    /// Builds the prompt. Its examples must hold real dates: a model copies any placeholder verbatim.
    #[must_use]
    pub fn build_prompt(input: &str) -> String {
        let now = chrono::Local::now();
        let today = now.format("%Y-%m-%d").to_string();
        let tomorrow = (now + Duration::days(1)).format("%Y-%m-%d").to_string();
        let end_of_next_month = now
            .date_naive()
            .with_day(1)
            .and_then(|first| first.checked_add_months(Months::new(2)))
            .and_then(|first| first.pred_opt())
            .map_or_else(|| tomorrow.clone(), |d| d.format("%Y-%m-%d").to_string());

        format!(
            r#"Today is {today}. Parse the following natural language input into structured JSON.

CRITICAL TIME PARSING RULES:
- "4:12 PM" or "4:12 pm" → use 16:12:00 (afternoon)
- "4:12 AM" or "4:12 am" → use 04:12:00 (morning)  
- "12:00 PM" → use 12:00:00 (noon)
- "12:00 AM" → use 00:00:00 (midnight)
- Always output datetime in ISO 8601 local time, with no timezone or offset: YYYY-MM-DDTHH:MM:SS

Extract: type (task/event), title, datetime (ISO 8601 local time), tags (array), priority (low/medium/high/urgent), deadline (ISO 8601 local time, a hard due date/time distinct from datetime — e.g. "before the end of next month", "by the 15th"), duration_minutes (integer, how long the task itself takes, e.g. "3 hours" -> 180).
Omit "deadline"/"duration_minutes" (or use null) when the input doesn't mention them.

Examples:
Input: "Submit report tomorrow at 3pm #work"
Output: {{"type": "task", "title": "Submit report", "datetime": "{tomorrow}T15:00:00", "tags": ["work"], "priority": "medium", "deadline": null, "duration_minutes": null}}

Input: "Meeting at 4:12 PM #important"
Output: {{"type": "task", "title": "Meeting", "datetime": "{today}T16:12:00", "tags": ["important"], "priority": "medium", "deadline": null, "duration_minutes": null}}

Input: "Call John at 9:30 AM tomorrow"
Output: {{"type": "task", "title": "Call John", "datetime": "{tomorrow}T09:30:00", "tags": [], "priority": "medium", "deadline": null, "duration_minutes": null}}

Input: "Finish the proposal before the end of next month, should take 3 hours"
Output: {{"type": "task", "title": "Finish the proposal", "datetime": null, "tags": [], "priority": "medium", "deadline": "{end_of_next_month}T23:59:59", "duration_minutes": 180}}

Now parse: "{input}"
Output (ONLY valid JSON, no explanations):"#
        )
    }

    /// Turns the model's JSON output into a task or event.
    ///
    /// # Errors
    ///
    /// Returns [`OllamaError::ParseError`] for invalid JSON, an unknown type, or an event without a time.
    pub fn parse_response(response: &str) -> Result<ParsedItem, OllamaError> {
        let structured: StructuredOutput =
            serde_json::from_str(response).map_err(|e| OllamaError::ParseError(e.to_string()))?;

        let datetime = structured
            .datetime
            .as_deref()
            .and_then(|raw| parse_timestamp("datetime", raw));

        let priority = match structured.priority.as_deref() {
            Some("urgent") => Priority::Urgent,
            Some("high") => Priority::High,
            Some("low") => Priority::Low,
            _ => Priority::Medium,
        };

        let tags = structured.tags.unwrap_or_default();

        let deadline = structured
            .deadline
            .as_deref()
            .and_then(|raw| parse_timestamp("deadline", raw));

        match structured.item_type.as_str() {
            "task" => Ok(ParsedItem::Task(Task {
                title: structured.title,
                due_date: datetime,
                deadline,
                duration_minutes: structured.duration_minutes,
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

    /// Loads the model into memory without generating, so the first real parse skips the load.
    ///
    /// # Errors
    ///
    /// Returns an error if the load takes over 90s or the request fails.
    pub async fn warm(&self) -> Result<(), OllamaError> {
        // An empty prompt makes Ollama load the model and return immediately.
        let request = serde_json::json!({ "model": self.model, "prompt": "", "stream": false });
        timeout(
            std::time::Duration::from_millis(OLLAMA_LOAD_TIMEOUT_MS),
            self.client
                .post(format!("{OLLAMA_BASE_URL}/api/generate"))
                .json(&request)
                .send(),
        )
        .await
        .map_err(|_| OllamaError::Timeout)?
        .and_then(reqwest::Response::error_for_status)
        .map_err(OllamaError::Request)?;
        Ok(())
    }

    /// Whether the model is resident in memory now (`/api/ps`); any failure counts as not loaded.
    async fn is_loaded(&self) -> bool {
        #[derive(Deserialize)]
        struct Running {
            name: String,
        }
        #[derive(Deserialize)]
        struct Ps {
            models: Vec<Running>,
        }

        let Ok(response) = self
            .client
            .get(format!("{OLLAMA_BASE_URL}/api/ps"))
            .send()
            .await
        else {
            return false;
        };
        response
            .json::<Ps>()
            .await
            .is_ok_and(|ps| ps.models.iter().any(|m| m.name == self.model))
    }

    pub async fn health_check(&self) -> bool {
        self.client
            .get(format!("{OLLAMA_BASE_URL}/api/tags"))
            .send()
            .await
            .is_ok()
    }
}

/// Reads an LLM timestamp, taking one without a UTC offset as local time.
///
/// Models often omit the offset the prompt asks for, which RFC 3339 rejects. Anything unparseable is
/// dropped with a warning that names the field but never the value, which may echo the input.
pub fn parse_timestamp(field: &str, raw: &str) -> Option<DateTime<Utc>> {
    let parsed = DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()?
                .and_local_timezone(Local)
                .earliest()
                .map(|dt| dt.with_timezone(&Utc))
        });
    if parsed.is_none() {
        tracing::warn!(field, "dropped an unparseable timestamp from the LLM");
    }
    parsed
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum OllamaError {
    Timeout,
    ModelNotLoaded,
    Request(ReqwestError),
    ParseError(String),
    ServiceUnavailable,
}

impl std::fmt::Display for OllamaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => write!(f, "Ollama request timed out"),
            Self::ModelNotLoaded => write!(f, "Ollama model not loaded"),
            Self::Request(e) => write!(f, "Request error: {e}"),
            Self::ParseError(e) => write!(f, "Parse error: {e}"),
            Self::ServiceUnavailable => write!(f, "Ollama service unavailable"),
        }
    }
}

impl std::error::Error for OllamaError {}
