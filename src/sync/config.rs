/// Configuration for the sync daemon
#[derive(Debug, Clone)]
pub struct SyncConfig {
    pub ollama_warmup_enabled: bool,
    pub cache_preload_enabled: bool,
    pub calendar_sync_enabled: bool,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            ollama_warmup_enabled: true,
            cache_preload_enabled: true,
            calendar_sync_enabled: false,
        }
    }
}

impl SyncConfig {
    pub fn from_env() -> Self {
        Self {
            ollama_warmup_enabled: true,
            cache_preload_enabled: true,
            calendar_sync_enabled: false,
        }
    }
}
