/// Configuration for the sync daemon
#[derive(Debug, Clone)]
pub struct SyncConfig {
    pub ollama_warmup_enabled: bool,
    pub cache_preload_enabled: bool,
    pub calendar_sync_enabled: bool,
    pub mail_sync_enabled: bool,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            ollama_warmup_enabled: true,
            cache_preload_enabled: true,
            calendar_sync_enabled: false,
            mail_sync_enabled: false,
        }
    }
}

impl SyncConfig {
    pub fn from_env() -> Self {
        let mail_sync_enabled = std::env::var("TRIPTYCH_EMAIL_ENABLED")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        Self {
            ollama_warmup_enabled: true,
            cache_preload_enabled: true,
            calendar_sync_enabled: false,
            mail_sync_enabled,
        }
    }
}
