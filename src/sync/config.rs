/// Configuration for the sync daemon
#[derive(Debug, Clone)]
pub struct SyncConfig {
    pub ollama_warmup_enabled: bool,
    pub cache_preload_enabled: bool,
    pub email_sync_enabled: bool,
    pub imap_config: Option<ImapConfig>,
    pub calendar_sync_enabled: bool,
    pub email_check_interval_secs: u64,
}

#[derive(Debug, Clone)]
pub struct ImapConfig {
    pub server: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub folder: String,
}

impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            ollama_warmup_enabled: true,
            cache_preload_enabled: true,
            email_sync_enabled: false,
            calendar_sync_enabled: false,
            email_check_interval_secs: 300,
            imap_config: None,
        }
    }
}

impl SyncConfig {
    pub fn from_env() -> Self {
        let email_enabled = std::env::var("TRIPTYCH_EMAIL_ENABLED").unwrap_or_default() == "true";

        let imap_config = if email_enabled {
            Some(ImapConfig {
                server: std::env::var("IMAP_SERVER")
                    .unwrap_or_else(|_| "imap.gmail.com".to_string()),
                port: std::env::var("IMAP_PORT")
                    .unwrap_or_else(|_| "993".to_string())
                    .parse()
                    .unwrap_or(993),
                username: std::env::var("IMAP_USERNAME")
                    .expect("IMAP_USERNAME must be set when email sync is enabled"),
                password: std::env::var("IMAP_PASSWORD")
                    .expect("IMAP_PASSWORD must be set when email sync is enabled"),
                folder: std::env::var("IMAP_FOLDER").unwrap_or_else(|_| "INBOX".to_string()),
            })
        } else {
            None
        };

        Self {
            ollama_warmup_enabled: true,
            cache_preload_enabled: true,
            email_sync_enabled: email_enabled,
            calendar_sync_enabled: false,
            email_check_interval_secs: 300,
            imap_config,
        }
    }
}
