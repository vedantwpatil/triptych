//! Email view: retention purge, account sync, refresh, read/open and convert-to-task.

use chrono::{Duration, Utc};

use super::App;
use crate::email::{EmailConfig, ImapMailSource, MailSource, message, store as email_store};

/// Email retention window for [`App::cleanup_old_emails`]. ~6 months, expressed
/// as days rather than calendar months to sidestep invalid-date edge cases
/// (e.g. Aug 31 minus 1 month = Feb 31) — the window isn't meant to be exact to
/// the day.
const EMAIL_RETENTION_DAYS: i64 = 180;

impl App {
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

    /// Kicks off a background pull of new mail from IMAP for every configured
    /// account, so the Email view catches up sooner than the next 60s
    /// `src/sync/mail.rs` poll instead of waiting on it. Fire-and-forget
    /// (`tokio::spawn`, not awaited) rather than the blocking call this used
    /// to be: run inline, a slow/unreachable IMAP server stalled the whole
    /// TUI (no redraw, no key input) until every account's TCP+TLS round-trip
    /// finished or failed. No-ops silently if email isn't configured;
    /// per-account failures are logged to stderr, same as the background
    /// poller, since there's no `&mut self` left to post a `status_message` to
    /// once the task is spawned.
    pub(super) fn sync_email_accounts(&self) {
        let configs = EmailConfig::all_from_env();
        if configs.is_empty() {
            return;
        }

        let db_pool = self.db_pool.clone();
        tokio::spawn(async move {
            for config in &configs {
                let cursor = match email_store::get_sync_cursor(
                    &db_pool,
                    &config.account,
                    &config.imap_folder,
                )
                .await
                {
                    Ok(cursor) => cursor,
                    Err(e) => {
                        tracing::warn!("[Email] sync failed for '{}': {}", config.account, e);
                        continue;
                    }
                };
                let source = ImapMailSource::new(config.clone());
                match source.fetch_new(cursor).await {
                    Ok((uid_validity, raw_messages)) => {
                        let epoch_changed = match (cursor, uid_validity) {
                            // `c.uid_validity` was itself stored from a `u32` (see
                            // `client.rs`), so this round-trip always fits.
                            (Some(c), Some(current)) => {
                                u32::try_from(c.uid_validity).unwrap_or(0) != current
                            }
                            _ => false,
                        };
                        if epoch_changed {
                            tracing::info!(
                                "[Email] UIDVALIDITY changed for '{}'; resyncing recent mail instead of resuming",
                                config.account
                            );
                        }

                        let fetched_max_uid = raw_messages.iter().map(|(uid, _, _)| *uid).max();

                        let new_emails: Vec<_> = raw_messages
                            .into_iter()
                            .filter_map(|(uid, raw, header_only)| {
                                message::parse_raw(
                                    &config.account,
                                    uid,
                                    &config.imap_folder,
                                    &raw,
                                    header_only,
                                )
                                .ok()
                            })
                            .collect();
                        if let Err(e) = email_store::insert_new(&db_pool, &new_emails).await {
                            tracing::warn!("[Email] sync failed for '{}': {}", config.account, e);
                        }
                        // See sync/mail.rs's sync_mail: skip persisting a synthetic
                        // `last_uid = 0` when the epoch changed but nothing came back, so
                        // the next sync retries the properly-capped catch-up.
                        if let Some(validity) = uid_validity
                            && !(epoch_changed && fetched_max_uid.is_none())
                        {
                            let prior_uid = if epoch_changed {
                                0
                            } else {
                                cursor.map_or(0, |c| c.last_uid)
                            };
                            let last_uid = fetched_max_uid
                                .map_or(prior_uid, |uid| i64::from(uid).max(prior_uid));
                            if let Err(e) = email_store::set_sync_cursor(
                                &db_pool,
                                &config.account,
                                &config.imap_folder,
                                email_store::SyncCursor {
                                    uid_validity: i64::from(validity),
                                    last_uid,
                                },
                            )
                            .await
                            {
                                tracing::warn!(
                                    "[Email] sync failed for '{}': {}",
                                    config.account,
                                    e
                                );
                            }
                        }
                    }
                    Err(e) => tracing::warn!("[Email] sync failed for '{}': {}", config.account, e),
                }
            }
        });
    }

    /// Reloads the newest stored emails into the Email view.
    ///
    /// # Errors
    ///
    /// Returns an error if a database query fails.
    pub async fn refresh_emails(&mut self) -> Result<(), sqlx::Error> {
        self.emails = email_store::get_recent(&self.db_pool, 100)
            .await
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;

        if self.selected_email >= self.emails.len() {
            self.selected_email = self.emails.len().saturating_sub(1);
        }
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
        Ok(())
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
