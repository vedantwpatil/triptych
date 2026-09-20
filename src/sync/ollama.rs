use crate::nlp::NLPParser;
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Load the Ollama model at startup so the first real parse isn't paying the model load
pub async fn prewarm_ollama(
    nlp: Arc<NLPParser>,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> Result<()> {
    tokio::select! {
        () = nlp.prewarm() => {}
        _ = shutdown_rx.recv() => {}
    }

    Ok(())
}
