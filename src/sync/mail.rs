use anyhow::Result;
use sqlx::SqlitePool;
use tokio::sync::broadcast;
use tokio::time::Duration;

use crate::email::{EmailConfig, IdleOutcome, ImapMailSource, MailSource, sync::sync_account};

/// How long one IDLE call blocks before giving up and running a sync pass anyway, in case the
/// server never pushes anything unsolicited (some don't, silently) — comfortably under the
/// 29-minute server-side "renew IDLE or get logged off" limit `async-imap` itself already guards
/// via `Handle::wait`. This is this module's own backstop poll cadence, same role the old fixed
/// 60s `interval` used to play, just triggered early whenever the server actually pushes.
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Runs one push-based (IMAP IDLE) sync loop per configured account, concurrently — a single
/// account's IDLE call blocks its own connection for up to [`IDLE_TIMEOUT`], so accounts must not
/// share one sequential loop the way the old fixed-interval poller did, or a slow/idle account
/// would delay every other account's sync just as long.
pub async fn mail_sync_worker(
    db: SqlitePool,
    mut shutdown_rx: broadcast::Receiver<()>,
) -> Result<()> {
    let configs = EmailConfig::all_from_env();
    if configs.is_empty() {
        tracing::info!(
            "[Mail] no IMAP accounts configured (TRIPTYCH_EMAIL_ENABLED/IMAP_*), mail sync disabled"
        );
        return Ok(());
    }

    let tasks: Vec<_> = configs
        .into_iter()
        .map(|config| {
            tokio::spawn(account_sync_loop(
                db.clone(),
                config,
                shutdown_rx.resubscribe(),
            ))
        })
        .collect();

    let _ = shutdown_rx.recv().await;
    for task in tasks {
        let _ = task.await;
    }

    Ok(())
}

/// One account's loop: an initial sync, then repeatedly IDLE-wait on `config.imap_folder` and run
/// another sync pass whenever that returns — on real push data, on the [`IDLE_TIMEOUT`] backstop,
/// or on an IDLE failure (network hiccup, server without IDLE support, etc.), all three treated the
/// same way since a sync pass is always safe to run and cheap to skip if nothing changed. Every
/// pass here passes `include_archive: false` — see `sync_account`'s doc for why an ambient,
/// IDLE-driven pass must never sweep the archive folder on its own (it would rediscover a message
/// this same IDLE wake just watched get archived, and silently reinsert it).
async fn account_sync_loop(
    db: SqlitePool,
    config: EmailConfig,
    mut shutdown_rx: broadcast::Receiver<()>,
) {
    let source = ImapMailSource::new(config.clone());

    // `include_archive: false` — see `sync_account`'s doc for why the background worker must
    // never sweep the archive folder on its own.
    if let Err(e) = sync_account(&db, &config, false).await {
        tracing::warn!("[Mail] sync failed for account '{}': {}", config.account, e);
    }

    loop {
        tokio::select! {
            _ = shutdown_rx.recv() => break,

            outcome = source.idle_wait(&config.imap_folder, IDLE_TIMEOUT) => {
                match outcome {
                    Ok(IdleOutcome::NewData) => {
                        tracing::debug!("[Mail:{}] IDLE reported new data", config.account);
                    }
                    Ok(IdleOutcome::Timeout) => {
                        tracing::debug!("[Mail:{}] IDLE backstop elapsed, syncing anyway", config.account);
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[Mail:{}] IDLE failed, falling back to a plain sync pass: {}",
                            config.account, e
                        );
                    }
                }

                if let Err(e) = sync_account(&db, &config, false).await {
                    tracing::warn!("[Mail] sync failed for account '{}': {}", config.account, e);
                }
            }
        }
    }
}
