use crate::nlp::ollama_client::OllamaClient;
use crate::nlp::regex_patterns::RegexParser;
use crate::nlp::types::{ParseResult, ParseStrategy, ParsedItem};
use lru::LruCache;
use std::num::NonZeroUsize;
use std::time::Instant;
use strsim::jaro_winkler;
use tokio::sync::Mutex;

pub struct NLPParser {
    ollama_client: OllamaClient,
    ollama_available: bool,
    cache: Mutex<LruCache<String, CachedParse>>,
}

#[derive(Clone)]
struct CachedParse {
    item: ParsedItem,
    strategy: ParseStrategy,
    confidence: f32,
    cached_at: Instant,
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
            cache: Mutex::new(LruCache::new(NonZeroUsize::new(1000).unwrap())),
        }
    }

    pub async fn parse(&self, input: &str) -> Result<ParseResult, ParseError> {
        let start = Instant::now();

        // Layer 0: Check exact cache match first (hold lock briefly)
        let cache_hit = {
            let mut cache = self.cache.lock().await;
            cache.get(input).cloned() // Clone while lock is held
        };

        if let Some(cached) = cache_hit {
            let elapsed = start.elapsed().as_millis() as u64;
            eprintln!("⚡ Exact cache hit!"); // Changed to eprintln!
            return Ok(ParseResult {
                item: cached.item,
                strategy: ParseStrategy::Cached,
                confidence: cached.confidence,
                parse_time_ms: elapsed,
            });
        }

        // Layer 0.5: Check similar inputs via fuzzy matching (optimized)
        let similarity_threshold = 0.85;
        let fuzzy_match = {
            let cache = self.cache.lock().await;

            // Early exit optimization: don't check if input is very short
            if input.len() < 3 {
                None
            } else {
                cache.iter().find_map(|(cached_input, cached_parse)| {
                    let similarity = jaro_winkler(input, cached_input);
                    if similarity > similarity_threshold {
                        Some((cached_input.clone(), cached_parse.clone(), similarity))
                    } else {
                        None
                    }
                })
            }
        };

        if let Some((matched_input, cached_parse, similarity)) = fuzzy_match {
            let elapsed = start.elapsed().as_millis() as u64;
            eprintln!(
                "🔍 Similar pattern found ({:.0}% match): \"{}\"",
                similarity * 100.0,
                matched_input
            );

            let adjusted_confidence = cached_parse.confidence * similarity as f32;
            return Ok(ParseResult {
                item: cached_parse.item,
                strategy: ParseStrategy::Cached,
                confidence: adjusted_confidence,
                parse_time_ms: elapsed,
            });
        }

        // Layer 1: Try regex fast path
        if let Some(item) = RegexParser::try_parse(input) {
            let elapsed = start.elapsed().as_millis() as u64;

            let result = ParseResult {
                item: item.clone(),
                strategy: ParseStrategy::Regex,
                confidence: 0.95,
                parse_time_ms: elapsed,
            };

            // Cache result
            {
                let mut cache = self.cache.lock().await;
                cache.put(
                    input.to_string(),
                    CachedParse {
                        item,
                        strategy: ParseStrategy::Regex,
                        confidence: 0.95,
                        cached_at: Instant::now(),
                    },
                );
            }

            return Ok(result);
        }

        // Layer 2: Try Ollama for complex parsing
        if self.ollama_available {
            match self.ollama_client.parse(input).await {
                Ok(item) => {
                    let elapsed = start.elapsed().as_millis() as u64;

                    let result = ParseResult {
                        item: item.clone(),
                        strategy: ParseStrategy::Ollama,
                        confidence: 0.85,
                        parse_time_ms: elapsed,
                    };

                    // Cache Ollama results
                    {
                        let mut cache = self.cache.lock().await;
                        cache.put(
                            input.to_string(),
                            CachedParse {
                                item,
                                strategy: ParseStrategy::Ollama,
                                confidence: 0.85,
                                cached_at: Instant::now(),
                            },
                        );
                    }

                    return Ok(result);
                }
                Err(e) => {
                    eprintln!("Ollama parsing failed: {}. Falling back.", e);
                }
            }
        }

        // Layer 3: Fallback
        let elapsed = start.elapsed().as_millis() as u64;

        let item = ParsedItem::Task(crate::nlp::types::Task {
            title: input.to_string(),
            due_date: None,
            tags: vec![],
            priority: crate::nlp::types::Priority::Medium,
            is_scheduled: false,
        });

        let result = ParseResult {
            item: item.clone(),
            strategy: ParseStrategy::Fallback,
            confidence: 0.50,
            parse_time_ms: elapsed,
        };

        // Cache fallback results
        {
            let mut cache = self.cache.lock().await;
            cache.put(
                input.to_string(),
                CachedParse {
                    item,
                    strategy: ParseStrategy::Fallback,
                    confidence: 0.50,
                    cached_at: Instant::now(),
                },
            );
        }

        Ok(result)
    }

    pub fn is_ollama_available(&self) -> bool {
        self.ollama_available
    }

    pub async fn cache_stats(&self) -> (usize, usize) {
        let cache = self.cache.lock().await;
        (cache.len(), cache.cap().get())
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
