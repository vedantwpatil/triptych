use std::env;

/// IMAP app-password config for a single account, read from environment variables.
#[derive(Debug, Clone)]
pub struct EmailConfig {
    pub imap_server: String,
    pub imap_port: u16,
    pub imap_username: String,
    pub imap_password: String,
    pub imap_folder: String,
}

impl EmailConfig {
    /// Returns `None` unless `TRIPTYCH_EMAIL_ENABLED=true` and all required vars are set.
    pub fn from_env() -> Option<Self> {
        let enabled = env::var("TRIPTYCH_EMAIL_ENABLED")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

        if !enabled {
            return None;
        }

        let imap_server = env::var("IMAP_SERVER").ok()?;
        let imap_username = env::var("IMAP_USERNAME").ok()?;
        let imap_password = env::var("IMAP_PASSWORD").ok()?;
        let imap_port = env::var("IMAP_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(993);
        let imap_folder = env::var("IMAP_FOLDER").unwrap_or_else(|_| "INBOX".to_string());

        Some(Self {
            imap_server,
            imap_port,
            imap_username,
            imap_password,
            imap_folder,
        })
    }
}
