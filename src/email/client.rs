use anyhow::{Context, Result};
use futures::TryStreamExt;
use mail_parser::MimeHeaders;
use std::future::Future;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::time::Duration;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::ClientConfig;
use tokio_rustls::rustls::pki_types::ServerName;

use super::config::EmailConfig;
use super::store::SyncCursor;
use super::tls::{build_root_store, ensure_crypto_provider};

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

/// Upper bound on one `delete` call (connect through logout) — much shorter than
/// [`FETCH_TIMEOUT`] since a delete is one STORE and one EXPUNGE, not a bulk fetch.
const DELETE_TIMEOUT: Duration = Duration::from_secs(30);

/// Upper bound on one `archive` call — same budget as [`DELETE_TIMEOUT`], since the
/// MOVE path is one command and the COPY/STORE/EXPUNGE fallback is the same shape as
/// delete plus one extra COPY.
const ARCHIVE_TIMEOUT: Duration = Duration::from_secs(30);

/// Upper bound on one `fetch_attachments` call. Unlike delete/archive this refetches the whole
/// `RFC822` message (there's no cheap "just this MIME part" IMAP fetch item this client uses), so
/// it gets [`FETCH_TIMEOUT`]'s budget rather than [`DELETE_TIMEOUT`]'s.
const FETCH_ATTACHMENT_TIMEOUT: Duration = FETCH_TIMEOUT;

/// Extra budget on top of the caller's own `idle_wait` timeout, covering just the
/// connect/login/select portion before `IDLE` itself is even sent — same rationale as
/// [`FETCH_TIMEOUT`], applied to a call whose overall bound is caller-supplied data rather than a
/// fixed constant, so it can't reuse `FETCH_TIMEOUT` directly.
const IDLE_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Upper bound on one `list_folders` call — same budget as [`DELETE_TIMEOUT`]/[`ARCHIVE_TIMEOUT`],
/// since `LIST` is one command against the whole account, not a per-message operation.
const LIST_TIMEOUT: Duration = Duration::from_secs(30);

/// What one [`MailSource::idle_wait`] call found out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleOutcome {
    /// The server pushed at least one unsolicited response (new mail, a flag change, etc.) —
    /// the caller should run its normal sync pass to find out what actually changed, since IDLE
    /// itself only proves *something* happened, not what.
    NewData,
    /// No push arrived before the timeout elapsed. Not an error — IDLE is best-effort, some
    /// servers push nothing — so the caller should still run a normal sync pass, same as it
    /// would on a plain poll tick.
    Timeout,
}

/// `(uid, raw_bytes, header_only)` for one fetched message. `header_only` is true
/// when the message exceeded [`LARGE_MESSAGE_BYTES`] and `raw_bytes` is just its
/// `RFC822.HEADER` (no body) rather than the full `RFC822`.
type RawMessage = (u32, Vec<u8>, bool);

/// Abstraction over where new mail comes from, so alternative auth (`OAuth2`, other
/// providers) can slot in later without touching callers.
pub trait MailSource: Send + Sync {
    /// Selects `folder` and returns its current UIDVALIDITY plus every new message. IMAP only
    /// guarantees UIDs are stable within one UIDVALIDITY epoch (RFC 3501) — Gmail,
    /// e.g., can change it without any user action, silently invalidating every
    /// previously-stored UID. If the server's current UIDVALIDITY doesn't match
    /// `since.uid_validity`, `since.last_uid` is ignored and this behaves like a
    /// first sync (capped to [`INITIAL_SYNC_LIMIT`]) instead of resuming from a UID
    /// that may no longer mean what it used to.
    fn fetch_new(
        &self,
        folder: &str,
        since: Option<SyncCursor>,
    ) -> impl Future<Output = Result<(Option<u32>, Vec<RawMessage>)>> + Send;

    /// Permanently removes one message from `folder` (the folder it's actually stored under —
    /// see `EmailMessage::folder`): `UID STORE +FLAGS.SILENT (\Deleted)` then `EXPUNGE`. No
    /// move-to-Trash — unlike most mail clients' delete key, this cannot be undone from within
    /// Triptych.
    fn delete(&self, folder: &str, uid: u32) -> impl Future<Output = Result<()>> + Send;

    /// Moves one message from `folder` to `EmailConfig::archive_folder`. Tries
    /// RFC 6851 `UID MOVE` first; if the server doesn't support it, falls back to
    /// `UID COPY` + `UID STORE +FLAGS.SILENT (\Deleted)` + `EXPUNGE`. Does not create the
    /// destination folder — a missing archive folder is a real error, not auto-fixed.
    fn archive(&self, folder: &str, uid: u32) -> impl Future<Output = Result<()>> + Send;

    /// Re-fetches one message's full `RFC822` body (from `folder`, the folder it's actually
    /// stored under) and returns every attachment's filename (`None` if the part had none) and
    /// raw bytes, in the same order `message.rs::parse_raw` enumerates them — attachment bytes
    /// are never persisted, so this is the only way to get them back. One connect fetches all of
    /// a message's attachments, not one connect each.
    fn fetch_attachments(
        &self,
        folder: &str,
        uid: u32,
    ) -> impl Future<Output = Result<Vec<(Option<String>, Vec<u8>)>>> + Send;

    /// Opens IDLE (RFC 2177) on `folder` and blocks until the server pushes an unsolicited
    /// response or `timeout` elapses, whichever first — a push-based alternative to polling
    /// `fetch_new` on a fixed interval. Uses its own fresh connection like every other method
    /// here, so a long-blocking call never holds up a concurrent `fetch_new`/`delete`/etc. on a
    /// different connection; it does mean this connection's own mailbox can't be otherwise
    /// accessed until the call returns (the `Handle` this wraps is documented to disallow it).
    fn idle_wait(
        &self,
        folder: &str,
        timeout: Duration,
    ) -> impl Future<Output = Result<IdleOutcome>> + Send;

    /// Lists every selectable mailbox on the server (RFC 3501 `LIST "" "*"`), so the TUI can offer
    /// folders beyond the two this client otherwise ever names (`imap_folder`, `archive_folder`) —
    /// e.g. Sent, Drafts, Junk, or any custom folder. A name flagged `\Noselect` (a hierarchy node
    /// with no mailbox of its own, e.g. a `%`-style parent) is dropped, since it can never be
    /// `SELECT`ed to sync. Sorted and deduplicated.
    fn list_folders(&self) -> impl Future<Output = Result<Vec<String>>> + Send;
}

#[derive(Debug)]
pub struct ImapMailSource {
    config: EmailConfig,
}

impl ImapMailSource {
    #[must_use]
    pub const fn new(config: EmailConfig) -> Self {
        Self { config }
    }
}

impl MailSource for ImapMailSource {
    async fn fetch_new(
        &self,
        folder: &str,
        since: Option<SyncCursor>,
    ) -> Result<(Option<u32>, Vec<RawMessage>)> {
        match tokio::time::timeout(FETCH_TIMEOUT, self.fetch_new_inner(folder, since)).await {
            Ok(result) => result,
            Err(_) => anyhow::bail!(
                "IMAP sync for '{}' timed out after {}s",
                self.config.account,
                FETCH_TIMEOUT.as_secs()
            ),
        }
    }

    async fn delete(&self, folder: &str, uid: u32) -> Result<()> {
        match tokio::time::timeout(DELETE_TIMEOUT, self.delete_inner(folder, uid)).await {
            Ok(result) => result,
            Err(_) => anyhow::bail!(
                "IMAP delete for '{}' timed out after {}s",
                self.config.account,
                DELETE_TIMEOUT.as_secs()
            ),
        }
    }

    async fn archive(&self, folder: &str, uid: u32) -> Result<()> {
        match tokio::time::timeout(ARCHIVE_TIMEOUT, self.archive_inner(folder, uid)).await {
            Ok(result) => result,
            Err(_) => anyhow::bail!(
                "IMAP archive for '{}' timed out after {}s",
                self.config.account,
                ARCHIVE_TIMEOUT.as_secs()
            ),
        }
    }

    async fn fetch_attachments(
        &self,
        folder: &str,
        uid: u32,
    ) -> Result<Vec<(Option<String>, Vec<u8>)>> {
        match tokio::time::timeout(
            FETCH_ATTACHMENT_TIMEOUT,
            self.fetch_attachments_inner(folder, uid),
        )
        .await
        {
            Ok(result) => result,
            Err(_) => anyhow::bail!(
                "IMAP attachment fetch for '{}' timed out after {}s",
                self.config.account,
                FETCH_ATTACHMENT_TIMEOUT.as_secs()
            ),
        }
    }

    async fn idle_wait(&self, folder: &str, timeout: Duration) -> Result<IdleOutcome> {
        let outer_timeout = timeout + IDLE_CONNECT_TIMEOUT;
        match tokio::time::timeout(outer_timeout, self.idle_wait_inner(folder, timeout)).await {
            Ok(result) => result,
            Err(_) => anyhow::bail!(
                "IMAP IDLE for '{}' timed out after {}s (connect/login/select never finished)",
                self.config.account,
                outer_timeout.as_secs()
            ),
        }
    }

    async fn list_folders(&self) -> Result<Vec<String>> {
        match tokio::time::timeout(LIST_TIMEOUT, self.list_folders_inner()).await {
            Ok(result) => result,
            Err(_) => anyhow::bail!(
                "IMAP LIST for '{}' timed out after {}s",
                self.config.account,
                LIST_TIMEOUT.as_secs()
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
        folder: &str,
        since: Option<SyncCursor>,
    ) -> Result<(Option<u32>, Vec<RawMessage>)> {
        ensure_crypto_provider();
        let account = &self.config.account;

        tracing::debug!(
            "[Mail:{account}] connecting to {}:{}",
            self.config.imap_server,
            self.config.imap_port
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

        tracing::debug!("[Mail:{account}] selecting folder {folder}");
        let mailbox = session
            .select(folder)
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

        let search_query =
            since_uid.map_or_else(|| "ALL".to_string(), |uid| format!("UID {}:*", uid + 1));

        tracing::debug!("[Mail:{account}] UID search: {search_query}");
        let uids = session
            .uid_search(&search_query)
            .await
            .context("IMAP UID search failed")?;

        let mut result = Vec::new();

        let mut sorted: Vec<u32> = uids.into_iter().collect();
        sorted.sort_unstable();
        // RFC 3501: `UID n:*` always includes the highest UID, even when that is below `n`, so a
        // mailbox with nothing new still answers with its last message. Drop what we already have.
        if let Some(seen) = since_uid {
            sorted.retain(|&uid| uid > seen);
        }

        if !sorted.is_empty() {
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
            tracing::debug!(
                "[Mail:{account}] checking size of {} message(s)",
                sorted.len()
            );
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
                if sizes
                    .get(uid)
                    .is_some_and(|&size| size > LARGE_MESSAGE_BYTES)
                {
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

    async fn delete_inner(&self, folder: &str, uid: u32) -> Result<()> {
        ensure_crypto_provider();
        let account = &self.config.account;

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

        session.select(folder).await.context("failed to select IMAP folder")?;

        tracing::debug!("[Mail:{account}] deleting UID {uid}");
        let mut store_stream = session
            .uid_store(uid.to_string(), "+FLAGS.SILENT (\\Deleted)")
            .await
            .context("IMAP UID STORE failed")?;
        while store_stream
            .try_next()
            .await
            .context("error reading IMAP STORE response")?
            .is_some()
        {}
        drop(store_stream);

        // Must be drained, not just dropped: an unpolled stream leaves its untagged/tagged
        // responses unread on the wire, which would desync the next command (`logout`, below)
        // into parsing leftover EXPUNGE bytes as its own reply.
        let expunge_stream = session.expunge().await.context("IMAP EXPUNGE failed")?;
        let _: Vec<u32> = expunge_stream
            .try_collect()
            .await
            .context("error reading IMAP EXPUNGE response")?;

        let _ = session.logout().await;
        Ok(())
    }

    async fn archive_inner(&self, folder: &str, uid: u32) -> Result<()> {
        ensure_crypto_provider();
        let account = &self.config.account;
        let dest = &self.config.archive_folder;

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

        session.select(folder).await.context("failed to select IMAP folder")?;

        tracing::debug!("[Mail:{account}] archiving UID {uid} to '{dest}'");
        // RFC 6851 MOVE in one round trip when the server supports it; a server that doesn't
        // (`BAD`/`NO` — no `MOVE` capability) falls back to the classical
        // COPY + STORE \Deleted + EXPUNGE sequence MOVE is defined to be equivalent to.
        match session.uid_mv(uid.to_string(), dest.as_str()).await {
            Ok(()) => {}
            Err(async_imap::error::Error::Bad(_) | async_imap::error::Error::No(_)) => {
                session
                    .uid_copy(uid.to_string(), dest.as_str())
                    .await
                    .context("IMAP UID COPY (archive fallback) failed")?;

                let mut store_stream = session
                    .uid_store(uid.to_string(), "+FLAGS.SILENT (\\Deleted)")
                    .await
                    .context("IMAP UID STORE (archive fallback) failed")?;
                while store_stream
                    .try_next()
                    .await
                    .context("error reading IMAP STORE response")?
                    .is_some()
                {}
                drop(store_stream);

                // Must be drained, not just dropped — see the identical note in `delete_inner`.
                let expunge_stream = session
                    .expunge()
                    .await
                    .context("IMAP EXPUNGE (archive fallback) failed")?;
                let _: Vec<u32> = expunge_stream
                    .try_collect()
                    .await
                    .context("error reading IMAP EXPUNGE response")?;
            }
            Err(err) => return Err(err).context("IMAP UID MOVE failed"),
        }

        let _ = session.logout().await;
        Ok(())
    }

    async fn fetch_attachments_inner(
        &self,
        folder: &str,
        uid: u32,
    ) -> Result<Vec<(Option<String>, Vec<u8>)>> {
        ensure_crypto_provider();
        let account = &self.config.account;

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

        session.select(folder).await.context("failed to select IMAP folder")?;

        tracing::debug!("[Mail:{account}] fetching attachments for UID {uid}");
        let mut stream = session
            .uid_fetch(uid.to_string(), "RFC822")
            .await
            .context("IMAP UID FETCH failed")?;

        let mut raw = None;
        while let Some(fetch) = stream
            .try_next()
            .await
            .context("error reading IMAP fetch response")?
        {
            if let Some(body) = fetch.body() {
                raw = Some(body.to_vec());
            }
        }
        drop(stream);
        let _ = session.logout().await;

        let raw = raw.context("message not found on server")?;
        let message = mail_parser::MessageParser::default()
            .parse(&raw)
            .context("failed to parse RFC822 message")?;

        Ok(message
            .attachments()
            .map(|part| (part.attachment_name().map(str::to_string), part.contents().to_vec()))
            .collect())
    }

    async fn idle_wait_inner(&self, folder: &str, timeout: Duration) -> Result<IdleOutcome> {
        ensure_crypto_provider();
        let account = &self.config.account;

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

        session.select(folder).await.context("failed to select IMAP folder")?;

        tracing::debug!("[Mail:{account}] entering IDLE on {folder}");
        let mut handle = session.idle();
        handle.init().await.context("IMAP IDLE init failed")?;
        let (idle_wait, _stop) = handle.wait_with_timeout(timeout);
        let outcome = match idle_wait.await {
            Ok(async_imap::extensions::idle::IdleResponse::NewData(data)) => {
                tracing::debug!("[Mail:{account}] IDLE got new data: {:?}", data.parsed());
                IdleOutcome::NewData
            }
            Ok(
                async_imap::extensions::idle::IdleResponse::Timeout
                | async_imap::extensions::idle::IdleResponse::ManualInterrupt,
            ) => IdleOutcome::Timeout,
            Err(e) => {
                tracing::warn!("[Mail:{account}] IDLE wait failed, treating as a timeout: {e}");
                IdleOutcome::Timeout
            }
        };

        // DONE must be sent to leave IDLE cleanly before the connection can do anything else
        // (including logout) — `Handle::done` sends it and hands back the underlying `Session`.
        let mut session = handle.done().await.context("IMAP IDLE DONE failed")?;
        let _ = session.logout().await;
        Ok(outcome)
    }

    async fn list_folders_inner(&self) -> Result<Vec<String>> {
        ensure_crypto_provider();
        let account = &self.config.account;

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

        tracing::debug!("[Mail:{account}] listing folders");
        let names_stream = session.list(None, Some("*")).await.context("IMAP LIST failed")?;
        // Must be drained, not just dropped — see the identical note on `expunge()` in
        // `delete_inner`; `Name` carries the same not-`Unpin` shape, so `try_collect` (takes
        // `self`) rather than `try_next` (needs `&mut self: Unpin`).
        let names: Vec<async_imap::types::Name> =
            names_stream.try_collect().await.context("error reading IMAP LIST response")?;

        let mut folders: Vec<String> = names
            .iter()
            .filter(|n| !n.attributes().contains(&async_imap::types::NameAttribute::NoSelect))
            .map(|n| n.name().to_string())
            .collect();
        folders.sort_unstable();
        folders.dedup();

        let _ = session.logout().await;
        Ok(folders)
    }
}
