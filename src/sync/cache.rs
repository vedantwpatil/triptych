use crate::nlp::NLPParser;
use anyhow::Result;
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Preload frequently used NLP patterns from database into cache
pub async fn preload_cache(
    db: SqlitePool,
    nlp: Arc<NLPParser>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> Result<()> {
    let preload_task = tokio::spawn(async move {
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

        for (input, _count) in rows {
            let _ = nlp.parse(&input).await;
        }

        Ok::<_, anyhow::Error>(())
    });

    tokio::select! {
        result = preload_task => {
            if let Err(e) = result {
                // Task panicked - silently ignore during TUI mode
                let _ = e;
            }
        }
        _ = shutdown_rx.recv() => {
            return Ok(());
        }
    }

    Ok(())
}
