use anyhow::{Context, Result};
use futures::TryStreamExt;
use std::future::Future;
use std::sync::{Arc, Once};
use tokio::net::TcpStream;
use tokio::time::Duration;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

use super::config::EmailConfig;
use super::store::SyncCursor;

static INIT_CRYPTO_PROVIDER: Once = Once::new();

/// Rustls 0.23 requires a process-wide default `CryptoProvider`. Both `ring` and
/// `aws-lc-rs` end up enabled in this workspace's dependency tree (pulled in by
/// different crates), which makes rustls' own auto-detection ambiguous and panic.
/// Install one explicitly, once; if something else (e.g. sqlx) already installed a
/// default first, this is a no-op.
fn ensure_crypto_provider() {
    INIT_CRYPTO_PROVIDER.call_once(|| {
        let _ = tokio_rustls::rustls::crypto::ring::default_provider().install_default();
    });
}

/// First-sync cap: when there's no prior `since_uid` to resume from, fetch only the
/// most recent N messages instead of the entire mailbox history. Full-body IMAP
/// fetches of a real inbox's entire history can be extremely slow and memory-heavy.
const INITIAL_SYNC_LIMIT: usize = 25;

/// Messages larger than this skip the full `RFC822` fetch (headers + body + every
/// MIME part, attachments included) and get `RFC822.HEADER` only instead — enough
/// for from/subject/date/message-id, no body. Observed live: a handful of
/// attachment-heavy messages in the 1-5MB range turned a routine sync into a
/// multi-megabyte download for content nothing here ever stores (`message.rs` only
/// keeps a plaintext snippet + body, never the raw MIME).
const LARGE_MESSAGE_BYTES: u32 = 1_048_576;

/// Upper bound on one `fetch_new` call (connect through logout). Each sync opens a
/// fresh TCP connection with no read timeout of its own, so a server that stops
/// responding mid-handshake or mid-fetch would otherwise hang the awaiting task
/// forever — observed live during this fix's own verification run. `mail_sync_worker`
/// awaits this sequentially per account inside one `tokio::select!` branch, so an
/// unbounded hang here doesn't just fail one account: it blocks every future tick for
/// every account, and the task never returns to poll `shutdown_rx` either.
const FETCH_TIMEOUT: Duration = Duration::from_secs(300);

/// `(uid, raw_bytes, header_only)` for one fetched message. `header_only` is true
/// when the message exceeded [`LARGE_MESSAGE_BYTES`] and `raw_bytes` is just its
/// `RFC822.HEADER` (no body) rather than the full `RFC822`.
type RawMessage = (u32, Vec<u8>, bool);

/// Abstraction over where new mail comes from, so alternative auth (`OAuth2`, other
/// providers) can slot in later without touching callers.
pub trait MailSource: Send + Sync {
    /// Returns the mailbox's current UIDVALIDITY plus every new message. IMAP only
    /// guarantees UIDs are stable within one UIDVALIDITY epoch (RFC 3501) — Gmail,
    /// e.g., can change it without any user action, silently invalidating every
    /// previously-stored UID. If the server's current UIDVALIDITY doesn't match
    /// `since.uid_validity`, `since.last_uid` is ignored and this behaves like a
    /// first sync (capped to [`INITIAL_SYNC_LIMIT`]) instead of resuming from a UID
    /// that may no longer mean what it used to.
    fn fetch_new(
        &self,
        since: Option<SyncCursor>,
    ) -> impl Future<Output = Result<(Option<u32>, Vec<RawMessage>)>> + Send;
}

pub struct ImapMailSource {
    config: EmailConfig,
}

impl ImapMailSource {
    pub const fn new(config: EmailConfig) -> Self {
        Self { config }
    }
}

impl MailSource for ImapMailSource {
    async fn fetch_new(&self, since: Option<SyncCursor>) -> Result<(Option<u32>, Vec<RawMessage>)> {
        match tokio::time::timeout(FETCH_TIMEOUT, self.fetch_new_inner(since)).await {
            Ok(result) => result,
            Err(_) => anyhow::bail!(
                "IMAP sync for '{}' timed out after {}s",
                self.config.account,
                FETCH_TIMEOUT.as_secs()
            ),
        }
    }
}

impl ImapMailSource {
    // One linear IMAP protocol sequence (connect, TLS, login, select, search,
    // fetch) - splitting it into helpers would scatter that sequence across
    // functions without reducing its actual complexity.
    #[allow(clippy::too_many_lines)]
    async fn fetch_new_inner(
        &self,
        since: Option<SyncCursor>,
    ) -> Result<(Option<u32>, Vec<RawMessage>)> {
        ensure_crypto_provider();
        let account = &self.config.account;

        tracing::debug!(
            "[Mail:{account}] connecting to {}:{}",
            self.config.imap_server, self.config.imap_port
        );
        let tcp = TcpStream::connect((self.config.imap_server.as_str(), self.config.imap_port))
            .await
            .context("failed to connect to IMAP server")?;

        let root_store = build_root_store()?;
        let tls_config = ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(tls_config));
        let domain = ServerName::try_from(self.config.imap_server.clone())
            .context("invalid IMAP server hostname")?;
        tracing::debug!("[Mail:{account}] TLS handshake");
        let tls_stream = connector
            .connect(domain, tcp)
            .await
            .context("TLS handshake with IMAP server failed")?;

        let mut client = async_imap::Client::new(tls_stream);
        tracing::debug!("[Mail:{account}] waiting for IMAP greeting");
        let _greeting = client
            .read_response()
            .await
            .context("failed to read IMAP greeting")?;

        tracing::debug!(
            "[Mail:{account}] logging in as {}",
            self.config.imap_username
        );
        let mut session = client
            .login(&self.config.imap_username, &self.config.imap_password)
            .await
            .map_err(|(err, _client)| err)
            .context("IMAP login failed")?;

        tracing::debug!(
            "[Mail:{account}] selecting folder {}",
            self.config.imap_folder
        );
        let mailbox = session
            .select(&self.config.imap_folder)
            .await
            .context("failed to select IMAP folder")?;
        let current_uid_validity = mailbox.uid_validity;

        // Only trust the stored `uid` if the server's UIDVALIDITY still matches the
        // one it had last time we stored it. If the server didn't report a
        // UIDVALIDITY at all (non-compliant server), fall back to trusting `since`
        // as before rather than forcing an unnecessary re-catch-up.
        let since_uid = current_uid_validity.map_or_else(
            || since.map(|cursor| u32::try_from(cursor.last_uid).unwrap_or(0)),
            |validity| {
                since.and_then(|cursor| {
                    // `cursor.uid_validity`/`cursor.last_uid` were themselves stored
                    // from `u32`s (see `store.rs`), so this round-trip always fits.
                    (u32::try_from(cursor.uid_validity).unwrap_or(0) == validity)
                        .then_some(u32::try_from(cursor.last_uid).unwrap_or(0))
                })
            },
        );

        let search_query = since_uid
            .map_or_else(|| "ALL".to_string(), |uid| format!("UID {}:*", uid + 1));

        tracing::debug!("[Mail:{account}] UID search: {search_query}");
        let uids = session
            .uid_search(&search_query)
            .await
            .context("IMAP UID search failed")?;

        let mut result = Vec::new();

        if !uids.is_empty() {
            let mut sorted: Vec<u32> = uids.into_iter().collect();
            sorted.sort_unstable();
            // `since_uid == Some(0)` means "no messages seen yet" (a fresh cursor row, or an
            // epoch change that lands on last_uid=0) — just as much a first sync as `None`.
            // Only checking `is_none()` here let a stale/edge-case last_uid=0 cursor bypass the
            // cap entirely, fetching the whole mailbox (observed live: 30k+ messages, several
            // multi-MB) instead of the intended catch-up window.
            let is_first_sync = since_uid.is_none_or(|uid| uid == 0);
            if is_first_sync && sorted.len() > INITIAL_SYNC_LIMIT {
                sorted = sorted.split_off(sorted.len() - INITIAL_SYNC_LIMIT);
            }
            let uid_set = sorted
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",");

            // Cheap pre-check: sizes only, no bodies. Splits the batch so oversized
            // messages (attachments) don't cost a full RFC822 download below.
            tracing::debug!("[Mail:{account}] checking size of {} message(s)", sorted.len());
            let mut size_stream = session
                .uid_fetch(&uid_set, "(RFC822.SIZE)")
                .await
                .context("IMAP UID FETCH (size) failed")?;
            let mut sizes: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
            while let Some(fetch) = size_stream
                .try_next()
                .await
                .context("error reading IMAP size response")?
            {
                if let (Some(uid), Some(size)) = (fetch.uid, fetch.size) {
                    sizes.insert(uid, size);
                }
            }
            drop(size_stream);

            let mut small_uids = Vec::new();
            let mut large_uids = Vec::new();
            for uid in &sorted {
                if sizes.get(uid).is_some_and(|&size| size > LARGE_MESSAGE_BYTES) {
                    large_uids.push(*uid);
                } else {
                    small_uids.push(*uid);
                }
            }

            if !small_uids.is_empty() {
                let uid_set = small_uids
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                tracing::debug!(
                    "[Mail:{account}] fetching {} full message(s): {uid_set}",
                    small_uids.len()
                );
                let mut stream = session
                    .uid_fetch(&uid_set, "RFC822")
                    .await
                    .context("IMAP UID FETCH failed")?;

                while let Some(fetch) = stream
                    .try_next()
                    .await
                    .context("error reading IMAP fetch response")?
                {
                    if let (Some(uid), Some(body)) = (fetch.uid, fetch.body()) {
                        tracing::debug!("[Mail:{account}] got UID {uid} ({} bytes)", body.len());
                        result.push((uid, body.to_vec(), false));
                    }
                }
            }

            if !large_uids.is_empty() {
                let uid_set = large_uids
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                tracing::debug!(
                    "[Mail:{account}] fetching {} oversized message header(s) only (>{LARGE_MESSAGE_BYTES} bytes): {uid_set}",
                    large_uids.len()
                );
                let mut stream = session
                    .uid_fetch(&uid_set, "RFC822.HEADER")
                    .await
                    .context("IMAP UID FETCH (header) failed")?;

                while let Some(fetch) = stream
                    .try_next()
                    .await
                    .context("error reading IMAP fetch response")?
                {
                    if let (Some(uid), Some(header)) = (fetch.uid, fetch.header()) {
                        tracing::debug!(
                            "[Mail:{account}] got UID {uid} header only ({} bytes)",
                            header.len()
                        );
                        result.push((uid, header.to_vec(), true));
                    }
                }
            }
        }

        let _ = session.logout().await;

        Ok((current_uid_validity, result))
    }
}

fn build_root_store() -> Result<RootCertStore> {
    let mut store = RootCertStore::empty();
    let native = rustls_native_certs::load_native_certs();
    let (added, _skipped) = store.add_parsable_certificates(native.certs);

    if added == 0 {
        anyhow::bail!("no usable native root certificates found");
    }

    Ok(store)
}
