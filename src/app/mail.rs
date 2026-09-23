//! Email view: retention purge, account sync, refresh, read/open and convert-to-task.

use std::sync::Arc;

use chrono::{Duration, Utc};

use super::{App, ViewMode};
use crate::email::{EmailConfig, EmailSort, priority, store as email_store, sync::sync_account};

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

/// Emails with a shorter body than this are not worth summarizing.
const SUMMARY_MIN_CHARS: usize = 200;
/// Cap on the body text sent to the model, so one huge message cannot stall the prompt.
const SUMMARY_INPUT_CHARS: usize = 4000;

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
                match sync_account(&db_pool, config).await {
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
        self.email_detail_open = true;
        self.email_detail_scroll = 0;
        self.mark_selected_email_read().await?;

        let body = crate::email::store::get_body(&self.db_pool, email_id)
            .await
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        if let Some(email) = self.emails.iter_mut().find(|e| e.id == email_id) {
            email.body_text = body;
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
            self.email_summaries.insert(email_id, Summary::Ready(cached));
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
                if let Err(e) = email_store::set_summary(&self.db_pool, done.email_id, &text).await {
                    tracing::warn!("[Email] could not cache a summary: {e}");
                }
                Summary::Ready(text)
            }
            Err(reason) => Summary::Failed(reason),
        };
        self.email_summaries.insert(done.email_id, state);
    }

    pub const fn close_email_detail(&mut self) {
        self.email_detail_open = false;
        self.email_detail_scroll = 0;
    }

    /// Starts creating a task from the selected email's subject (parsed in the background, see
    /// `App::submit_task`); `finish_email_conversion` then links the two.
    pub fn convert_selected_email_to_task(&mut self) {
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
        self.submit_task(subject, Some(email_id));
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
}
