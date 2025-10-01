use crate::nlp::ollama_client::OllamaClient;
use crate::nlp::regex_patterns::RegexParser;
use crate::nlp::types::{ParseResult, ParseStrategy, ParsedItem};
use std::time::Instant;

pub struct NLPParser {
    ollama_client: OllamaClient,
    ollama_available: bool,
}

impl NLPParser {
    pub async fn new() -> Self {
        let ollama_client = OllamaClient::new(None);
        let ollama_available = ollama_client.health_check().await;

        if !ollama_available {
            eprintln!("Warning: Ollama service not available. Falling back to regex-only parsing.");
        }

        Self {
            ollama_client,
            ollama_available,
        }
    }

    pub async fn parse(&self, input: &str) -> Result<ParseResult, ParseError> {
        let start = Instant::now();

        // Layer 1: Try regex fast path
        if let Some(item) = RegexParser::try_parse(input) {
            let elapsed = start.elapsed().as_millis() as u64;
            return Ok(ParseResult {
                item,
                strategy: ParseStrategy::Regex,
                confidence: 0.95,
                parse_time_ms: elapsed,
            });
        }

        // Layer 2: Try Ollama for complex parsing
        if self.ollama_available {
            match self.ollama_client.parse(input).await {
                Ok(item) => {
                    let elapsed = start.elapsed().as_millis() as u64;
                    return Ok(ParseResult {
                        item,
                        strategy: ParseStrategy::Ollama,
                        confidence: 0.85,
                        parse_time_ms: elapsed,
                    });
                }
                Err(e) => {
                    eprintln!("Ollama parsing failed: {}. Falling back.", e);
                }
            }
        }

        // Layer 3: Fallback - create basic task
        let elapsed = start.elapsed().as_millis() as u64;
        Ok(ParseResult {
            item: ParsedItem::Task(crate::nlp::types::Task {
                title: input.to_string(),
                due_date: None,
                tags: vec![],
                priority: crate::nlp::types::Priority::Medium,
                is_scheduled: false,
            }),
            strategy: ParseStrategy::Fallback,
            confidence: 0.50,
            parse_time_ms: elapsed,
        })
    }

    pub fn is_ollama_available(&self) -> bool {
        self.ollama_available
    }
}

#[derive(Debug)]
#[allow(dead_code)]
pub enum ParseError {
    InvalidInput(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::InvalidInput(msg) => write!(f, "Invalid input: {}", msg),
        }
    }
}

impl std::error::Error for ParseError {}
