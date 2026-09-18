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

/// Upper bound on one `fetch_new` call (connect through logout). Each sync opens a
/// fresh TCP connection with no read timeout of its own, so a server that stops
/// responding mid-handshake or mid-fetch would otherwise hang the awaiting task
/// forever — observed live during this fix's own verification run. `mail_sync_worker`
/// awaits this sequentially per account inside one `tokio::select!` branch, so an
/// unbounded hang here doesn't just fail one account: it blocks every future tick for
/// every account, and the task never returns to poll `shutdown_rx` either.
const FETCH_TIMEOUT: Duration = Duration::from_secs(120);

/// `(uid, raw_rfc822_bytes)` for one fetched message.
type RawMessage = (u32, Vec<u8>);

/// Abstraction over where new mail comes from, so alternative auth (OAuth2, other
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
    pub fn new(config: EmailConfig) -> Self {
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
    async fn fetch_new_inner(&self, since: Option<SyncCursor>) -> Result<(Option<u32>, Vec<RawMessage>)> {
        ensure_crypto_provider();

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
        let tls_stream = connector
            .connect(domain, tcp)
            .await
            .context("TLS handshake with IMAP server failed")?;

        let mut client = async_imap::Client::new(tls_stream);
        let _greeting = client
            .read_response()
            .await
            .context("failed to read IMAP greeting")?;

        let mut session = client
            .login(&self.config.imap_username, &self.config.imap_password)
            .await
            .map_err(|(err, _client)| err)
            .context("IMAP login failed")?;

        let mailbox = session
            .select(&self.config.imap_folder)
            .await
            .context("failed to select IMAP folder")?;
        let current_uid_validity = mailbox.uid_validity;

        // Only trust the stored `uid` if the server's UIDVALIDITY still matches the
        // one it had last time we stored it. If the server didn't report a
        // UIDVALIDITY at all (non-compliant server), fall back to trusting `since`
        // as before rather than forcing an unnecessary re-catch-up.
        let since_uid = match current_uid_validity {
            Some(validity) => since.and_then(|cursor| {
                (cursor.uid_validity as u32 == validity).then_some(cursor.last_uid as u32)
            }),
            None => since.map(|cursor| cursor.last_uid as u32),
        };

        let search_query = match since_uid {
            Some(uid) => format!("UID {}:*", uid + 1),
            None => "ALL".to_string(),
        };

        let uids = session
            .uid_search(&search_query)
            .await
            .context("IMAP UID search failed")?;

        let mut result = Vec::new();

        if !uids.is_empty() {
            let mut sorted: Vec<u32> = uids.into_iter().collect();
            sorted.sort_unstable();
            if since_uid.is_none() && sorted.len() > INITIAL_SYNC_LIMIT {
                sorted = sorted.split_off(sorted.len() - INITIAL_SYNC_LIMIT);
            }
            let uid_set = sorted
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",");

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
                    result.push((uid, body.to_vec()));
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
