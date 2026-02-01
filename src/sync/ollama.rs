use crate::nlp::NLPParser;
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Pre-warm Ollama model to eliminate cold start latency
pub async fn prewarm_ollama(
    nlp: Arc<NLPParser>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> Result<()> {
    let warmup_task = tokio::spawn(async move {
        let _ = nlp.parse("Test warmup query").await;
    });

    tokio::select! {
        _ = warmup_task => {}
        _ = shutdown_rx.recv() => {
            return Ok(());
        }
    }

    Ok(())
}
