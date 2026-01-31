use crate::nlp::NLPParser;
use anyhow::Result;
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::task::JoinHandle;
use tokio::time::Duration;

use super::config::SyncConfig;
use super::{cache, calendar, ollama};

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

        // Pre-warm Ollama on startup
        if config.ollama_warmup_enabled {
            let shutdown_rx = shutdown_tx.subscribe();
            let nlp = nlp_parser.clone();

            tasks.push(tokio::spawn(async move {
                ollama::prewarm_ollama(nlp, shutdown_rx).await
            }));
        }

        // Preload cache from SQLite
        if config.cache_preload_enabled {
            let shutdown_rx = shutdown_tx.subscribe();
            let db_clone = db.clone();
            let nlp = nlp_parser.clone();

            tasks.push(tokio::spawn(async move {
                cache::preload_cache(db_clone, nlp, shutdown_rx).await
            }));
        }

        // Calendar sync
        // Not yet finished
        if config.calendar_sync_enabled {
            let shutdown_rx = shutdown_tx.subscribe();
            let db_clone = db.clone();

            tasks.push(tokio::spawn(async move {
                calendar::calendar_sync_worker(db_clone, shutdown_rx).await
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
