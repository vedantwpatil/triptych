use crate::nlp::NLPParser;
use anyhow::Result;
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::sync::{broadcast, oneshot};

/// Preload frequently used NLP patterns from database into cache
pub async fn preload_cache(
    db: SqlitePool,
    nlp: Arc<NLPParser>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> Result<()> {
    eprintln!("[Sync] Preloading cache from database...");

    let (result_tx, result_rx) = oneshot::channel();

    let preload_task = tokio::spawn(async move {
        let start = std::time::Instant::now();

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
                        eprintln!("[Sync] Preloaded {} cache entries in {:.2}s", count, elapsed.as_secs_f64());
                    }
                }
                Ok(Err(e)) => eprintln!("[Sync] Cache preload error: {}", e),
                Err(e) => eprintln!("[Sync] Cache preload task panicked: {}", e),
            }
        }
        _ = shutdown_rx.recv() => {
            eprintln!("[Sync] Cache preload cancelled");
            return Ok(());
        }
    }

    Ok(())
}
