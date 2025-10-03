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
