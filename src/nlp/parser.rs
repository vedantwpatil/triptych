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

        // Layer 0: Check exact cache match first
        {
            let mut cache = self.cache.lock().await;
            if let Some(cached) = cache.get(input) {
                let elapsed = start.elapsed().as_millis() as u64;

                println!("⚡ Exact cache hit!");

                return Ok(ParseResult {
                    item: cached.item.clone(),
                    strategy: ParseStrategy::Cached,
                    confidence: cached.confidence,
                    parse_time_ms: elapsed,
                });
            }
        }

        // Layer 0.5: Check similar inputs via fuzzy matching
        let similarity_threshold = 0.85;
        {
            let cache = self.cache.lock().await;
            for (cached_input, cached_parse) in cache.iter() {
                let similarity = jaro_winkler(input, cached_input);

                if similarity > similarity_threshold {
                    let elapsed = start.elapsed().as_millis() as u64;

                    println!(
                        "🔍 Similar pattern found ({:.0}% match): \"{}\"",
                        similarity * 100.0,
                        cached_input
                    );

                    // Adjust confidence based on similarity
                    let adjusted_confidence = cached_parse.confidence * similarity as f32;

                    return Ok(ParseResult {
                        item: cached_parse.item.clone(),
                        strategy: ParseStrategy::Cached,
                        confidence: adjusted_confidence,
                        parse_time_ms: elapsed,
                    });
                }
            }
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

            // Cache regex results for future fuzzy matches
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
                    } // Lock released here

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
        } // Lock released here

        Ok(result)
    }

    pub fn is_ollama_available(&self) -> bool {
        self.ollama_available
    }

    // Cache statistics for debugging
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
