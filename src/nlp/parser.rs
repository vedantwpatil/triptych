use crate::nlp::ollama_client::OllamaClient;
use crate::nlp::rules::RuleParser;
use crate::nlp::types::{ParseResult, ParseStrategy, ParsedItem};
use lru::LruCache;
use std::num::NonZeroUsize;
use std::time::{Duration, Instant};
use strsim::jaro_winkler;
use tokio::sync::Mutex;

/// Cache entries older than this are treated as misses, so parsing behavior
/// (e.g. relative dates like "tomorrow") doesn't go stale across long sessions.
const CACHE_TTL: Duration = Duration::from_secs(3600);

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

impl CachedParse {
    fn is_expired(&self) -> bool {
        self.cached_at.elapsed() > CACHE_TTL
    }
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
            cache.get(input).cloned().filter(|c| !c.is_expired())
        };

        if let Some(cached) = cache_hit {
            let elapsed = start.elapsed().as_millis() as u64;
            eprintln!("» Exact cache hit (originally {:?})!", cached.strategy);
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
                    if cached_parse.is_expired() {
                        return None;
                    }
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
                "≈ Similar pattern found ({:.0}% match): \"{}\"",
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

        // Layer 1: Try regex fast path. If it resolves everything (including any
        // deadline intent), return immediately. Otherwise keep it as a fallback
        // and still try Ollama, so a phrase like "before the end of next month"
        // isn't silently accepted with its deadline dropped.
        let rule_fallback = RuleParser::try_parse(input);
        if let Some(item) = &rule_fallback {
            let resolved_deadline = matches!(item, ParsedItem::Task(t) if t.deadline.is_some());
            if !crate::nlp::rules::has_unresolved_deadline_intent(input, resolved_deadline) {
                let elapsed = start.elapsed().as_millis() as u64;
                let item = item.clone();

                let result = ParseResult {
                    item: item.clone(),
                    strategy: ParseStrategy::Regex,
                    confidence: 0.95,
                    parse_time_ms: elapsed,
                };

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
                drop(cache);

                return Ok(result);
            }
        }

        // Layer 2: Try Ollama for complex parsing (including deadline phrases the
        // regex fast path couldn't resolve)
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

        // Layer 2.5: Ollama unavailable/failed but the regex parser did produce
        // something usable (just without a fully-resolved deadline) - use that
        // rather than discarding it entirely for the empty Layer 3 fallback.
        if let Some(item) = rule_fallback {
            let elapsed = start.elapsed().as_millis() as u64;
            return Ok(ParseResult {
                item,
                strategy: ParseStrategy::Regex,
                confidence: 0.7,
                parse_time_ms: elapsed,
            });
        }

        // Layer 3: Fallback
        let elapsed = start.elapsed().as_millis() as u64;

        let item = ParsedItem::Task(crate::nlp::types::Task {
            title: input.to_string(),
            due_date: None,
            deadline: None,
            duration_minutes: None,
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
