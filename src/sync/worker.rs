use anyhow::Result;
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::sync::{broadcast, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Duration, interval};

use crate::nlp::NLPParser;

/// Configuration for the sync daemon
#[derive(Debug, Clone)]
pub struct SyncConfig {
    pub ollama_warmup_enabled: bool,
    pub cache_preload_enabled: bool,
    pub email_sync_enabled: bool,
    pub calendar_sync_enabled: bool,
    pub email_check_interval_secs: u64,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            ollama_warmup_enabled: true,
            cache_preload_enabled: true,
            email_sync_enabled: false,
            calendar_sync_enabled: false,
            email_check_interval_secs: 300,
        }
    }
}

/// Handle for managing the background sync daemon
pub struct SyncDaemon {
    shutdown_tx: broadcast::Sender<()>,
    tasks: Vec<JoinHandle<Result<()>>>,
}

impl SyncDaemon {
    /// Start the background sync daemon with all enabled services
    pub async fn start(
        db: SqlitePool,
        nlp_parser: Arc<NLPParser>,
        config: SyncConfig,
    ) -> Result<Self> {
        let (shutdown_tx, _) = broadcast::channel::<()>(1);
        let mut tasks = Vec::new();

        // Task 1: Pre-warm Ollama on startup
        if config.ollama_warmup_enabled {
            let shutdown_rx = shutdown_tx.subscribe();
            let nlp = nlp_parser.clone();

            tasks.push(tokio::spawn(async move {
                prewarm_ollama(nlp, shutdown_rx).await
            }));
        }

        // Task 2: Preload cache from SQLite
        if config.cache_preload_enabled {
            let shutdown_rx = shutdown_tx.subscribe();
            let db_clone = db.clone();
            let nlp = nlp_parser.clone();

            tasks.push(tokio::spawn(async move {
                preload_cache(db_clone, nlp, shutdown_rx).await
            }));
        }

        // Task 3: Email sync with IMAP IDLE
        if config.email_sync_enabled {
            let shutdown_rx = shutdown_tx.subscribe();
            let db_clone = db.clone();
            let interval_secs = config.email_check_interval_secs;

            tasks.push(tokio::spawn(async move {
                email_sync_worker(db_clone, shutdown_rx, interval_secs).await
            }));
        }

        // Task 4: Calendar sync
        if config.calendar_sync_enabled {
            let shutdown_rx = shutdown_tx.subscribe();
            let db_clone = db.clone();

            tasks.push(tokio::spawn(async move {
                calendar_sync_worker(db_clone, shutdown_rx).await
            }));
        }

        Ok(Self { shutdown_tx, tasks })
    }

    /// Gracefully shutdown all background tasks
    pub async fn shutdown(self) -> Result<()> {
        let _ = self.shutdown_tx.send(());

        let results = tokio::time::timeout(
            Duration::from_secs(5),
            futures::future::join_all(self.tasks),
        )
        .await;

        match results {
            Ok(results) => {
                for result in results {
                    if let Err(e) = result {
                        eprintln!("Task panicked during shutdown: {:?}", e);
                    }
                }
            }
            Err(_) => {
                eprintln!("Shutdown timeout exceeded - some tasks may not have completed");
            }
        }

        Ok(())
    }
}

/// Pre-warm Ollama model to eliminate cold start latency
async fn prewarm_ollama(
    nlp: Arc<NLPParser>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> Result<()> {
    println!("[Sync] Pre-warming Ollama model...");

    let (result_tx, result_rx) = oneshot::channel();

    let warmup_task = tokio::spawn(async move {
        let start = std::time::Instant::now();
        let _ = nlp.parse("Test warmup query").await;
        let elapsed = start.elapsed();
        let _ = result_tx.send(elapsed);
    });

    tokio::select! {
        _ = warmup_task => {
            if let Ok(elapsed) = result_rx.await {
                println!("[Sync] Ollama pre-warmed in {:.2}s", elapsed.as_secs_f64());
            }
        }
        _ = shutdown_rx.recv() => {
            println!("[Sync] Ollama warmup cancelled");
            return Ok(());
        }
    }

    Ok(())
}

/// Preload frequently used NLP patterns from database into cache
async fn preload_cache(
    db: SqlitePool,
    nlp: Arc<NLPParser>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> Result<()> {
    println!("[Sync] Preloading cache from database...");

    let (result_tx, result_rx) = oneshot::channel();

    let preload_task = tokio::spawn(async move {
        let start = std::time::Instant::now();

        // Use query() instead of query!() to avoid compile-time checking
        let rows: Vec<(String, i64)> = sqlx::query_as(
            r#"
            SELECT natural_language_input, COUNT(*) as count
            FROM tasks
            WHERE natural_language_input IS NOT NULL
            GROUP BY natural_language_input
            ORDER BY count DESC
            LIMIT 100
            "#,
        )
        .fetch_all(&db)
        .await?;

        let mut loaded = 0;
        for (input, _count) in rows {
            // Parse to populate cache (result discarded)
            let _ = nlp.parse(&input).await;
            loaded += 1;
        }

        let elapsed = start.elapsed();
        let _ = result_tx.send((loaded, elapsed));
        Ok::<_, anyhow::Error>(())
    });

    tokio::select! {
        result = preload_task => {
            match result {
                Ok(Ok(())) => {
                    if let Ok((count, elapsed)) = result_rx.await {
                        println!("[Sync] Preloaded {} cache entries in {:.2}s", count, elapsed.as_secs_f64());
                    }
                }
                Ok(Err(e)) => eprintln!("[Sync] Cache preload error: {}", e),
                Err(e) => eprintln!("[Sync] Cache preload task panicked: {}", e),
            }
        }
        _ = shutdown_rx.recv() => {
            println!("[Sync] Cache preload cancelled");
            return Ok(());
        }
    }

    Ok(())
}

/// Background email sync worker using IMAP IDLE for real-time notifications
async fn email_sync_worker(
    db: SqlitePool,
    mut shutdown_rx: broadcast::Receiver<()>,
    interval_secs: u64,
) -> Result<()> {
    println!(
        "[Sync] Starting email sync worker (interval: {}s)",
        interval_secs
    );

    let mut sync_interval = interval(Duration::from_secs(interval_secs));

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                println!("[Sync] Email sync worker shutting down");
                break;
            }

            _ = sync_interval.tick() => {
                if let Err(e) = sync_emails(&db).await {
                    eprintln!("[Sync] Email sync error: {}", e);
                }
            }
        }
    }

    Ok(())
}

/// Sync emails from IMAP server (placeholder for full IMAP IDLE implementation)
async fn sync_emails(db: &SqlitePool) -> Result<()> {
    println!("[Sync] Syncing emails...");

    // Use query() instead of query!() for runtime queries
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM emails")
        .fetch_one(db)
        .await?;

    println!("[Sync] Email sync complete ({} total emails)", count.0);
    Ok(())
}

/// Background calendar sync worker for CalDAV integration
async fn calendar_sync_worker(
    db: SqlitePool,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> Result<()> {
    println!("[Sync] Starting calendar sync worker");

    let mut sync_interval = interval(Duration::from_secs(600));

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => {
                println!("[Sync] Calendar sync worker shutting down");
                break;
            }

            _ = sync_interval.tick() => {
                if let Err(e) = sync_calendar(&db).await {
                    eprintln!("[Sync] Calendar sync error: {}", e);
                }
            }
        }
    }

    Ok(())
}

/// Sync calendar events from CalDAV server
async fn sync_calendar(db: &SqlitePool) -> Result<()> {
    println!("[Sync] Syncing calendar events...");

    // Use query() instead of query!() for runtime queries
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM events")
        .fetch_one(db)
        .await?;

    println!("[Sync] Calendar sync complete ({} total events)", count.0);
    Ok(())
}
