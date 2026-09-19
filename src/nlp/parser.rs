use crate::nlp::ollama_client::OllamaClient;
use crate::nlp::rules::RuleParser;
use crate::nlp::types::{ParseResult, ParseStrategy, ParsedItem};
use lru::LruCache;
use std::num::NonZeroUsize;
use std::time::{Duration, Instant};
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

/// Milliseconds since `start`, for `ParseResult::parse_time_ms`. A parse never
/// takes anywhere near `u64::MAX` ms, so the truncation is unreachable in practice.
fn elapsed_ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

impl NLPParser {
    pub async fn new() -> Self {
        let ollama_client = OllamaClient::new(None);
        let ollama_available = ollama_client.health_check().await;

        if !ollama_available {
            tracing::warn!("Ollama service not available; falling back to regex-only parsing");
        }

        Self {
            ollama_client,
            ollama_available,
            // 1000 is a non-zero literal, so this is never `None`.
            cache: Mutex::new(LruCache::new(NonZeroUsize::new(1000).unwrap_or(NonZeroUsize::MIN))),
        }
    }

    // A linear layered pipeline (Layer 0 through 3, see comments below) - splitting
    // it into helpers would scatter that sequence without reducing its complexity.
    #[allow(clippy::too_many_lines)]
    pub async fn parse(&self, input: &str) -> Result<ParseResult, ParseError> {
        let start = Instant::now();

        // Layer 0: Check exact cache match first (hold lock briefly)
        let cache_hit = {
            let mut cache = self.cache.lock().await;
            cache.get(input).cloned().filter(|c| !c.is_expired())
        };

        if let Some(cached) = cache_hit {
            let elapsed = elapsed_ms(start);
            tracing::debug!("exact cache hit (originally {:?})", cached.strategy);
            return Ok(ParseResult {
                item: cached.item,
                strategy: ParseStrategy::Cached,
                confidence: cached.confidence,
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
                let elapsed = elapsed_ms(start);
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
                    let elapsed = elapsed_ms(start);

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
                    tracing::warn!("Ollama parsing failed: {e}; falling back");
                }
            }
        }

        // Layer 2.5: Ollama unavailable/failed but the regex parser did produce
        // something usable (just without a fully-resolved deadline) - use that
        // rather than discarding it entirely for the empty Layer 3 fallback.
        if let Some(item) = rule_fallback {
            let elapsed = elapsed_ms(start);
            return Ok(ParseResult {
                item,
                strategy: ParseStrategy::Regex,
                confidence: 0.7,
                parse_time_ms: elapsed,
            });
        }

        // Layer 3: Fallback
        let elapsed = elapsed_ms(start);

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

    pub const fn is_ollama_available(&self) -> bool {
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
            Self::InvalidInput(msg) => write!(f, "Invalid input: {msg}"),
        }
    }
}

impl std::error::Error for ParseError {}
