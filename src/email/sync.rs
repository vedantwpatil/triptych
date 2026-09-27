//! One fetch, parse and store pass for a single account. The background poller, `triptych email
//! sync` and the Email view's `s` key all call it, so the cursor rules live in one place.

use anyhow::Result;
use sqlx::SqlitePool;

use super::store::{self, SyncCursor};
use super::{EmailConfig, ImapMailSource, MailSource, message};

/// What one pass did.
#[derive(Debug, Clone, Copy, Default)]
pub struct SyncReport {
    /// Messages that were not stored yet.
    pub new: u64,
    /// The server renumbered its UIDs, so this pass re-fetched recent mail instead of resuming.
    pub epoch_changed: bool,
}

/// Pulls new mail for `config` into the database and moves its sync cursor forward. Syncs
/// `imap_folder` (normally `INBOX`) as before, then, if `include_archive`, also `archive_folder`
/// so mail moved out of the inbox by `App::archive_selected_email` stays visible in Triptych
/// instead of vanishing once it leaves the tracked folder (Slice 13: folder browsing). Each folder
/// has its own resume cursor (`email_sync_state` is keyed `(account, folder)`), so the two passes
/// don't interfere.
///
/// `include_archive` must be `false` for any *ambient* caller (the background push/poll worker)
/// and `true` for any *explicit* one (the `s` key, view entry, `triptych email sync`). An archive
/// action removes its message from `imap_folder`, which is exactly the kind of change an open IMAP
/// IDLE connection on that folder is watching for — so the background worker wakes right as an
/// archive completes, essentially every time. If that wake also swept `archive_folder`, it would
/// rediscover the message the user just archived (now legitimately new to that folder's own
/// cursor) and silently reinsert the row `archive_selected_email` just deleted. Scoping the
/// archive-folder pass to explicit callers only keeps that resync a deliberate, user-visible act
/// (the whole point of folder browsing) rather than an ambient side effect of push-based sync.
///
/// # Errors
///
/// Returns an error if `imap_folder`'s fetch or a database write fails. A failure syncing
/// `archive_folder` alone (e.g. the server has no such folder, or it hasn't been created yet) is
/// logged and swallowed rather than failing the whole call — it must not regress accounts that
/// worked fine before this folder was tracked.
pub async fn sync_account(
    pool: &SqlitePool,
    config: &EmailConfig,
    include_archive: bool,
) -> Result<SyncReport> {
    let mut total = sync_folder(pool, config, &config.imap_folder).await?;

    if include_archive && config.archive_folder != config.imap_folder {
        match sync_folder(pool, config, &config.archive_folder).await {
            Ok(report) => {
                total.new += report.new;
                total.epoch_changed |= report.epoch_changed;
            }
            Err(e) => tracing::warn!(
                "[Mail] archive-folder sync failed for account '{}': {e}",
                config.account
            ),
        }
    }

    Ok(total)
}

/// Syncs exactly one folder for `config`, ignoring `imap_folder`/`archive_folder` entirely — used
/// by folder browsing (`App::browse_to_selected_folder`) to pull in a server folder discovered via
/// `MailSource::list_folders` that isn't already tracked by either of those two. Same cursor,
/// parse and store pipeline as `sync_account`'s own per-folder pass.
///
/// # Errors
///
/// Returns an error if the fetch or a database write fails.
pub async fn sync_one_folder(
    pool: &SqlitePool,
    config: &EmailConfig,
    folder: &str,
) -> Result<SyncReport> {
    sync_folder(pool, config, folder).await
}

async fn sync_folder(pool: &SqlitePool, config: &EmailConfig, folder: &str) -> Result<SyncReport> {
    let cursor = store::get_sync_cursor(pool, &config.account, folder).await?;
    let (uid_validity, raw_messages) = ImapMailSource::new(config.clone())
        .fetch_new(folder, cursor)
        .await?;

    let epoch_changed = match (cursor, uid_validity) {
        // `c.uid_validity` was itself stored from a `u32` (see `client.rs`), so this round-trip
        // always fits.
        (Some(c), Some(current)) => u32::try_from(c.uid_validity).unwrap_or(0) != current,
        _ => false,
    };
    let fetched_max_uid = raw_messages.iter().map(|(uid, _, _)| *uid).max();

    let new_emails: Vec<_> = raw_messages
        .into_iter()
        .filter_map(|(uid, raw, header_only)| {
            message::parse_raw(&config.account, uid, folder, &raw, header_only).ok()
        })
        .collect();
    let new = store::insert_new(pool, &new_emails).await?;

    // If the epoch changed and nothing came back, don't persist a synthetic `last_uid = 0`: the
    // next search would be `UID 1:*`, which is not capped by `INITIAL_SYNC_LIMIT` the way a
    // first sync's `ALL` is. Keeping the stale cursor makes the next pass retry the capped catch-up.
    if let Some(validity) = uid_validity
        && !(epoch_changed && fetched_max_uid.is_none())
    {
        let prior_uid = if epoch_changed {
            0
        } else {
            cursor.map_or(0, |c| c.last_uid)
        };
        let last_uid = fetched_max_uid.map_or(prior_uid, |uid| i64::from(uid).max(prior_uid));
        let cursor = SyncCursor {
            uid_validity: i64::from(validity),
            last_uid,
        };
        store::set_sync_cursor(pool, &config.account, folder, cursor).await?;
    }

    Ok(SyncReport { new, epoch_changed })
}
