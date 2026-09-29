//! Hand-rolled RFC 5321 SMTP client for sending mail.
//!
//! Mirrors `client.rs`'s hand-rolled IMAP approach (no `lettre`/mail-send dependency) and shares
//! its TLS setup (`tls.rs`).

use anyhow::{Context, Result};
use chrono::Utc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::Duration;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::ClientConfig;
use tokio_rustls::rustls::pki_types::ServerName;

use super::config::SmtpConfig;
use super::tls::{build_root_store, ensure_crypto_provider};

/// Upper bound on one `send` call (connect through QUIT). Mirrors `client.rs`'s `FETCH_TIMEOUT`:
/// no read timeout of its own, so a stalled server would otherwise hang the caller forever.
const SEND_TIMEOUT: Duration = Duration::from_secs(60);

/// A message ready to send, plus the headers a reply/forward needs for correct threading.
///
/// `to`/`cc` are comma-or-semicolon-separated address lists as the user typed them (either bare
/// `a@b.com` or `Name <a@b.com>`); `body` is plain text with `\n` line endings.
#[derive(Debug, Clone, Default)]
pub struct OutgoingMessage {
    pub to: String,
    pub cc: String,
    pub subject: String,
    pub body: String,
    /// Set when replying: the original message's `Message-ID`, sent as `In-Reply-To`.
    pub in_reply_to: Option<String>,
    /// Set when replying: `in_reply_to` chained onto the original's own `References` (RFC 5322
    /// §3.6.4), sent as-is as this message's `References`.
    pub references: Option<String>,
}

/// Sends `message` via `config`, timeout-bounded like `client.rs::fetch_new`.
///
/// # Errors
///
/// Returns an error if connecting, authenticating, or any SMTP command fails, if there are no
/// recipients, or if sending doesn't complete within the timeout.
pub async fn send(config: &SmtpConfig, message: &OutgoingMessage) -> Result<()> {
    match tokio::time::timeout(SEND_TIMEOUT, send_inner(config, message)).await {
        Ok(result) => result,
        Err(_) => anyhow::bail!(
            "SMTP send for '{}' timed out after {}s",
            config.account,
            SEND_TIMEOUT.as_secs()
        ),
    }
}

// One linear SMTP protocol sequence (connect, [STARTTLS], EHLO, AUTH, MAIL/RCPT/DATA, QUIT) -
// splitting it into helpers would scatter that sequence without reducing its actual complexity.
#[allow(clippy::too_many_lines)]
async fn send_inner(config: &SmtpConfig, message: &OutgoingMessage) -> Result<()> {
    ensure_crypto_provider();
    let account = &config.account;
    let recipients = parse_recipients(&message.to, &message.cc);
    if recipients.is_empty() {
        anyhow::bail!("no recipients to send to");
    }

    tracing::debug!(
        "[SMTP:{account}] connecting to {}:{}",
        config.smtp_server,
        config.smtp_port
    );
    let tcp = TcpStream::connect((config.smtp_server.as_str(), config.smtp_port))
        .await
        .context("failed to connect to SMTP server")?;

    let root_store = build_root_store()?;
    let tls_config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();
    let connector = TlsConnector::from(Arc::new(tls_config));
    let domain =
        ServerName::try_from(config.smtp_server.clone()).context("invalid SMTP server hostname")?;

    // Implicit TLS (port 465, RFC 8314) wraps the connection immediately, greeting included.
    // Everything else assumes STARTTLS (RFC 3207) over a plaintext connection - the greeting and
    // a first EHLO happen in the clear, then STARTTLS upgrades and RFC 3207 requires a second
    // EHLO over the now-encrypted channel (capabilities may differ, and the first exchange was
    // unauthenticated).
    let mut stream: TlsStream<TcpStream> = if config.smtp_port == 465 {
        tracing::debug!("[SMTP:{account}] TLS handshake (implicit)");
        connector
            .connect(domain.clone(), tcp)
            .await
            .context("TLS handshake with SMTP server failed")?
    } else {
        let mut plain = tcp;
        expect(&mut plain, "SMTP greeting").await?;

        send_line(&mut plain, "EHLO triptych").await?;
        expect(&mut plain, "EHLO").await?;

        send_line(&mut plain, "STARTTLS").await?;
        expect(&mut plain, "STARTTLS").await?;

        tracing::debug!("[SMTP:{account}] TLS handshake (STARTTLS)");
        connector
            .connect(domain, plain)
            .await
            .context("TLS handshake with SMTP server failed")?
    };

    if config.smtp_port == 465 {
        expect(&mut stream, "SMTP greeting").await?;
    }

    send_line(&mut stream, "EHLO triptych").await?;
    expect(&mut stream, "EHLO").await?;

    tracing::debug!(
        "[SMTP:{account}] authenticating as {}",
        config.smtp_username
    );
    send_line(&mut stream, "AUTH LOGIN").await?;
    expect(&mut stream, "AUTH LOGIN").await?;

    send_line(&mut stream, &base64_encode(config.smtp_username.as_bytes())).await?;
    expect(&mut stream, "SMTP username").await?;

    send_line(&mut stream, &base64_encode(config.smtp_password.as_bytes())).await?;
    expect(&mut stream, "SMTP authentication").await?;

    send_line(&mut stream, &format!("MAIL FROM:<{}>", config.from_addr)).await?;
    expect(&mut stream, "MAIL FROM").await?;

    for addr in &recipients {
        send_line(&mut stream, &format!("RCPT TO:<{addr}>")).await?;
        expect(&mut stream, "RCPT TO").await?;
    }

    send_line(&mut stream, "DATA").await?;
    expect(&mut stream, "DATA").await?;

    let mime = build_mime(config, message);
    stream
        .write_all(dot_stuff(&mime).as_bytes())
        .await
        .context("failed to write message body")?;
    stream
        .write_all(b"\r\n.\r\n")
        .await
        .context("failed to terminate message body")?;
    expect(&mut stream, "message submission").await?;

    send_line(&mut stream, "QUIT").await?;
    let _ = read_reply(&mut stream).await;

    tracing::info!("[SMTP:{account}] sent message to {}", recipients.join(", "));
    Ok(())
}

async fn send_line<S: AsyncWrite + Unpin>(stream: &mut S, line: &str) -> Result<()> {
    stream
        .write_all(line.as_bytes())
        .await
        .context("failed to write SMTP command")?;
    stream
        .write_all(b"\r\n")
        .await
        .context("failed to write SMTP command")?;
    Ok(())
}

/// Reads one (possibly multi-line) SMTP reply, returning its 3-digit status code and the last
/// line's text. RFC 5321 §4.2: a continuation line has `-` right after the code (`250-`); the
/// final line has a space (`250 `).
async fn read_reply<S: AsyncRead + Unpin>(stream: &mut S) -> Result<(u16, String)> {
    let mut last_line: String;
    loop {
        let mut raw = Vec::new();
        loop {
            let mut byte = [0u8; 1];
            let n = stream
                .read(&mut byte)
                .await
                .context("connection closed while reading SMTP reply")?;
            if n == 0 {
                anyhow::bail!("SMTP server closed the connection unexpectedly");
            }
            if byte[0] == b'\n' {
                break;
            }
            if byte[0] != b'\r' {
                raw.push(byte[0]);
            }
        }
        last_line = String::from_utf8_lossy(&raw).into_owned();
        if last_line.as_bytes().get(3) == Some(&b'-') {
            continue;
        }
        break;
    }

    let code: u16 = last_line
        .get(..3)
        .and_then(|s| s.parse().ok())
        .context("malformed SMTP reply (no status code)")?;
    let text = last_line.get(4..).unwrap_or("").to_string();

    Ok((code, text))
}

/// Reads a reply and errors (tagged with `what`) unless its code is 2xx or 3xx.
async fn expect<S: AsyncRead + Unpin>(stream: &mut S, what: &str) -> Result<()> {
    let (code, text) = read_reply(stream).await?;
    if !(200..400).contains(&code) {
        anyhow::bail!("{what} rejected: SMTP {code} {text}");
    }
    Ok(())
}

/// Splits `to`/`cc` on commas/semicolons and strips any `Name <...>` wrapper down to the bare
/// address, for use in `MAIL FROM`/`RCPT TO` (which take addresses only, no display names).
#[must_use]
pub fn parse_recipients(to: &str, cc: &str) -> Vec<String> {
    format!("{to},{cc}")
        .split([',', ';'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(extract_address)
        .collect()
}

fn extract_address(raw: &str) -> String {
    raw.rsplit_once('<')
        .and_then(|(_, rest)| rest.strip_suffix('>'))
        .map_or_else(|| raw.to_string(), str::to_string)
}

static MESSAGE_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A locally-unique `Message-ID`: current time plus a process-wide counter, since there's no
/// dependency here for random bytes. Both parts change on every call, so two messages sent in
/// the same nanosecond still get distinct IDs.
fn generate_message_id(domain: &str) -> String {
    let nanos = Utc::now().timestamp_nanos_opt().unwrap_or_default();
    let counter = MESSAGE_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("<{nanos:x}.{counter:x}@{domain}>")
}

/// Builds the RFC 5322 message: headers, blank line, then the plain-text body. Not yet
/// dot-stuffed - that happens once, in [`dot_stuff`], right before the wire write.
fn build_mime(config: &SmtpConfig, message: &OutgoingMessage) -> String {
    let mut headers = vec![
        format!("Date: {}", Utc::now().to_rfc2822()),
        format!("From: {}", config.from_addr),
        format!("To: {}", message.to),
    ];

    if !message.cc.trim().is_empty() {
        headers.push(format!("Cc: {}", message.cc));
    }

    headers.push(format!("Subject: {}", message.subject));
    headers.push(format!(
        "Message-ID: {}",
        generate_message_id(&config.smtp_server)
    ));

    if let Some(in_reply_to) = &message.in_reply_to {
        headers.push(format!("In-Reply-To: {in_reply_to}"));
    }
    if let Some(references) = &message.references {
        headers.push(format!("References: {references}"));
    }

    headers.push("MIME-Version: 1.0".to_string());
    headers.push("Content-Type: text/plain; charset=utf-8".to_string());
    headers.push("Content-Transfer-Encoding: 8bit".to_string());

    let body = message.body.lines().collect::<Vec<_>>().join("\r\n");
    format!("{}\r\n\r\n{body}", headers.join("\r\n"))
}

/// RFC 5321 §4.5.2: any line starting with `.` gets a second `.` prepended, since a lone `.`
/// line ends the `DATA` phase. Operates on `\r\n`-delimited text (what [`build_mime`] produces).
#[must_use]
pub fn dot_stuff(mime: &str) -> String {
    mime.split("\r\n")
        .map(|line| {
            if line.starts_with('.') {
                format!(".{line}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\r\n")
}

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// RFC 4648 base64 encoding, hand-rolled (no `base64` crate) - used for `AUTH LOGIN`'s
/// username/password exchange.
#[must_use]
pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied();
        let b2 = chunk.get(2).copied();

        out.push(BASE64_ALPHABET[(b0 >> 2) as usize] as char);
        out.push(BASE64_ALPHABET[(((b0 & 0x03) << 4) | (b1.unwrap_or(0) >> 4)) as usize] as char);
        out.push(b1.map_or('=', |b1| {
            BASE64_ALPHABET[(((b1 & 0x0F) << 2) | (b2.unwrap_or(0) >> 6)) as usize] as char
        }));
        out.push(b2.map_or('=', |b2| BASE64_ALPHABET[(b2 & 0x3F) as usize] as char));
    }
    out
}
