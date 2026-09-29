//! Email view: retention purge, account sync, refresh, read/open and convert-to-task.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};

use super::time::resolve_local_datetime;
use super::{App, ComposeField, ComposeState, InputMode, ViewMode};
use crate::email::{
    EmailConfig, EmailMessage, EmailRule, EmailSort, ImapMailSource, MailSource, SmtpConfig,
    drafts, message, priority, smtp, store as email_store,
    sync::{sync_account, sync_one_folder},
};

/// Email retention window for [`App::cleanup_old_emails`]. ~6 months, expressed
/// as days rather than calendar months to sidestep invalid-date edge cases
/// (e.g. Aug 31 minus 1 month = Feb 31) — the window isn't meant to be exact to
/// the day.
const EMAIL_RETENTION_DAYS: i64 = 180;

/// Outcome of a background mail sync, sent from the spawned task to `run_app` over `App::mail_rx`.
#[derive(Debug, Default)]
pub struct MailSync {
    /// Messages stored across all accounts.
    new: u64,
    /// One `account: reason` entry per failed account.
    errors: Vec<String>,
}

/// State of one email's AI summary in `App::email_summaries`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Summary {
    Pending,
    Ready(String),
    /// Why it failed; the next open of the email tries again.
    Failed(&'static str),
}

/// A finished background summary, sent over `App::summary_rx`.
#[derive(Debug)]
pub struct SummaryDone {
    email_id: i64,
    result: Result<String, &'static str>,
}

/// Outcome of a background triage pass over every not-yet-classified email, sent over
/// `App::triage_rx`.
#[derive(Debug, Default)]
pub struct TriageDone {
    /// `(email_id, focused)` for every email successfully classified this pass.
    results: Vec<(i64, bool)>,
}

/// Outcome of a background compose/reply/forward send, sent over `App::send_rx`.
#[derive(Debug)]
pub struct SendResult {
    result: Result<(), String>,
    /// Set when the sent compose was resumed from a saved draft, so a successful send can delete
    /// that now-stale `email_drafts` row.
    draft_id: Option<i64>,
}

/// Outcome of a background email delete, sent over `App::delete_rx`.
#[derive(Debug)]
pub struct DeleteResult {
    email_id: i64,
    result: Result<(), String>,
}

/// Outcome of a background email archive, sent over `App::archive_rx`.
#[derive(Debug)]
pub struct ArchiveResult {
    email_id: i64,
    result: Result<(), String>,
}

/// Outcome of a background attachment save, sent over `App::attachment_rx`: how many files were
/// written and where, or why none were.
#[derive(Debug)]
pub struct AttachmentSaveResult {
    result: Result<(usize, String), String>,
}

/// Outcome of a background folder-discovery LIST pass, sent over `App::folder_list_rx`.
#[derive(Debug, Default)]
pub struct FolderListResult {
    folders: Vec<(String, String)>,
    errors: Vec<String>,
}

/// Outcome of a background one-folder sync triggered by picking a discovered folder, sent over
/// `App::folder_sync_rx`.
#[derive(Debug)]
pub struct FolderSyncResult {
    folder: String,
    result: Result<u64, String>,
}

/// Where `save_selected_attachments` writes files: `TRIPTYCH_ATTACHMENT_DIR`, or a fixed default
/// next to the log/socket files this project already scatters under the system temp dir.
fn attachment_dir() -> std::path::PathBuf {
    std::env::var("TRIPTYCH_ATTACHMENT_DIR").map_or_else(
        |_| std::env::temp_dir().join("triptych-attachments"),
        std::path::PathBuf::from,
    )
}

/// The last path component of `name`, discarding any directory traversal.
///
/// A hostile `Content-Disposition: filename` could otherwise smuggle one in (e.g.
/// `../../etc/passwd`) — this value came off the wire, from a header the sender fully controls.
/// `None` for an empty or entirely-traversal name, so the caller falls back to a synthesized one.
#[must_use]
pub fn sanitize_filename(name: &str) -> Option<String> {
    std::path::Path::new(name)
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Emails with a shorter body than this are not worth summarizing.
const SUMMARY_MIN_CHARS: usize = 200;
/// Cap on the body text sent to the model, so one huge message cannot stall the prompt.
const SUMMARY_INPUT_CHARS: usize = 4000;
/// Cap on how many not-yet-classified emails one background triage pass classifies, so a large
/// backlog (e.g. after a first-ever sync) doesn't serialize dozens of Ollama round-trips at once —
/// the rest catch up on the next sync's pass. Lowered from 20: most rows resolve via
/// `bulk_mail_heuristic` with no Ollama call at all, so a smaller cap still clears a normal
/// backlog in one or two passes while capping worst-case GPU load per pass.
const TRIAGE_BATCH_LIMIT: i64 = 10;
/// Cap on how many not-yet-checked emails one `run_email_rules` pass checks against every saved
/// rule. Unlike `TRIAGE_BATCH_LIMIT`, this is pure local-DB work (no per-item Ollama/IMAP round
/// trip), so a larger batch per pass costs nothing extra.
const RULE_CHECK_LIMIT: i64 = 100;

impl App {
    /// Shows `msg` in the status line for a few seconds.
    pub(super) fn notify(&mut self, msg: &str) {
        self.status_message = Some((msg.to_string(), std::time::Instant::now()));
    }

    /// Purges emails older than [`EMAIL_RETENTION_DAYS`] (~6 months), run every
    /// time the Email view is entered — the only retention path there is, so
    /// without it `email_messages` grows unbounded. Runs inline, not spawned:
    /// unlike `sync_email_accounts`'s IMAP round-trip, a `DELETE` on the indexed
    /// `date_utc` column is a local disk write, not a network call, so there's no
    /// TUI-stall risk to avoid.
    pub(super) async fn cleanup_old_emails(&self) {
        let cutoff = Utc::now() - Duration::days(EMAIL_RETENTION_DAYS);
        if let Err(e) = email_store::delete_older_than(&self.db_pool, cutoff).await {
            tracing::warn!("[Email] cleanup failed: {}", e);
        }
    }

    /// Starts a background pull of new mail for every configured account: on entering the Email
    /// view (`manual` false, silent) and on `s` (`manual` true, reports the outcome). Spawned, never
    /// awaited: an unreachable IMAP server would otherwise freeze the TUI until every TCP+TLS
    /// attempt failed. The result comes back over `mail_rx` and is applied by `apply_mail_sync`.
    /// A manual request during a running sync waits for that sync and reports its result.
    pub fn start_email_sync(&mut self, manual: bool) {
        let configs = EmailConfig::all_from_env();
        if configs.is_empty() {
            if manual {
                self.notify("Email not configured (set TRIPTYCH_EMAIL_ENABLED and IMAP_* in .env)");
            }
            return;
        }
        if manual {
            self.mail_manual = true;
            self.notify("Syncing...");
        }
        if self.mail_syncing {
            return;
        }
        self.mail_syncing = true;

        let (db_pool, tx) = (self.db_pool.clone(), self.mail_tx.clone());
        tokio::spawn(async move {
            let mut done = MailSync::default();
            for config in &configs {
                match sync_account(&db_pool, config, true).await {
                    Ok(report) => done.new += report.new,
                    Err(e) => {
                        tracing::warn!("[Email] sync failed for '{}': {}", config.account, e);
                        done.errors.push(format!("{}: {e}", config.account));
                    }
                }
            }
            let _ = tx.send(done);
        });
    }

    /// Applies a finished background sync: reloads the list and, if `s` asked for it, says what happened.
    pub async fn apply_mail_sync(&mut self, done: MailSync) {
        self.mail_syncing = false;
        let manual = std::mem::take(&mut self.mail_manual);
        // A reload would blank the open popup's body (`get_recent` rows carry none).
        if self.view_mode == ViewMode::Email && !self.email_detail_open {
            let _ = self.refresh_emails().await;
        }
        self.run_email_triage();
        self.run_email_rules().await;
        if manual {
            let msg = match (done.new, done.errors.as_slice()) {
                (0, []) => "All emails gathered - nothing new".to_string(),
                (n, []) => format!("Synced {n} new email(s)"),
                (0, errors) => format!("Sync failed: {}", errors.join("; ")),
                (n, errors) => format!("Synced {n} new; failed: {}", errors.join("; ")),
            };
            self.notify(&msg);
        }
    }

    /// Reloads the newest stored emails into the Email view.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn refresh_emails(&mut self) -> Result<(), sqlx::Error> {
        let selected_id = self.emails.get(self.selected_email).map(|e| e.id);
        self.emails = email_store::get_recent(&self.db_pool, 100)
            .await
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        if let Some(account) = &self.account_filter {
            self.emails.retain(|e| &e.account == account);
        }
        if let Some(folder) = &self.folder_filter {
            self.emails.retain(|e| &e.folder == folder);
        }
        if let Some(focused) = self.focus_filter {
            self.emails.retain(|e| e.triage_focused == Some(focused));
        }
        if let Some(has_attachments) = self.attachment_filter {
            self.emails.retain(|e| e.has_attachments == has_attachments);
        }
        if let Some(unread) = self.unread_filter {
            self.emails.retain(|e| e.is_read != unread);
        }
        if let Some(starred) = self.starred_filter {
            self.emails.retain(|e| e.is_starred == starred);
        }
        if let Some(domain) = &self.domain_filter {
            self.emails
                .retain(|e| e.from_addr.rsplit('@').next() == Some(domain.as_str()));
        }
        let now = Utc::now();
        if self.show_snoozed {
            self.emails
                .retain(|e| e.snoozed_until.is_some_and(|t| t > now));
        } else {
            self.emails
                .retain(|e| e.snoozed_until.is_none_or(|t| t <= now));
        }
        if self.email_sort == EmailSort::Priority {
            // Stable, so the newest-first order from `get_recent` breaks ties.
            self.emails
                .sort_by_key(|e| std::cmp::Reverse(priority::score(e)));
        }

        // Keep the cursor on the same message when newer mail is inserted above it.
        if let Some(pos) = selected_id.and_then(|id| self.emails.iter().position(|e| e.id == id)) {
            self.selected_email = pos;
        } else if self.selected_email >= self.emails.len() {
            self.selected_email = self.emails.len().saturating_sub(1);
        }
        Ok(())
    }

    /// Cycles the email list's account filter: all accounts merged -> the first account with any
    /// stored mail -> the next -> ... -> all accounts merged again. `refresh_emails` applies
    /// whatever `account_filter` ends up holding, so every other view of `App::emails` (open,
    /// delete, archive, star, convert-to-task, search) sees only the filtered set automatically —
    /// no separate index to keep in sync.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn cycle_account_filter(&mut self) -> Result<(), sqlx::Error> {
        let accounts = email_store::distinct_accounts(&self.db_pool)
            .await
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        if accounts.is_empty() {
            return Ok(());
        }
        self.account_filter = self.account_filter.as_ref().map_or_else(
            || Some(accounts[0].clone()),
            |current| {
                let next = accounts
                    .iter()
                    .position(|a| a == current)
                    .map_or(0, |i| i + 1);
                accounts.get(next).cloned()
            },
        );
        self.refresh_emails().await
    }

    /// Cycles the email list's folder filter: all folders merged -> the first folder with any
    /// stored mail -> the next -> ... -> all folders merged again (`F` in the email list). Same
    /// shape as `cycle_account_filter` — `refresh_emails` applies whatever `folder_filter` ends up
    /// holding, so every other view of `App::emails` sees only the filtered set automatically.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn cycle_folder_filter(&mut self) -> Result<(), sqlx::Error> {
        let folders = email_store::distinct_folders(&self.db_pool)
            .await
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        if folders.is_empty() {
            return Ok(());
        }
        self.folder_filter = self.folder_filter.as_ref().map_or_else(
            || Some(folders[0].clone()),
            |current| {
                let next = folders
                    .iter()
                    .position(|f| f == current)
                    .map_or(0, |i| i + 1);
                folders.get(next).cloned()
            },
        );
        self.refresh_emails().await
    }

    /// Cycles the email list's focus filter: merged (all mail) -> Focused only -> Other only ->
    /// merged again (`I` in the email list) — Outlook's Focused Inbox split. Unlike
    /// `cycle_account_filter`/`cycle_folder_filter`, which cycle through a dynamic label list read
    /// from the DB, this is a fixed 3-state cycle since triage classification is always binary. An
    /// email not yet classified (`triage_focused` still `None`) matches neither `Some` state, so it
    /// drops out of both filtered views until `run_email_triage` catches up.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn cycle_focus_filter(&mut self) -> Result<(), sqlx::Error> {
        self.focus_filter = match self.focus_filter {
            None => Some(true),
            Some(true) => Some(false),
            Some(false) => None,
        };
        self.refresh_emails().await
    }

    /// Cycles the email list's attachment filter: merged (all mail) -> has attachments only -> no
    /// attachments only -> merged again (`H` in the email list, Slice 23). Same fixed 3-state shape
    /// as `cycle_focus_filter` — `has_attachments` is always known (computed at query time, see
    /// `email::store::get_recent`), never `None`, so there's no third "unclassified" bucket to fall
    /// out of.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn cycle_attachment_filter(&mut self) -> Result<(), sqlx::Error> {
        self.attachment_filter = match self.attachment_filter {
            None => Some(true),
            Some(true) => Some(false),
            Some(false) => None,
        };
        self.refresh_emails().await
    }

    /// Cycles the email list's unread filter: merged (all mail) -> unread only -> read only ->
    /// merged again (`U` in the email list, Slice 24). Same fixed 3-state shape as
    /// `cycle_attachment_filter` — `EmailMessage.is_read` is always known, never unclassified.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn cycle_unread_filter(&mut self) -> Result<(), sqlx::Error> {
        self.unread_filter = match self.unread_filter {
            None => Some(true),
            Some(true) => Some(false),
            Some(false) => None,
        };
        self.refresh_emails().await
    }

    /// Cycles the email list's starred filter: merged (all mail) -> starred only -> unstarred only
    /// -> merged again (`S` in the email list, Slice 24). Same fixed 3-state shape as
    /// `cycle_attachment_filter` — `EmailMessage.is_starred` is always known, never unclassified.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn cycle_starred_filter(&mut self) -> Result<(), sqlx::Error> {
        self.starred_filter = match self.starred_filter {
            None => Some(true),
            Some(true) => Some(false),
            Some(false) => None,
        };
        self.refresh_emails().await
    }

    /// Cycles the email list's sender-domain filter: all domains merged -> the first domain with
    /// any stored mail -> the next -> ... -> all domains merged again (`@` in the email list,
    /// Slice 25 — every free uppercase letter that reads naturally for "domain" was already
    /// claimed, `G` by the vim-motion `gg`/`G` top/bottom pair). Same shape as
    /// `cycle_account_filter`/`cycle_folder_filter` — a dynamic label list read from the DB, not a
    /// fixed 3-state cycle — since a domain is one of arbitrarily many values, not a
    /// binary/tri-state property.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn cycle_domain_filter(&mut self) -> Result<(), sqlx::Error> {
        let domains = email_store::distinct_domains(&self.db_pool)
            .await
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        if domains.is_empty() {
            return Ok(());
        }
        self.domain_filter = self.domain_filter.as_ref().map_or_else(
            || Some(domains[0].clone()),
            |current| {
                let next = domains
                    .iter()
                    .position(|d| d == current)
                    .map_or(0, |i| i + 1);
                domains.get(next).cloned()
            },
        );
        self.refresh_emails().await
    }

    /// Opens the folder-browser popup (`B` in the email list) and kicks off a background
    /// `MailSource::list_folders` LIST pass over every configured account (Slice 16), so the user
    /// can pick a server folder beyond the two this app otherwise ever names (`imap_folder`,
    /// `archive_folder`) — Sent, Drafts, Junk, or any custom folder.
    pub fn open_folder_browser(&mut self) {
        let configs = EmailConfig::all_from_env();
        if configs.is_empty() {
            self.notify("Email not configured (set TRIPTYCH_EMAIL_ENABLED and IMAP_* in .env)");
            return;
        }
        self.folder_browser_open = true;
        self.selected_discovered_folder = 0;
        self.notify("Listing folders...");

        let tx = self.folder_list_tx.clone();
        tokio::spawn(async move {
            let mut result = FolderListResult::default();
            for config in configs {
                let account = config.account.clone();
                match ImapMailSource::new(config).list_folders().await {
                    Ok(folders) => {
                        result
                            .folders
                            .extend(folders.into_iter().map(|f| (account.clone(), f)));
                    }
                    Err(e) => {
                        tracing::warn!("[Mail] folder list failed for '{account}': {e}");
                        result.errors.push(format!("{account}: {e}"));
                    }
                }
            }
            result.folders.sort();
            let _ = tx.send(result);
        });
    }

    /// Applies a finished background folder list: replaces `discovered_folders` and resets the
    /// popup's selection to the top. A partial failure (one account's LIST erroring) still shows
    /// whatever other accounts returned, plus a status message naming which account failed.
    pub fn apply_folder_list(&mut self, done: FolderListResult) {
        self.discovered_folders = done.folders;
        self.selected_discovered_folder = 0;
        if !done.errors.is_empty() {
            self.notify(&format!("Folder list failed: {}", done.errors.join("; ")));
        }
    }

    pub const fn close_folder_browser(&mut self) {
        self.folder_browser_open = false;
    }

    /// Picks the highlighted discovered folder, closes the popup, and kicks off a background
    /// one-folder sync (`sync_one_folder`) so its mail actually loads instead of just narrowing
    /// the filter over whatever happened to sync before.
    pub fn browse_to_selected_folder(&mut self) {
        let Some((account, folder)) = self
            .discovered_folders
            .get(self.selected_discovered_folder)
            .cloned()
        else {
            return;
        };
        let Some(config) = EmailConfig::for_account(&account) else {
            self.notify(&format!("No IMAP config for account '{account}'"));
            return;
        };
        self.folder_browser_open = false;
        self.folder_filter = Some(folder.clone());
        self.notify(&format!("Syncing '{folder}'..."));

        let (db_pool, tx) = (self.db_pool.clone(), self.folder_sync_tx.clone());
        tokio::spawn(async move {
            let result = sync_one_folder(&db_pool, &config, &folder)
                .await
                .map(|report| report.new)
                .map_err(|e| e.to_string());
            let _ = tx.send(FolderSyncResult { folder, result });
        });
    }

    /// Applies a finished background one-folder sync: refreshes the email list if it's the
    /// visible view (so the newly synced folder's mail actually appears), then reports how many
    /// new messages came in.
    pub async fn apply_folder_sync(&mut self, done: FolderSyncResult) {
        if self.view_mode == ViewMode::Email && !self.email_detail_open {
            let _ = self.refresh_emails().await;
        }
        self.run_email_triage();
        self.run_email_rules().await;
        let msg = match done.result {
            Ok(0) => format!("'{}' - nothing new", done.folder),
            Ok(n) => format!("Synced {n} new from '{}'", done.folder),
            Err(e) => format!("Sync of '{}' failed: {e}", done.folder),
        };
        self.notify(&msg);
    }

    /// Starts typing a snooze spec (`z` in the email list); `commit_snooze`/`cancel_snooze` ends it.
    pub fn start_snooze_prompt(&mut self) {
        self.input_buffer.clear();
        self.input_mode = InputMode::EmailSnooze;
    }

    pub fn cancel_snooze(&mut self) {
        self.input_buffer.clear();
        self.input_mode = InputMode::Normal;
    }

    /// Ends typing and, on a valid spec, snoozes the selected email (hides it from the normal
    /// list until then — see `EmailMessage::snoozed_until`). An empty selection, an unparseable
    /// spec, or a database error reports a status message and applies nothing.
    pub async fn commit_snooze(&mut self) {
        let typed = self.input_buffer.trim().to_lowercase();
        self.cancel_snooze();
        let Some(email) = self.emails.get(self.selected_email) else {
            return;
        };
        let email_id = email.id;
        let Some(until) = parse_snooze_spec(&typed, Utc::now()) else {
            self.notify(&format!("Not a snooze spec: {typed}"));
            return;
        };
        if let Err(e) = email_store::set_snooze(&self.db_pool, email_id, until).await {
            self.notify(&format!("Error: {e}"));
            return;
        }
        if let Err(e) = self.refresh_emails().await {
            self.notify(&format!("Error: {e}"));
            return;
        }
        self.notify(&format!(
            "Snoozed until {}",
            until.with_timezone(&chrono::Local).format("%a %H:%M")
        ));
    }

    /// Clears an in-progress snooze on the selected email early (`x` in the email list).
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn unsnooze_selected_email(&mut self) -> Result<(), sqlx::Error> {
        let Some(email) = self.emails.get(self.selected_email) else {
            return Ok(());
        };
        email_store::clear_snooze(&self.db_pool, email.id)
            .await
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        self.refresh_emails().await
    }

    /// Toggles between the normal inbox and the snoozed-only view (`Z` in the email list).
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn toggle_show_snoozed(&mut self) -> Result<(), sqlx::Error> {
        self.show_snoozed = !self.show_snoozed;
        self.refresh_emails().await
    }

    /// Switches the Email list between priority and date order; the cursor stays on its message.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn toggle_email_sort(&mut self) -> Result<(), sqlx::Error> {
        self.email_sort = self.email_sort.toggled();
        self.refresh_emails().await?;
        self.notify(&format!("Sorted by {}", self.email_sort.label()));
        Ok(())
    }

    /// Marks the selected email as read.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn mark_selected_email_read(&mut self) -> Result<(), sqlx::Error> {
        let Some(email) = self.emails.get(self.selected_email) else {
            return Ok(());
        };
        let email_id = email.id;

        crate::email::store::mark_read(&self.db_pool, email_id)
            .await
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;

        self.refresh_emails().await
    }

    /// Marks the selected email as unread.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn mark_selected_email_unread(&mut self) -> Result<(), sqlx::Error> {
        let Some(email) = self.emails.get(self.selected_email) else {
            return Ok(());
        };
        let email_id = email.id;

        crate::email::store::mark_unread(&self.db_pool, email_id)
            .await
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;

        self.refresh_emails().await
    }

    /// Toggles the starred flag on the selected email.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn toggle_selected_star(&mut self) -> Result<(), sqlx::Error> {
        let Some(email) = self.emails.get(self.selected_email) else {
            return Ok(());
        };
        let email_id = email.id;
        let starred = !email.is_starred;

        crate::email::store::set_starred(&self.db_pool, email_id, starred)
            .await
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;

        self.refresh_emails().await
    }

    /// Cycles the selected email's colored category tag (`t` in the email list) through the fixed
    /// [`CATEGORY_ORDER`] palette, wrapping back to untagged after the last colour — Outlook's
    /// colored categories, one tag per message rather than Outlook's several. Same shape as
    /// `toggle_selected_star`.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn cycle_selected_category(&mut self) -> Result<(), sqlx::Error> {
        let Some(email) = self.emails.get(self.selected_email) else {
            return Ok(());
        };
        let email_id = email.id;
        let next = next_category(email.category.as_deref());

        crate::email::store::set_category(&self.db_pool, email_id, next.as_deref())
            .await
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;

        self.refresh_emails().await
    }

    /// Opens the email detail popup on the selected email and marks it read,
    /// same as most mail clients do on open. `self.emails` (from `get_recent`)
    /// never carries `body_text` - fetched here, on demand, only for the one
    /// email actually being viewed, rather than for every row in the list.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn open_selected_email(&mut self) -> Result<(), sqlx::Error> {
        let Some(email) = self.emails.get(self.selected_email) else {
            return Ok(());
        };
        let email_id = email.id;
        let has_attachments = email.has_attachments;
        self.email_detail_open = true;
        self.email_detail_scroll = 0;
        self.mark_selected_email_read().await?;

        let body = crate::email::store::get_body(&self.db_pool, email_id)
            .await
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        if let Some(email) = self.emails.iter_mut().find(|e| e.id == email_id) {
            email.body_text = body;
        }
        if has_attachments && !self.email_attachments.contains_key(&email_id) {
            let attachments = email_store::get_attachments(&self.db_pool, email_id)
                .await
                .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
            self.email_attachments.insert(email_id, attachments);
        }
        self.request_email_summary(email_id).await;
        Ok(())
    }

    /// Shows the cached summary of `email_id`, or starts generating one in the background: the local
    /// model can take many seconds, so the popup opens at once and `apply_summary` fills it in.
    /// Short mail gets none. Needs the body, so call it after `open_selected_email` has loaded it.
    async fn request_email_summary(&mut self, email_id: i64) {
        if matches!(
            self.email_summaries.get(&email_id),
            Some(Summary::Pending | Summary::Ready(_))
        ) {
            return;
        }
        if let Ok(Some(cached)) = email_store::get_summary(&self.db_pool, email_id).await {
            self.email_summaries
                .insert(email_id, Summary::Ready(cached));
            return;
        }
        let Some(email) = self.emails.iter().find(|e| e.id == email_id) else {
            return;
        };
        let Some(body) = email
            .body_text
            .as_deref()
            .filter(|b| b.trim().chars().count() >= SUMMARY_MIN_CHARS)
        else {
            return;
        };
        let input = format!(
            "From: {}\nSubject: {}\n\n{}",
            email.from_name.as_deref().unwrap_or(&email.from_addr),
            email.subject,
            body.chars().take(SUMMARY_INPUT_CHARS).collect::<String>()
        );

        self.email_summaries.insert(email_id, Summary::Pending);
        let (parser, tx) = (Arc::clone(&self.nlp_parser), self.summary_tx.clone());
        tokio::spawn(async move {
            let result = parser.summarize(&input).await.map_err(|e| {
                tracing::warn!("[Email] summary failed: {e}");
                e.short_reason()
            });
            let _ = tx.send(SummaryDone { email_id, result });
        });
    }

    /// Stores a finished summary and shows it if that email's popup is open.
    pub async fn apply_summary(&mut self, done: SummaryDone) {
        let state = match done.result {
            Ok(text) => {
                if let Err(e) = email_store::set_summary(&self.db_pool, done.email_id, &text).await
                {
                    tracing::warn!("[Email] could not cache a summary: {e}");
                }
                Summary::Ready(text)
            }
            Err(reason) => Summary::Failed(reason),
        };
        self.email_summaries.insert(done.email_id, state);
    }

    /// Runs a background triage pass (Outlook's Focused Inbox split) over every email not yet
    /// classified, capped at `TRIAGE_BATCH_LIMIT` per pass, wired in after every sync
    /// (`apply_mail_sync`, `apply_folder_sync`). Spawned and never awaited, same reasoning as
    /// `start_email_sync`; a second pass is not started while one is running (`triage_running`).
    /// `NLPParser::triage`, like `summarize`, ignores the sticky `ollama_available` flag, so this
    /// still tries and simply logs+skips per-email on failure rather than erroring the whole pass.
    pub fn run_email_triage(&mut self) {
        if self.triage_running {
            return;
        }
        self.triage_running = true;

        let (db_pool, parser, tx) = (
            self.db_pool.clone(),
            Arc::clone(&self.nlp_parser),
            self.triage_tx.clone(),
        );
        tokio::spawn(async move {
            let mut done = TriageDone::default();
            let pending = email_store::pending_triage(&db_pool, TRIAGE_BATCH_LIMIT)
                .await
                .unwrap_or_default();
            for email in pending {
                let snippet = email.snippet.as_deref().unwrap_or("");
                match parser.triage(&email.subject, snippet).await {
                    Ok(focused) => {
                        if email_store::set_triage(&db_pool, email.id, focused)
                            .await
                            .is_ok()
                        {
                            done.results.push((email.id, focused));
                        }
                    }
                    Err(e) => tracing::warn!("[Email] triage failed for {}: {e}", email.id),
                }
            }
            let _ = tx.send(done);
        });
    }

    /// Applies a finished background triage pass: updates each classified email's in-memory
    /// `triage_focused` directly (rather than a full `refresh_emails` reload, which would also
    /// re-run the whole filter chain and could move the cursor) and clears the running guard.
    pub fn apply_triage(&mut self, done: TriageDone) {
        self.triage_running = false;
        for (id, focused) in done.results {
            if let Some(email) = self.emails.iter_mut().find(|e| e.id == id) {
                email.triage_focused = Some(focused);
            }
        }
    }

    pub const fn close_email_detail(&mut self) {
        self.email_detail_open = false;
        self.email_detail_scroll = 0;
    }

    /// Starts creating a task from the selected email (parsed in the background, see
    /// `App::submit_task`); `finish_email_conversion` then links the two. When there's a snippet
    /// (body preview), it's folded in alongside the subject before parsing, so a deadline/date/
    /// priority/tag phrase that only appears in the body is still picked up — but the title is then
    /// forced back to the bare subject (`title_override`), since the NLP parser's last-resort
    /// fallback is the entire unparsed input verbatim and body prose would otherwise leak into it.
    /// A subject with no snippet skips the override and keeps the parser's own cleaned-up title
    /// (e.g. a trailing "tomorrow" stripped out into the deadline field), unchanged from before.
    ///
    /// Separately fetches the full body (on demand, like `open_selected_email` — list rows never
    /// carry it) and runs it through `message::clean_body_for_deadline_scan` +
    /// `nlp::rules::extract_deadline_only`, so a deadline stated only deep in the body (past the
    /// 200-char snippet) is still found. That result is passed as `submit_task`'s `body_deadline`
    /// fallback rather than folded into `description`: `extract_deadline_only` only ever returns a
    /// trigger-word-anchored deadline (never a bare date), and it only applies if the subject+
    /// snippet parse found no deadline of its own — a deliberately narrow path so body prose can
    /// promote a *deadline* but can never hijack the title or the scheduled `due_date`.
    pub async fn convert_selected_email_to_task(&mut self) {
        let Some(email) = self.emails.get(self.selected_email) else {
            return;
        };
        if email.task_id.is_some() {
            self.status_message = Some((
                "Email already converted to a task".to_string(),
                std::time::Instant::now(),
            ));
            return;
        }
        let (email_id, subject) = (email.id, email.subject.clone());
        let snippet = email.snippet.clone().filter(|s| !s.is_empty());
        let description = snippet.as_deref().map_or_else(
            || subject.clone(),
            |snippet| format!("{subject}. {snippet}"),
        );
        let title_override = snippet.is_some().then(|| subject.clone());

        let body_deadline = email_store::get_body(&self.db_pool, email_id)
            .await
            .ok()
            .flatten()
            .and_then(|body| {
                let cleaned = message::clean_body_for_deadline_scan(&body);
                crate::nlp::rules::extract_deadline_only(&cleaned)
            });

        self.submit_task(description, Some(email_id), title_override, body_deadline);
    }

    /// Links the task made from an email, marks the email read and reloads the list.
    pub(super) async fn finish_email_conversion(
        &mut self,
        email_id: i64,
        task_id: i64,
    ) -> Result<(), sqlx::Error> {
        crate::email::store::link_task(&self.db_pool, email_id, task_id)
            .await
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;

        crate::email::store::mark_read(&self.db_pool, email_id)
            .await
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;

        self.status_message = Some((
            "Email converted to task".to_string(),
            std::time::Instant::now(),
        ));
        self.refresh_emails().await
    }

    /// Accepts the selected email's meeting invite (Slice 19: `meeting_title`/`meeting_start` set
    /// by `email::message::parse_raw`'s ICS extraction) as a scheduled task, direct-`INSERT`ed with
    /// `scheduled_at` already known — unlike `convert_selected_email_to_task`, this never goes
    /// through `submit_task`'s NLP parse, since the exact time is already on hand from the ICS
    /// `DTSTART`/`DTEND`, the same direct-insert shape as `placement::add_task_at_selected_cell`.
    /// No-ops (with a status message) if the email has no invite or was already converted —
    /// reuses `email_messages.task_id` as the same "already converted" marker
    /// `convert_selected_email_to_task` uses, rather than a separate flag.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn accept_meeting_invite(&mut self) -> Result<(), sqlx::Error> {
        let Some(email) = self.emails.get(self.selected_email) else {
            return Ok(());
        };
        if email.task_id.is_some() {
            self.notify("Email already converted to a task");
            return Ok(());
        }
        let Some(title) = email.meeting_title.clone() else {
            self.notify("This email has no meeting invite");
            return Ok(());
        };
        let Some(start) = email.meeting_start else {
            self.notify("This email has no meeting invite");
            return Ok(());
        };
        let email_id = email.id;
        let duration_minutes = email
            .meeting_end
            .and_then(|end| (end - start).num_minutes().try_into().ok())
            .filter(|&m: &i32| m > 0);
        let category = super::tasks::classify_task(&title).to_string();
        let new_order = i64::try_from(self.tasks.len()).unwrap_or(i64::MAX);

        let result = sqlx::query(
            "INSERT INTO tasks (description, completed, item_order, priority, scheduled_at, duration_minutes, task_category) VALUES (?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(format!("Meeting: {title}"))
        .bind(false)
        .bind(new_order)
        .bind(1i32)
        .bind(start)
        .bind(duration_minutes)
        .bind(&category)
        .execute(&self.db_pool)
        .await?;
        let task_id = result.last_insert_rowid();

        self.load_tasks().await?;
        self.refresh_calendar_data().await;
        self.finish_email_conversion(email_id, task_id).await
    }

    /// Opens a blank compose form addressed to nobody, sent from the first account with SMTP
    /// configured (multi-account "from" pick is future work — see `docs/roadmap-email.md`).
    pub fn start_compose_new(&mut self) {
        let Some(smtp) = SmtpConfig::all_from_env().into_iter().next() else {
            self.notify("No SMTP account configured (set SMTP_* in .env)");
            return;
        };
        let mut compose = ComposeState::blank(smtp.account);
        compose.signature = smtp.signature;
        self.email_compose = Some(compose);
        self.input_mode = InputMode::EmailCompose;
    }

    /// Opens a reply (or reply-all) form for the selected email, prefilled with recipient(s),
    /// `Re:` subject, `In-Reply-To`/`References` threading headers and a quoted copy of the
    /// original body. No-ops if no email is selected or its account has no SMTP config.
    pub fn start_reply(&mut self, reply_all: bool) {
        let Some(email) = self.emails.get(self.selected_email) else {
            return;
        };
        let Some(smtp) = SmtpConfig::for_account(&email.account) else {
            self.notify(&format!("No SMTP config for account '{}'", email.account));
            return;
        };
        let to = email.from_addr.clone();
        let cc = if reply_all {
            merge_reply_all_cc(email, &to, &smtp.from_addr)
        } else {
            String::new()
        };
        self.email_compose = Some(ComposeState {
            to,
            cc,
            subject: reply_subject(&email.subject),
            body: String::new(),
            active_field: ComposeField::Body,
            account: email.account.clone(),
            in_reply_to: Some(email.message_id.clone()),
            references: Some(chain_references(email)),
            quoted: Some(quote_original(email)),
            signature: smtp.signature,
            draft_id: None,
        });
        self.input_mode = InputMode::EmailCompose;
    }

    /// Opens a forward form for the selected email: blank `To`, `Fwd:` subject, quoted original
    /// body, no threading headers (a forward starts a new thread with its recipient).
    pub fn start_forward(&mut self) {
        let Some(email) = self.emails.get(self.selected_email) else {
            return;
        };
        let Some(smtp) = SmtpConfig::for_account(&email.account) else {
            self.notify(&format!("No SMTP config for account '{}'", email.account));
            return;
        };
        self.email_compose = Some(ComposeState {
            to: String::new(),
            cc: String::new(),
            subject: forward_subject(&email.subject),
            body: String::new(),
            active_field: ComposeField::To,
            account: email.account.clone(),
            in_reply_to: None,
            references: None,
            quoted: Some(quote_original(email)),
            signature: smtp.signature,
            draft_id: None,
        });
        self.input_mode = InputMode::EmailCompose;
    }

    pub fn cancel_compose(&mut self) {
        self.email_compose = None;
        self.input_mode = InputMode::Normal;
    }

    pub fn compose_push_char(&mut self, c: char) {
        let Some(compose) = self.email_compose.as_mut() else {
            return;
        };
        match compose.active_field {
            ComposeField::To => compose.to.push(c),
            ComposeField::Cc => compose.cc.push(c),
            ComposeField::Subject => compose.subject.push(c),
            ComposeField::Body => compose.body.push(c),
        }
    }

    pub fn compose_backspace(&mut self) {
        let Some(compose) = self.email_compose.as_mut() else {
            return;
        };
        match compose.active_field {
            ComposeField::To => {
                compose.to.pop();
            }
            ComposeField::Cc => {
                compose.cc.pop();
            }
            ComposeField::Subject => {
                compose.subject.pop();
            }
            ComposeField::Body => {
                compose.body.pop();
            }
        }
    }

    /// Enter within the `Body` field inserts a newline; the single-line fields ignore it (`Tab`
    /// moves between those instead).
    pub fn compose_newline(&mut self) {
        if let Some(compose) = self.email_compose.as_mut()
            && compose.active_field == ComposeField::Body
        {
            compose.body.push('\n');
        }
    }

    pub const fn compose_next_field(&mut self) {
        if let Some(compose) = self.email_compose.as_mut() {
            compose.next_field();
        }
    }

    pub const fn compose_prev_field(&mut self) {
        if let Some(compose) = self.email_compose.as_mut() {
            compose.prev_field();
        }
    }

    /// Sends the open compose form in the background (a stalled SMTP server must not freeze the
    /// TUI, same reasoning as `start_email_sync`). The form stays open until `apply_send_result`
    /// confirms success, so a failed send doesn't lose the draft.
    pub fn send_compose(&mut self) {
        let Some(compose) = self.email_compose.clone() else {
            return;
        };
        let Some(smtp_config) = SmtpConfig::for_account(&compose.account) else {
            self.notify(&format!("No SMTP config for account '{}'", compose.account));
            return;
        };
        if compose.to.trim().is_empty() {
            self.notify("Cannot send: no recipient");
            return;
        }

        let draft_id = compose.draft_id;
        let body = compose_full_body(
            &compose.body,
            compose.signature.as_deref(),
            compose.quoted.as_deref(),
        );
        let message = smtp::OutgoingMessage {
            to: compose.to,
            cc: compose.cc,
            subject: compose.subject,
            body,
            in_reply_to: compose.in_reply_to,
            references: compose.references,
        };

        self.notify("Sending...");
        let tx = self.send_tx.clone();
        tokio::spawn(async move {
            let result = smtp::send(&smtp_config, &message)
                .await
                .map_err(|e| e.to_string());
            let _ = tx.send(SendResult { result, draft_id });
        });
    }

    /// Applies a finished background send: on success, closes the compose form (and, if it was
    /// resumed from a saved draft, deletes that draft row) — on failure, leaves it open so the
    /// draft isn't lost.
    pub async fn apply_send_result(&mut self, sent: SendResult) {
        match sent.result {
            Ok(()) => {
                self.notify("Sent");
                self.email_compose = None;
                self.input_mode = InputMode::Normal;
                if let Some(id) = sent.draft_id
                    && let Err(e) = drafts::delete_draft(&self.db_pool, id).await
                {
                    tracing::warn!("Failed to delete sent draft {id}: {e}");
                }
            }
            Err(e) => self.notify(&format!("Send failed: {e}")),
        }
    }

    /// Saves the open compose form as a draft (overwriting the row it was resumed from, if any)
    /// and closes the compose form. Unlike sending, this is a single small write, so it runs
    /// directly on the DB pool rather than through the spawn+channel pattern `send_compose` uses.
    pub async fn save_compose_as_draft(&mut self) {
        let Some(compose) = self.email_compose.clone() else {
            return;
        };

        match drafts::save_draft(
            &self.db_pool,
            compose.draft_id,
            &compose.account,
            &compose.to,
            &compose.cc,
            &compose.subject,
            &compose.body,
        )
        .await
        {
            Ok(_id) => {
                self.notify("Draft saved");
                self.email_compose = None;
                self.input_mode = InputMode::Normal;
            }
            Err(e) => self.notify(&format!("Failed to save draft: {e}")),
        }
    }

    /// Opens the drafts list popup (`D` in the email view), loading every saved draft fresh from
    /// the DB so it reflects the latest save.
    pub async fn open_drafts_list(&mut self) {
        match drafts::list_drafts(&self.db_pool).await {
            Ok(drafts) => {
                self.drafts = drafts;
                self.selected_draft = 0;
                self.drafts_open = true;
            }
            Err(e) => self.notify(&format!("Failed to load drafts: {e}")),
        }
    }

    pub const fn close_drafts_list(&mut self) {
        self.drafts_open = false;
    }

    /// Reopens the selected draft in the compose form, cursor on `Body`, ready to keep writing.
    /// Not threaded as a reply/forward — a draft is a message that was never sent, so it carries
    /// no `In-Reply-To`/`References` of its own.
    pub fn resume_selected_draft(&mut self) {
        let Some(draft) = self.drafts.get(self.selected_draft) else {
            return;
        };
        let signature = SmtpConfig::for_account(&draft.account).and_then(|c| c.signature);
        self.email_compose = Some(ComposeState {
            to: draft.to_addrs.clone(),
            cc: draft.cc_addrs.clone(),
            subject: draft.subject.clone(),
            body: draft.body.clone(),
            active_field: ComposeField::Body,
            account: draft.account.clone(),
            in_reply_to: None,
            references: None,
            quoted: None,
            signature,
            draft_id: Some(draft.id),
        });
        self.drafts_open = false;
        self.input_mode = InputMode::EmailCompose;
    }

    /// Deletes the selected draft from the DB and the open list, clamping the selection so it
    /// stays in bounds (mirrors `apply_delete_result`'s local-row-drop shape, but synchronous
    /// end-to-end since this popup's own list is not the shared `App::emails` list).
    pub async fn delete_selected_draft(&mut self) {
        let Some(draft) = self.drafts.get(self.selected_draft) else {
            return;
        };
        let id = draft.id;

        if let Err(e) = drafts::delete_draft(&self.db_pool, id).await {
            self.notify(&format!("Failed to delete draft: {e}"));
            return;
        }

        self.drafts.remove(self.selected_draft);
        if self.selected_draft >= self.drafts.len() && self.selected_draft > 0 {
            self.selected_draft -= 1;
        }
    }

    /// Opens the rules popup (`R` in the email list), loading every saved rule fresh from the DB.
    pub async fn open_rules_list(&mut self) {
        match email_store::list_rules(&self.db_pool).await {
            Ok(rules) => {
                self.rules = rules;
                self.selected_rule = 0;
                self.rules_open = true;
            }
            Err(e) => self.notify(&format!("Failed to load rules: {e}")),
        }
    }

    pub const fn close_rules_list(&mut self) {
        self.rules_open = false;
    }

    /// Starts typing a rule spec (`n` in the rules popup); `commit_rule_input`/`cancel_rule_input`
    /// ends it. See `parse_rule_spec` for the expected shape.
    pub fn start_rule_input(&mut self) {
        self.input_buffer.clear();
        self.input_mode = InputMode::EmailRuleInput;
    }

    pub fn cancel_rule_input(&mut self) {
        self.input_buffer.clear();
        self.input_mode = InputMode::Normal;
    }

    /// Ends typing and, on a valid spec, saves the new rule and reloads the popup's list. An
    /// unparseable spec or a database error reports a status message and adds nothing.
    pub async fn commit_rule_input(&mut self) {
        let typed = self.input_buffer.trim().to_string();
        self.cancel_rule_input();
        let Some((match_field, pattern, action)) = parse_rule_spec(&typed) else {
            self.notify(
                "Rule not understood (e.g. 'subject newsletter star', 'from noreply archive')",
            );
            return;
        };
        match email_store::create_rule(&self.db_pool, &match_field, &pattern, &action).await {
            Ok(_) => self.open_rules_list().await,
            Err(e) => self.notify(&format!("Failed to save rule: {e}")),
        }
    }

    /// Deletes the selected rule from the DB and the open list, clamping the selection so it stays
    /// in bounds — same shape as `delete_selected_draft`.
    pub async fn delete_selected_rule(&mut self) {
        let Some(rule) = self.rules.get(self.selected_rule) else {
            return;
        };
        let id = rule.id;

        if let Err(e) = email_store::delete_rule(&self.db_pool, id).await {
            self.notify(&format!("Failed to delete rule: {e}"));
            return;
        }

        self.rules.remove(self.selected_rule);
        if self.selected_rule >= self.rules.len() && self.selected_rule > 0 {
            self.selected_rule -= 1;
        }
    }

    /// Applies every saved rule to messages not yet checked (`rule_applied = 0`), capped at
    /// `RULE_CHECK_LIMIT` per pass — wired in after every sync (`apply_mail_sync`,
    /// `apply_folder_sync`), like `run_email_triage`. `star`/`read` are applied inline and
    /// awaited (pure local-DB work, no long-running I/O to keep off the render loop); `archive`/
    /// `delete` (Slice 21) are spawned by `apply_rule_action` itself and only reported back later
    /// over `archive_tx`/`delete_tx`, same as their manual (`a`/`d`) counterparts.
    pub async fn run_email_rules(&mut self) {
        let rules = email_store::list_rules(&self.db_pool)
            .await
            .unwrap_or_default();
        if rules.is_empty() {
            return;
        }
        let pending = email_store::pending_rule_check(&self.db_pool, RULE_CHECK_LIMIT)
            .await
            .unwrap_or_default();
        for email in pending {
            for rule in rules
                .iter()
                .filter(|r| match_rule(r, &email.subject, &email.from_addr))
            {
                self.apply_rule_action(&email, &rule.action).await;
            }
            if let Err(e) = email_store::mark_rule_checked(&self.db_pool, email.id).await {
                tracing::warn!("[Email] rule check mark failed for {}: {e}", email.id);
            }
        }
    }

    /// Runs one rule's action against one email. `star`/`read` write straight to the DB and patch
    /// the matching `App::emails` entry in memory (if loaded) — same reasoning as `apply_triage`,
    /// avoids a full `refresh_emails` reload mid-pass. `archive`/`delete` (Slice 21) instead hand
    /// off to `spawn_archive`/`spawn_delete`: a network call can't be awaited inline here without
    /// stalling every other rule and email in the same sync pass, so their local-row cleanup
    /// happens later, generically, via `apply_archive_result`/`apply_delete_result` — a rule match
    /// is indistinguishable from a manual `a`/`d` press once spawned. Silently no-ops (skips this
    /// action, still marks the email checked) when the account has no IMAP config or an invalid
    /// UID, same as the manual paths' guard clauses minus the user-facing `notify`.
    async fn apply_rule_action(&mut self, email: &EmailMessage, action: &str) {
        match action {
            "star" => {
                if email_store::set_starred(&self.db_pool, email.id, true)
                    .await
                    .is_ok()
                    && let Some(e) = self.emails.iter_mut().find(|e| e.id == email.id)
                {
                    e.is_starred = true;
                }
            }
            "read" => {
                if email_store::mark_read(&self.db_pool, email.id)
                    .await
                    .is_ok()
                    && let Some(e) = self.emails.iter_mut().find(|e| e.id == email.id)
                {
                    e.is_read = true;
                }
            }
            "archive" | "delete" => {
                let Some(config) = EmailConfig::for_account(&email.account) else {
                    return;
                };
                let Ok(uid) = u32::try_from(email.uid) else {
                    return;
                };
                let folder = email.folder.clone();
                if action == "archive" {
                    self.spawn_archive(config, folder, uid, email.id);
                } else {
                    self.spawn_delete(config, folder, uid, email.id);
                }
            }
            _ => {}
        }
    }

    /// Spawns the server-delete-then-local-drop sequence for one message and reports over
    /// `delete_tx`, never awaited — shared by `delete_selected_email` (user-initiated, `d`) and
    /// `apply_rule_action`'s `"delete"` action (Slice 21), so a rule's network call can't freeze
    /// the TUI any more than a manual delete can.
    fn spawn_delete(&self, config: EmailConfig, folder: String, uid: u32, email_id: i64) {
        let (db_pool, tx) = (self.db_pool.clone(), self.delete_tx.clone());
        tokio::spawn(async move {
            let source = ImapMailSource::new(config);
            let result = match source.delete(&folder, uid).await {
                Ok(()) => email_store::delete_email(&db_pool, email_id)
                    .await
                    .map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(DeleteResult { email_id, result });
        });
    }

    /// Deletes the selected email: server first (`UID STORE +FLAGS.SILENT (\Deleted)` then
    /// `EXPUNGE`, spawned and never awaited — an unreachable IMAP server must not freeze the TUI,
    /// same reasoning as `start_email_sync`), local row only on that success (see
    /// `store::delete_email`'s doc). Permanent: this client has no multi-folder support, so there
    /// is no "move to Trash" to undo it from within Triptych.
    pub fn delete_selected_email(&mut self) {
        let Some(email) = self.emails.get(self.selected_email) else {
            return;
        };
        let Some(config) = EmailConfig::for_account(&email.account) else {
            self.notify(&format!("No IMAP config for account '{}'", email.account));
            return;
        };
        let Ok(uid) = u32::try_from(email.uid) else {
            self.notify("Cannot delete: invalid UID");
            return;
        };
        let (email_id, folder) = (email.id, email.folder.clone());

        self.notify("Deleting...");
        self.spawn_delete(config, folder, uid, email_id);
    }

    /// Applies a finished background delete: on success, drops the row from the in-memory list
    /// (closing the detail popup if it was open on that message) so the list reflects the server
    /// at once instead of waiting for the next 2s reload; on failure, leaves everything as-is so
    /// a network error never silently loses mail the server still has.
    pub fn apply_delete_result(&mut self, done: DeleteResult) {
        match done.result {
            Ok(()) => {
                self.notify("Deleted");
                self.emails.retain(|e| e.id != done.email_id);
                if self.selected_email >= self.emails.len() {
                    self.selected_email = self.emails.len().saturating_sub(1);
                }
                if self.email_detail_open {
                    self.close_email_detail();
                }
            }
            Err(e) => self.notify(&format!("Delete failed: {e}")),
        }
    }

    /// Spawns the server-archive-then-local-drop sequence for one message and reports over
    /// `archive_tx`, never awaited — shared by `archive_selected_email` (user-initiated, `a`) and
    /// `apply_rule_action`'s `"archive"` action (Slice 21), same reasoning as `spawn_delete`.
    fn spawn_archive(&self, config: EmailConfig, folder: String, uid: u32, email_id: i64) {
        let (db_pool, tx) = (self.db_pool.clone(), self.archive_tx.clone());
        tokio::spawn(async move {
            let source = ImapMailSource::new(config);
            let result = match source.archive(&folder, uid).await {
                Ok(()) => email_store::delete_email(&db_pool, email_id)
                    .await
                    .map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(ArchiveResult { email_id, result });
        });
    }

    /// Archives the selected email: server first (`UID MOVE` to `EmailConfig::archive_folder`,
    /// falling back to `COPY`+`STORE`+`EXPUNGE` when the server lacks the `MOVE` extension —
    /// see `client.rs`'s `archive_inner`), local row only on that success, same reasoning as
    /// `delete_selected_email`. Does not create the destination folder if it's missing.
    pub fn archive_selected_email(&mut self) {
        let Some(email) = self.emails.get(self.selected_email) else {
            return;
        };
        let Some(config) = EmailConfig::for_account(&email.account) else {
            self.notify(&format!("No IMAP config for account '{}'", email.account));
            return;
        };
        let Ok(uid) = u32::try_from(email.uid) else {
            self.notify("Cannot archive: invalid UID");
            return;
        };
        let (email_id, folder) = (email.id, email.folder.clone());

        self.notify("Archiving...");
        self.spawn_archive(config, folder, uid, email_id);
    }

    /// Applies a finished background archive: on success, drops the row from the in-memory list
    /// (closing the detail popup if it was open on that message), same as `apply_delete_result`;
    /// on failure, leaves everything as-is.
    pub fn apply_archive_result(&mut self, done: ArchiveResult) {
        match done.result {
            Ok(()) => {
                self.notify("Archived");
                self.emails.retain(|e| e.id != done.email_id);
                if self.selected_email >= self.emails.len() {
                    self.selected_email = self.emails.len().saturating_sub(1);
                }
                if self.email_detail_open {
                    self.close_email_detail();
                }
            }
            Err(e) => self.notify(&format!("Archive failed: {e}")),
        }
    }

    /// Saves every attachment of the selected email to disk: re-fetches the message over IMAP
    /// (bytes are never persisted — see `MailSource::fetch_attachments`), spawned and never
    /// awaited, same reasoning as `delete_selected_email`. No-ops if the email has none.
    pub fn save_selected_attachments(&mut self) {
        let Some(email) = self.emails.get(self.selected_email) else {
            return;
        };
        if !email.has_attachments {
            self.notify("No attachments on this email");
            return;
        }
        let Some(config) = EmailConfig::for_account(&email.account) else {
            self.notify(&format!("No IMAP config for account '{}'", email.account));
            return;
        };
        let Ok(uid) = u32::try_from(email.uid) else {
            self.notify("Cannot fetch attachments: invalid UID");
            return;
        };
        let email_id = email.id;
        let folder = email.folder.clone();
        let dir = attachment_dir().join(email_id.to_string());

        self.notify("Saving attachments...");
        let tx = self.attachment_tx.clone();
        tokio::spawn(async move {
            let result = save_attachments_to(&config, &folder, uid, &dir).await;
            let _ = tx.send(AttachmentSaveResult { result });
        });
    }

    /// Applies a finished background attachment save: reports the count and directory, or why it
    /// failed. No list/popup state to update — nothing here changes what's stored or shown.
    pub fn apply_attachment_save(&mut self, done: AttachmentSaveResult) {
        match done.result {
            Ok((0, dir)) => self.notify(&format!("No attachments found in {dir}")),
            Ok((n, dir)) => self.notify(&format!("Saved {n} attachment(s) to {dir}")),
            Err(e) => self.notify(&format!("Attachment save failed: {e}")),
        }
    }
}

/// Connects once, fetches every attachment on `uid`, and writes each to `dir` (created if
/// missing), sanitizing each stored filename against directory traversal first. Returns the
/// count written and `dir` as a displayable string.
async fn save_attachments_to(
    config: &EmailConfig,
    folder: &str,
    uid: u32,
    dir: &std::path::Path,
) -> Result<(usize, String), String> {
    let source = ImapMailSource::new(config.clone());
    let attachments = source
        .fetch_attachments(folder, uid)
        .await
        .map_err(|e| e.to_string())?;
    if attachments.is_empty() {
        return Ok((0, dir.display().to_string()));
    }

    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    for (index, (name, bytes)) in attachments.iter().enumerate() {
        let filename = name
            .as_deref()
            .and_then(sanitize_filename)
            .unwrap_or_else(|| format!("attachment-{index}"));
        std::fs::write(dir.join(filename), bytes).map_err(|e| e.to_string())?;
    }
    Ok((attachments.len(), dir.display().to_string()))
}

#[must_use]
pub fn reply_subject(subject: &str) -> String {
    if subject
        .get(..3)
        .is_some_and(|s| s.eq_ignore_ascii_case("re:"))
    {
        subject.to_string()
    } else {
        format!("Re: {subject}")
    }
}

#[must_use]
pub fn forward_subject(subject: &str) -> String {
    if subject
        .get(..4)
        .is_some_and(|s| s.eq_ignore_ascii_case("fwd:"))
    {
        subject.to_string()
    } else {
        format!("Fwd: {subject}")
    }
}

/// Fixed palette a category cycles through (`t` in the email list).
///
/// none -> Red -> Orange -> Yellow -> Green -> Blue -> Purple -> none. Outlook's colored-category
/// tagging, simplified to a single tag per message (Outlook allows several) since one is enough to
/// sort/scan by at a glance.
pub const CATEGORY_ORDER: [&str; 6] = ["red", "orange", "yellow", "green", "blue", "purple"];

/// The next category after `current` in [`CATEGORY_ORDER`], wrapping back to `None` after the last.
#[must_use]
pub fn next_category(current: Option<&str>) -> Option<String> {
    let next = current.map_or(0, |c| {
        CATEGORY_ORDER
            .iter()
            .position(|&x| x == c)
            .map_or(0, |i| i + 1)
    });
    CATEGORY_ORDER.get(next).map(|&s| s.to_string())
}

/// RFC 5322 `References`: the original's own `References` header (if any) plus its
/// `Message-ID`, so mail clients thread the reply under the whole chain, not just one message.
#[must_use]
pub fn chain_references(email: &EmailMessage) -> String {
    match &email.references_header {
        Some(refs) if !refs.is_empty() => format!("{refs} {}", email.message_id),
        _ => email.message_id.clone(),
    }
}

/// Assembles the outgoing body: the editable `body`, then the account's signature (if any), then
/// the quoted original (if replying/forwarding) — Outlook's own ordering.
///
/// Each part is separated by a blank line, with no leading blank line when an earlier part is
/// absent/empty. Shared by `send_compose` and the compose popup's live preview so the two can
/// never drift apart.
#[must_use]
pub fn compose_full_body(body: &str, signature: Option<&str>, quoted: Option<&str>) -> String {
    let mut out = body.to_string();
    for extra in [signature, quoted].into_iter().flatten() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(extra);
    }
    out
}

#[must_use]
pub fn quote_original(email: &EmailMessage) -> String {
    use std::fmt::Write as _;

    let who = email.from_name.as_deref().unwrap_or(&email.from_addr);
    let when = email.date_utc.format("%Y-%m-%d %H:%M UTC");
    let body = email.body_text.as_deref().unwrap_or("");
    let mut quoted_lines = String::new();
    for line in body.lines() {
        let _ = writeln!(quoted_lines, "> {line}");
    }
    format!("On {when}, {who} wrote:\n{quoted_lines}")
}

/// Merges the original's `to`/`cc` addresses for reply-all, dropping the reply's own `to`
/// (already covered), the sending account's own address, and duplicates.
#[must_use]
pub fn merge_reply_all_cc(email: &EmailMessage, to: &str, own_addr: &str) -> String {
    let mut addrs: Vec<String> = Vec::new();
    if let Some(cc) = &email.cc_addrs {
        addrs.extend(cc.split(',').map(str::trim).map(str::to_string));
    }
    if let Some(orig_to) = &email.to_addrs {
        addrs.extend(orig_to.split(',').map(str::trim).map(str::to_string));
    }
    addrs.retain(|a| {
        !a.is_empty() && !a.eq_ignore_ascii_case(to) && !a.eq_ignore_ascii_case(own_addr)
    });
    addrs.sort();
    addrs.dedup();
    addrs.join(", ")
}

/// Union-find over `Message-ID` strings, so a virtual ancestor id (referenced by a `References`
/// header but never itself fetched) still links its children into one component even though it
/// has no `EmailMessage` row of its own.
#[derive(Default)]
struct MsgIdForest {
    parent: HashMap<String, String>,
}

impl MsgIdForest {
    fn find(&mut self, key: &str) -> String {
        let Some(parent) = self.parent.get(key).cloned() else {
            return key.to_string();
        };
        if parent == key {
            return key.to_string();
        }
        let root = self.find(&parent);
        self.parent.insert(key.to_string(), root.clone());
        root
    }

    fn union(&mut self, a: &str, b: &str) {
        let root_a = self.find(a);
        let root_b = self.find(b);
        if root_a != root_b {
            self.parent.insert(root_a, root_b);
        }
    }
}

/// Strips repeated `Re:`/`Fwd:`/`Fw:` prefixes and lowercases, so replies/forwards group together.
///
/// Subject-based fallback for [`thread_count`], used only when no `References` header links the
/// message to anything else in `App::emails` — most first-in-thread messages, and mail from
/// clients that drop the header.
#[must_use]
pub fn normalize_subject(subject: &str) -> String {
    let mut s = subject.trim();
    loop {
        let lower = s.to_ascii_lowercase();
        let rest = if lower.starts_with("re:") {
            &s[3..]
        } else if lower.starts_with("fwd:") {
            &s[4..]
        } else if lower.starts_with("fw:") {
            &s[3..]
        } else {
            break;
        };
        s = rest.trim_start();
    }
    s.to_ascii_lowercase()
}

/// How many messages in `emails` are in `email_id`'s conversation, itself included.
///
/// Real `References`-header reconstruction (RFC 5322 §3.6.4) first: every email's own
/// `Message-ID` is unioned with each id in its `References` header, so a whole chain — including
/// ancestors never fetched into `emails` — collapses to one component. Falls back to
/// [`normalize_subject`] grouping only when the target has no header link to anything else
/// currently loaded (no `References` header at all, or every referenced id is likewise absent).
/// A blank normalized subject returns 1 in that fallback, rather than bucketing every subjectless
/// email into one giant false thread.
#[must_use]
pub fn thread_count(emails: &[EmailMessage], email_id: i64) -> usize {
    let Some(target) = emails.iter().find(|e| e.id == email_id) else {
        return 0;
    };

    let mut forest = MsgIdForest::default();
    for e in emails {
        if let Some(refs) = &e.references_header {
            for r in refs.split_whitespace() {
                forest.union(&e.message_id, r);
            }
        }
    }
    let target_root = forest.find(&target.message_id);
    let header_count = emails
        .iter()
        .filter(|e| forest.find(&e.message_id) == target_root)
        .count();
    if header_count > 1 {
        return header_count;
    }

    let key = normalize_subject(&target.subject);
    if key.is_empty() {
        return 1;
    }
    emails
        .iter()
        .filter(|e| normalize_subject(&e.subject) == key)
        .count()
}

/// Parses a snooze spec typed after `z` in the email list into an absolute UTC instant.
///
/// Supported: `<N>m`/`<N>h`/`<N>d` (minutes/hours/days from `now`, `N` a positive integer) and
/// the keywords `tomorrow`/`nextweek` (8am local time, one/seven days out respectively — DST-safe
/// via [`resolve_local_datetime`]). `None` for anything else.
#[must_use]
pub fn parse_snooze_spec(spec: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    match spec {
        "tomorrow" => return snooze_to_local_morning(1),
        "nextweek" => return snooze_to_local_morning(7),
        _ => {}
    }
    let split = spec.len().checked_sub(1)?;
    let (count, unit) = spec.split_at(split);
    let count: i64 = count.parse().ok()?;
    if count <= 0 {
        return None;
    }
    let delta = match unit {
        "m" => Duration::minutes(count),
        "h" => Duration::hours(count),
        "d" => Duration::days(count),
        _ => return None,
    };
    Some(now + delta)
}

/// 8am local time, `days_ahead` days from today — the shared target for `tomorrow`/`nextweek`.
fn snooze_to_local_morning(days_ahead: i64) -> Option<DateTime<Utc>> {
    let date = chrono::Local::now().naive_local().date() + Duration::days(days_ahead);
    let naive = date.and_hms_opt(8, 0, 0)?;
    Some(resolve_local_datetime(naive))
}

/// Tests whether `rule` matches a message's subject or from-address.
///
/// `match_field` selects which (`"from_addr"`, else `"subject"`); the test itself is a
/// case-insensitive substring, not a regex — keeps rule specs typeable in one line
/// (see `parse_rule_spec`).
#[must_use]
pub fn match_rule(rule: &EmailRule, subject: &str, from_addr: &str) -> bool {
    let haystack = if rule.match_field == "from_addr" {
        from_addr
    } else {
        subject
    };
    haystack
        .to_lowercase()
        .contains(&rule.pattern.to_lowercase())
}

/// Parses a rule spec typed in the rules popup's `n` prompt: `<field> <pattern...> <action>`.
///
/// E.g. `subject newsletter star` or `from noreply archive`. `field` is `subject` or
/// `from`/`sender` (mapped to the stored `from_addr`); `action` is `star`, `read`, `archive` or
/// `delete` (Slice 21 added the latter two — see `apply_rule_action`); `pattern` is everything in
/// between (may contain spaces). `None` for anything that doesn't fit this shape.
#[must_use]
pub fn parse_rule_spec(spec: &str) -> Option<(String, String, String)> {
    let mut parts = spec.trim().splitn(2, char::is_whitespace);
    let field = parts.next()?.to_lowercase();
    let rest = parts.next()?.trim();

    let match_field = match field.as_str() {
        "subject" => "subject",
        "from" | "sender" | "from_addr" => "from_addr",
        _ => return None,
    };

    let split = rest.rfind(char::is_whitespace)?;
    let (pattern, action) = (rest[..split].trim(), rest[split..].trim());
    if pattern.is_empty() {
        return None;
    }
    let action = match action.to_lowercase().as_str() {
        "star" => "star",
        "read" => "read",
        "archive" => "archive",
        "delete" => "delete",
        _ => return None,
    };

    Some((
        match_field.to_string(),
        pattern.to_string(),
        action.to_string(),
    ))
}
