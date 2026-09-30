//! Task links: reading a task's stored URLs, the `o`/`O` actions, and handing a URL to the browser.

use std::process::Stdio;
use std::time::Instant;

use super::App;

/// Program run as `cmd <url>` instead of the system opener (`open` / `xdg-open`). Lets tests and
/// headless sandboxes record links rather than launch a browser.
pub const OPEN_CMD_ENV: &str = "TRIPTYCH_OPEN_CMD";

/// Whether `url` is an `http(s)` link. Only these are ever stored or opened.
#[must_use]
pub fn is_web_url(url: &str) -> bool {
    let url = url.to_ascii_lowercase();
    url.starts_with("https://") || url.starts_with("http://")
}

/// The URLs in a task's `links` column; empty for `None` or text that is not a JSON list.
#[must_use]
pub fn parse_links(json: Option<&str>) -> Vec<String> {
    json.and_then(|j| serde_json::from_str(j).ok())
        .unwrap_or_default()
}

/// A URL as shown in a list: no scheme or `www.`, cut to `max` characters with a trailing `…`.
#[must_use]
pub fn link_label(url: &str, max: usize) -> String {
    let bare = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let bare = bare.strip_prefix("www.").unwrap_or(bare);
    if bare.chars().count() <= max {
        return bare.to_string();
    }
    let kept: String = bare.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

/// Starts the opener for `url` and returns without waiting for it.
fn open_url(url: &str) -> std::io::Result<()> {
    if !is_web_url(url) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not an http(s) link",
        ));
    }
    let program = std::env::var(OPEN_CMD_ENV)
        .ok()
        .filter(|c| !c.trim().is_empty())
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "open"
            } else {
                "xdg-open"
            }
            .into()
        });
    tokio::process::Command::new(program)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(drop)
}

impl App {
    fn selected_links(&self) -> Vec<String> {
        self.tasks
            .get(self.selected)
            .map(|t| parse_links(t.links.as_deref()))
            .unwrap_or_default()
    }

    fn report(&mut self, message: String) {
        self.status_message = Some((message, Instant::now()));
    }

    fn open_link(&mut self, url: &str) {
        let message = match open_url(url) {
            Ok(()) => format!("Opened {}", link_label(url, 60)),
            Err(e) => format!("Error: cannot open link: {e}"),
        };
        self.report(message);
    }

    /// `o`: opens the selected task's first link (for Canvas, the assignment page).
    pub fn open_first_link(&mut self) {
        match self.selected_links().first() {
            Some(url) => self.open_link(&url.clone()),
            None => self.report("No links on this task".into()),
        }
    }

    /// `O`: lists the selected task's links in a popup; a single link opens directly.
    pub fn open_links_popup(&mut self) {
        let mut links = self.selected_links();
        match links.len() {
            0 => self.report("No links on this task".into()),
            1 => self.open_link(&links.remove(0)),
            _ => {
                self.links = links;
                self.selected_link = 0;
                self.links_open = true;
            }
        }
    }

    pub fn close_links_popup(&mut self) {
        self.links_open = false;
        self.links.clear();
    }

    /// Opens the highlighted link and closes the popup.
    pub fn open_selected_link(&mut self) {
        if let Some(url) = self.links.get(self.selected_link).cloned() {
            self.open_link(&url);
        }
        self.close_links_popup();
    }

    /// Opens link `number` (1-based, as the popup numbers them); an unused number does nothing.
    pub fn open_link_number(&mut self, number: usize) {
        if let Some(url) = number
            .checked_sub(1)
            .and_then(|i| self.links.get(i))
            .cloned()
        {
            self.open_link(&url);
            self.close_links_popup();
        }
    }
}
