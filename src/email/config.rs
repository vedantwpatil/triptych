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
    /// Destination folder for `archive` (move-out-of-inbox). Defaults to `"Archive"`; not
    /// auto-created on the server if missing.
    pub archive_folder: String,
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
            .field("archive_folder", &self.archive_folder)
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
        let archive_folder =
            env::var("IMAP_ARCHIVE_FOLDER").unwrap_or_else(|_| "Archive".to_string());

        Some(Self {
            account: "default".to_string(),
            imap_server,
            imap_port,
            imap_username,
            imap_password,
            imap_folder,
            archive_folder,
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
        let archive_folder = env::var(format!("IMAP_ARCHIVE_FOLDER{suffix}"))
            .unwrap_or_else(|_| "Archive".to_string());

        Some(Self {
            account: label.to_string(),
            imap_server,
            imap_port,
            imap_username,
            imap_password,
            imap_folder,
            archive_folder,
        })
    }

    /// The configured account matching `account`, if any (used to reach the right server for a
    /// stored message's own account when acting on it, e.g. deleting it).
    #[must_use]
    pub fn for_account(account: &str) -> Option<Self> {
        Self::all_from_env().into_iter().find(|c| c.account == account)
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

/// SMTP app-password config for a single account, read from environment variables.
///
/// Paired with an [`EmailConfig`] by `account` label; a label with no SMTP vars set simply has no
/// send capability (compose/reply/forward look up a matching `SmtpConfig` and error if none
/// exists).
#[derive(Clone)]
pub struct SmtpConfig {
    pub account: String,
    pub smtp_server: String,
    pub smtp_port: u16,
    pub smtp_username: String,
    pub smtp_password: String,
    /// Envelope/header From address. Always the authenticated username: most providers reject
    /// (or silently rewrite) a From that doesn't match the login, so there's no separate
    /// `SMTP_FROM` override.
    pub from_addr: String,
}

impl std::fmt::Debug for SmtpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpConfig")
            .field("account", &self.account)
            .field("smtp_server", &self.smtp_server)
            .field("smtp_port", &self.smtp_port)
            .field("smtp_username", &self.smtp_username)
            .field("smtp_password", &"<redacted>")
            .field("from_addr", &self.from_addr)
            .finish()
    }
}

impl SmtpConfig {
    /// All configured accounts with send capability. Empty unless `TRIPTYCH_EMAIL_ENABLED=true`.
    /// Mirrors [`EmailConfig::all_from_env`]'s multi-account scheme (`IMAP_ACCOUNTS` labels,
    /// `SMTP_*_<LABEL>` suffixed vars), so the same account label configured for IMAP can also
    /// carry SMTP vars.
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

    /// The configured account matching `account`, if any (used when composing/replying from a
    /// specific mailbox).
    #[must_use]
    pub fn for_account(account: &str) -> Option<Self> {
        Self::all_from_env().into_iter().find(|c| c.account == account)
    }

    fn from_legacy_env() -> Option<Self> {
        let smtp_server = env::var("SMTP_SERVER").ok()?;
        let smtp_username = env::var("SMTP_USERNAME").ok()?;
        let smtp_password = env::var("SMTP_PASSWORD").ok()?;
        let smtp_port = env::var("SMTP_PORT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(587);

        Some(Self {
            account: "default".to_string(),
            smtp_server,
            smtp_port,
            from_addr: smtp_username.clone(),
            smtp_username,
            smtp_password,
        })
    }

    fn from_suffixed_env(label: &str) -> Option<Self> {
        let suffix = env_suffix(label);
        let smtp_server = env::var(format!("SMTP_SERVER{suffix}")).ok()?;
        let smtp_username = env::var(format!("SMTP_USERNAME{suffix}")).ok()?;
        let smtp_password = env::var(format!("SMTP_PASSWORD{suffix}")).ok()?;
        let smtp_port = env::var(format!("SMTP_PORT{suffix}"))
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(587);

        Some(Self {
            account: label.to_string(),
            smtp_server,
            smtp_port,
            from_addr: smtp_username.clone(),
            smtp_username,
            smtp_password,
        })
    }
}
