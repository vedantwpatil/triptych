use std::env;

/// IMAP app-password config for a single account, read from environment variables.
#[derive(Clone)]
pub struct EmailConfig {
    /// Label distinguishing this account's stored mail from others (e.g. `"default"`,
    /// `"work"`). Never empty.
    pub account: String,
    pub imap_server: String,
    pub imap_port: u16,
    pub imap_username: String,
    pub imap_password: String,
    pub imap_folder: String,
}

// Manual `Debug` so the app password never reaches a log line or panic message.
impl std::fmt::Debug for EmailConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmailConfig")
            .field("account", &self.account)
            .field("imap_server", &self.imap_server)
            .field("imap_port", &self.imap_port)
            .field("imap_username", &self.imap_username)
            .field("imap_password", &"<redacted>")
            .field("imap_folder", &self.imap_folder)
            .finish()
    }
}

impl EmailConfig {
    /// All configured accounts. Empty unless `TRIPTYCH_EMAIL_ENABLED=true`.
    ///
    /// Multi-account: set `IMAP_ACCOUNTS=work,personal` (comma-separated labels),
    /// then per-account vars suffixed `_<LABEL>` (label upper-cased, non-alphanumeric
    /// characters replaced with `_`) — e.g. `IMAP_SERVER_WORK`, `IMAP_USERNAME_WORK`,
    /// `IMAP_PASSWORD_WORK`, `IMAP_PORT_WORK`, `IMAP_FOLDER_WORK`.
    ///
    /// If `IMAP_ACCOUNTS` is unset, falls back to the legacy flat `IMAP_SERVER` /
    /// `IMAP_USERNAME` / `IMAP_PASSWORD` / `IMAP_PORT` / `IMAP_FOLDER` vars as a
    /// single account labeled `"default"`, so existing single-account `.env` files
    /// keep working unmodified.
    #[must_use]
    pub fn all_from_env() -> Vec<Self> {
        let enabled =
            env::var("TRIPTYCH_EMAIL_ENABLED").is_ok_and(|v| v.eq_ignore_ascii_case("true"));

        if !enabled {
            return Vec::new();
        }

        env::var("IMAP_ACCOUNTS").map_or_else(
            |_| Self::from_legacy_env().into_iter().collect(),
            |raw| {
                parse_account_labels(&raw)
                    .into_iter()
                    .filter_map(|label| Self::from_suffixed_env(&label))
                    .collect()
            },
        )
    }

    fn from_legacy_env() -> Option<Self> {
        let imap_server = env::var("IMAP_SERVER").ok()?;
        let imap_username = env::var("IMAP_USERNAME").ok()?;
        let imap_password = env::var("IMAP_PASSWORD").ok()?;
        let imap_port = env::var("IMAP_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(993);
        let imap_folder = env::var("IMAP_FOLDER").unwrap_or_else(|_| "INBOX".to_string());

        Some(Self {
            account: "default".to_string(),
            imap_server,
            imap_port,
            imap_username,
            imap_password,
            imap_folder,
        })
    }

    fn from_suffixed_env(label: &str) -> Option<Self> {
        let suffix = env_suffix(label);
        let imap_server = env::var(format!("IMAP_SERVER{suffix}")).ok()?;
        let imap_username = env::var(format!("IMAP_USERNAME{suffix}")).ok()?;
        let imap_password = env::var(format!("IMAP_PASSWORD{suffix}")).ok()?;
        let imap_port = env::var(format!("IMAP_PORT{suffix}"))
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(993);
        let imap_folder =
            env::var(format!("IMAP_FOLDER{suffix}")).unwrap_or_else(|_| "INBOX".to_string());

        Some(Self {
            account: label.to_string(),
            imap_server,
            imap_port,
            imap_username,
            imap_password,
            imap_folder,
        })
    }
}

/// Turns an account label into the upper-cased, underscore-delimited suffix used to
/// look up its env vars, e.g. `"work"` -> `"_WORK"`, `"my-personal"` -> `"_MY_PERSONAL"`.
#[must_use]
pub fn env_suffix(label: &str) -> String {
    let normalized: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("_{normalized}")
}

pub fn parse_account_labels(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}
