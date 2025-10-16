use anyhow::{Result, anyhow};
use async_imap::{Client, Session};
use futures::StreamExt;
use mailparse::{MailHeaderMap, parse_mail};
use sqlx::SqlitePool;
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tokio::time::{Duration, sleep, timeout};
use tokio_native_tls::{TlsConnector, TlsStream, native_tls};

use super::config::ImapConfig;

type ImapSession = Session<TlsStream<TcpStream>>;

const IDLE_REFRESH_TIMEOUT: Duration = Duration::from_secs(24 * 60);
const MAX_RECONNECT_ATTEMPTS: u32 = 10;

pub async fn email_sync_worker(
    db: SqlitePool,
    mut shutdown_rx: broadcast::Receiver<()>,
    config: ImapConfig,
) -> Result<()> {
    eprintln!("[Email] Starting IMAP sync worker for {}", config.server);

    let mut reconnect_delay = Duration::from_secs(1);
    let mut reconnect_attempts = 0;

    loop {
        match connect_and_authenticate(&config).await {
            Ok(session) => {
                eprintln!("[Email] Connected to IMAP server");
                reconnect_delay = Duration::from_secs(1);
                reconnect_attempts = 0;

                match run_idle_loop(session, &db, &config, &mut shutdown_rx).await {
                    Ok(_) => {
                        eprintln!("[Email] IDLE loop terminated gracefully");
                        break;
                    }
                    Err(e) => {
                        eprintln!("[Email] IDLE loop error: {}", e);
                    }
                }
            }
            Err(e) => {
                eprintln!("[Email] Connection failed: {}", e);

                if e.to_string().to_lowercase().contains("authentication")
                    || e.to_string().to_lowercase().contains("login")
                {
                    eprintln!("[Email] Authentication failed - disabling email sync");
                    return Err(e);
                }

                reconnect_attempts += 1;
                if reconnect_attempts >= MAX_RECONNECT_ATTEMPTS {
                    return Err(anyhow!(
                        "Failed to connect after {} attempts",
                        MAX_RECONNECT_ATTEMPTS
                    ));
                }
            }
        }

        tokio::select! {
            _ = shutdown_rx.recv() => {
                eprintln!("[Email] Shutdown signal received during backoff");
                break;
            }
            _ = sleep(reconnect_delay) => {
                reconnect_delay = (reconnect_delay * 2).min(Duration::from_secs(30));
            }
        }
    }

    Ok(())
}

async fn connect_and_authenticate(config: &ImapConfig) -> Result<ImapSession> {
    let tcp_stream = TcpStream::connect((config.server.as_str(), config.port)).await?;

    let connector = TlsConnector::from(native_tls::TlsConnector::new()?);
    let tls_stream = connector
        .connect(&config.server, tcp_stream)
        .await
        .map_err(|e| anyhow!("TLS connection failed: {}", e))?;

    let mut client = Client::new(tls_stream);
    let _greeting = client
        .read_response()
        .await?
        .ok_or_else(|| anyhow!("No greeting from server"))?;

    let session = client
        .login(&config.username, &config.password)
        .await
        .map_err(|(e, _)| anyhow!("Login failed: {}", e))?;

    Ok(session)
}

async fn run_idle_loop(
    mut session: ImapSession,
    db: &SqlitePool,
    config: &ImapConfig,
    shutdown_rx: &mut broadcast::Receiver<()>,
) -> Result<()> {
    session.select(&config.folder).await?;
    if let Ok(count) = fetch_new_messages(&mut session, db).await {
        eprintln!("[Email] Initial sync: {} unread messages", count);
    }

    loop {
        let mut idle = session.idle(); // session is moved into idle here
        idle.init().await?;

        let (idle_wait_future, _stop_source) = idle.wait();
        let idle_with_timeout = timeout(IDLE_REFRESH_TIMEOUT, idle_wait_future);

        tokio::select! {
            result = idle_with_timeout => {
                // Recover ownership of Session before using it again
                session = idle.done().await?;  // <-- critical re-assignment

                match result {
                    Ok(Ok(_)) => {
                        if let Ok(count) = fetch_new_messages(&mut session, db).await
                            && count > 0 {
                                eprintln!("[Email] Fetched {} new messages", count);
                            }
                    }
                    Ok(Err(e)) => {
                        eprintln!("[Email] IDLE error: {}", e);
                        return Err(e.into());
                    }
                    Err(_) => {
                        eprintln!("[Email] IDLE refresh timeout - restarting IDLE");
                    }
                }
            }
            _ = shutdown_rx.recv() => {
                eprintln!("[Email] Shutdown signal received");
                // Stop IDLE and recover Session before logging out
                session = idle.done().await?;  // <-- critical re-assignment
                session.logout().await?;
                return Ok(());
            }
        }
    }
}

async fn fetch_new_messages(session: &mut ImapSession, db: &SqlitePool) -> Result<usize> {
    let uid_strings = session.uid_search("UNSEEN").await?;

    if uid_strings.is_empty() {
        return Ok(0);
    }

    eprintln!("[Email] Found {} unseen message(s)", uid_strings.len());
    let mut fetched_count = 0;

    for uid in uid_strings.iter() {
        let uid_str = uid.to_string();

        let exists =
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM emails WHERE message_id = ?")
                .bind(&uid_str)
                .fetch_one(db)
                .await?
                > 0;

        if exists {
            continue;
        }

        let messages = session.uid_fetch(&uid_str, "RFC822").await?;
        let messages: Vec<_> = messages.collect().await;

        for fetch_result in messages {
            match fetch_result {
                Ok(fetch) => {
                    if let Some(body) = fetch.body() {
                        match parse_and_store_email(&uid_str, body, db).await {
                            Ok(_) => fetched_count += 1,
                            Err(e) => {
                                eprintln!("[Email] Failed to parse message {}: {}", uid_str, e)
                            }
                        }
                    }
                }
                Err(e) => eprintln!("[Email] Fetch error for UID {}: {}", uid_str, e),
            }
        }
    }

    Ok(fetched_count)
}

async fn parse_and_store_email(message_id: &str, raw_email: &[u8], db: &SqlitePool) -> Result<()> {
    let parsed = parse_mail(raw_email)?;

    let subject = parsed
        .headers
        .get_first_value("Subject")
        .unwrap_or_else(|| "(No Subject)".to_string());
    let from = parsed
        .headers
        .get_first_value("From")
        .unwrap_or_else(|| "Unknown".to_string());
    let to = parsed
        .headers
        .get_first_value("To")
        .unwrap_or_else(|| String::new());
    let date_str = parsed
        .headers
        .get_first_value("Date")
        .unwrap_or_else(|| chrono::Utc::now().to_rfc2822());

    let received_at = mailparse::dateparse(&date_str)
        .ok()
        .and_then(|timestamp| chrono::DateTime::from_timestamp(timestamp, 0))
        .unwrap_or_else(chrono::Utc::now);

    let body_text = extract_text_body(&parsed);
    let body_html = extract_html_body(&parsed);

    sqlx::query(
        "INSERT INTO emails (message_id, subject, sender, recipients, body_text, body_html, received_at, folder_name, is_read) 
         VALUES (?, ?, ?, ?, ?, ?, ?, 'INBOX', 0)"
    )
    .bind(message_id)
    .bind(subject)
    .bind(from)
    .bind(to)
    .bind(body_text)
    .bind(body_html)
    .bind(received_at.to_rfc3339())
    .execute(db)
    .await?;

    Ok(())
}

fn extract_text_body(mail: &mailparse::ParsedMail) -> Option<String> {
    if mail.ctype.mimetype == "text/plain" {
        return mail.get_body().ok();
    }
    for subpart in &mail.subparts {
        if let Some(body) = extract_text_body(subpart) {
            return Some(body);
        }
    }
    None
}

fn extract_html_body(mail: &mailparse::ParsedMail) -> Option<String> {
    if mail.ctype.mimetype == "text/html" {
        return mail.get_body().ok();
    }
    for subpart in &mail.subparts {
        if let Some(body) = extract_html_body(subpart) {
            return Some(body);
        }
    }
    None
}
