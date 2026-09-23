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

/// Pulls new mail for `config` into the database and moves its sync cursor forward.
///
/// # Errors
///
/// Returns an error if the fetch or a database write fails; nothing is stored after a failed fetch.
pub async fn sync_account(pool: &SqlitePool, config: &EmailConfig) -> Result<SyncReport> {
    let cursor = store::get_sync_cursor(pool, &config.account, &config.imap_folder).await?;
    let (uid_validity, raw_messages) = ImapMailSource::new(config.clone())
        .fetch_new(cursor)
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
            message::parse_raw(&config.account, uid, &config.imap_folder, &raw, header_only).ok()
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
        store::set_sync_cursor(pool, &config.account, &config.imap_folder, cursor).await?;
    }

    Ok(SyncReport { new, epoch_changed })
}
