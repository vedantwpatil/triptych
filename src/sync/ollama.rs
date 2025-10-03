use crate::nlp::NLPParser;
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::{broadcast, oneshot};

/// Pre-warm Ollama model to eliminate cold start latency
pub async fn prewarm_ollama(
    nlp: Arc<NLPParser>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> Result<()> {
    eprintln!("[Sync] Pre-warming Ollama model...");

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
                eprintln!("[Sync] Ollama pre-warmed in {:.2}s", elapsed.as_secs_f64());
            }
        }
        _ = shutdown_rx.recv() => {
            eprintln!("[Sync] Ollama warmup cancelled");
            return Ok(());
        }
    }

    Ok(())
}
