use crate::nlp::{NLPParser, types::ParseResult};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal;

// Socket path (will be in /tmp on Unix systems)
fn socket_path() -> PathBuf {
    std::env::temp_dir().join("triptych.sock")
}

// Messages sent between CLI and daemon
#[derive(Serialize, Deserialize, Debug)]
pub enum DaemonRequest {
    Parse { input: String },
    AddTask { description: String },
    Shutdown,
    Health,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum DaemonResponse {
    ParseResult(ParseResult),
    TaskAdded { id: i64 },
    Ok,
    Error(String),
}

/// Start the persistent background daemon
pub async fn start_daemon(db: SqlitePool, nlp: Arc<NLPParser>) -> Result<()> {
    let socket = socket_path();

    // Remove old socket if exists
    let _ = std::fs::remove_file(&socket);

    let listener =
        UnixListener::bind(&socket).context(format!("Failed to bind to socket: {:?}", socket))?;

    eprintln!("[Daemon] Started at {:?}", socket);
    eprintln!("[Daemon] Pre-warming Ollama and loading cache...");

    // Pre-warm Ollama
    let warmup_start = std::time::Instant::now();
    let _ = nlp.parse("warmup query").await;
    eprintln!(
        "[Daemon] Pre-warmed in {:.2}s",
        warmup_start.elapsed().as_secs_f64()
    );

    // Preload cache from database
    let cache_start = std::time::Instant::now();
    let rows: Vec<(String, i64)> = sqlx::query_as(
        r#"
        SELECT natural_language_input, COUNT(*) as count
        FROM tasks
        WHERE natural_language_input IS NOT NULL
        GROUP BY natural_language_input
        ORDER BY count DESC
        LIMIT 100
        "#,
    )
    .fetch_all(&db)
    .await?;

    let mut loaded = 0;
    for (input, _) in rows {
        let _ = nlp.parse(&input).await;
        loaded += 1;
    }

    eprintln!(
        "[Daemon] Loaded {} cache entries in {:.2}s",
        loaded,
        cache_start.elapsed().as_secs_f64()
    );
    eprintln!("[Daemon] Ready! Listening for commands...");

    // Setup graceful shutdown
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);

    // Spawn shutdown handler
    let shutdown_tx_clone = shutdown_tx.clone();
    tokio::spawn(async move {
        signal::ctrl_c().await.ok();
        eprintln!("\n[Daemon] Shutting down...");
        let _ = shutdown_tx_clone.send(());
    });

    // Accept connections until shutdown
    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, _)) => {
                        let db = db.clone();
                        let nlp = nlp.clone();
                        let mut shutdown_rx = shutdown_tx.subscribe();

                        tokio::spawn(async move {
                            tokio::select! {
                                result = handle_client(stream, db, nlp) => {
                                    if let Err(e) = result {
                                        eprintln!("[Daemon] Client error: {}", e);
                                    }
                                }
                                _ = shutdown_rx.recv() => {
                                    eprintln!("[Daemon] Client connection closed due to shutdown");
                                }
                            }
                        });
                    }
                    Err(e) => {
                        eprintln!("[Daemon] Accept error: {}", e);
                    }
                }
            }

            _ = shutdown_rx.recv() => {
                eprintln!("[Daemon] Shutdown signal received, exiting");
                break;
            }
        }
    }

    // Cleanup socket
    let _ = std::fs::remove_file(&socket);
    Ok(())
}

/// Handle a single client connection
async fn handle_client(mut stream: UnixStream, db: SqlitePool, nlp: Arc<NLPParser>) -> Result<()> {
    // Read request
    let mut buffer = vec![0u8; 8192];
    let n = stream
        .read(&mut buffer)
        .await
        .context("Failed to read from socket")?;

    if n == 0 {
        return Ok(());
    }

    let request: DaemonRequest =
        serde_json::from_slice(&buffer[..n]).context("Failed to parse request")?;

    // Process request
    let response = match request {
        DaemonRequest::Parse { input } => match nlp.parse(&input).await {
            Ok(result) => DaemonResponse::ParseResult(result),
            Err(e) => DaemonResponse::Error(format!("Parse error: {}", e)),
        },

        DaemonRequest::AddTask { description } => {
            match add_task_to_db(&db, &nlp, &description).await {
                Ok(id) => DaemonResponse::TaskAdded { id },
                Err(e) => DaemonResponse::Error(format!("Database error: {}", e)),
            }
        }

        DaemonRequest::Shutdown => {
            // Send OK then exit
            let response_bytes = serde_json::to_vec(&DaemonResponse::Ok)?;
            stream.write_all(&response_bytes).await?;
            std::process::exit(0);
        }

        DaemonRequest::Health => DaemonResponse::Ok,
    };

    // Send response
    let response_bytes = serde_json::to_vec(&response)?;
    stream
        .write_all(&response_bytes)
        .await
        .context("Failed to write response")?;

    Ok(())
}

/// Add a task to the database (daemon version)
async fn add_task_to_db(db: &SqlitePool, nlp: &Arc<NLPParser>, description: &str) -> Result<i64> {
    use crate::nlp::types::{ParsedItem, Priority};

    let parse_result = nlp.parse(description).await?;

    let (task_title, scheduled_at, priority_value, tags_list) = match parse_result.item {
        ParsedItem::Task(nlp_task) => {
            let priority = match nlp_task.priority {
                Priority::Urgent => 3,
                Priority::High => 2,
                Priority::Medium => 1,
                Priority::Low => 0,
            };
            (nlp_task.title, nlp_task.due_date, priority, nlp_task.tags)
        }
        ParsedItem::Event(event) => (event.title, Some(event.start_time), 1, event.tags),
        ParsedItem::Email(_) => (description.to_string(), None, 1, Vec::new()),
    };

    let tags_json = if tags_list.is_empty() {
        None
    } else {
        Some(serde_json::to_string(&tags_list).unwrap_or_default())
    };

    // Use runtime query instead of query! macro
    let result = sqlx::query(
        r#"
        INSERT INTO tasks (description, completed, item_order, priority, natural_language_input, tags, scheduled_at)
        VALUES (?, ?, (SELECT COALESCE(MAX(item_order), -1) + 1 FROM tasks), ?, ?, ?, ?)
        "#
    )
    .bind(&task_title)
    .bind(false)
    .bind(priority_value)
    .bind(description)
    .bind(tags_json)
    .bind(scheduled_at)
    .execute(db)
    .await?;

    Ok(result.last_insert_rowid())
}

/// Send a request to the daemon
pub async fn send_to_daemon(request: DaemonRequest) -> Result<DaemonResponse> {
    let socket = socket_path();

    if !socket.exists() {
        anyhow::bail!("Daemon not running (socket not found)");
    }

    let mut stream = UnixStream::connect(&socket)
        .await
        .context("Failed to connect to daemon")?;

    // Send request
    let request_bytes = serde_json::to_vec(&request)?;
    stream.write_all(&request_bytes).await?;
    stream.shutdown().await?;

    // Read response
    let mut buffer = vec![0u8; 8192];
    let n = stream.read(&mut buffer).await?;

    let response: DaemonResponse = serde_json::from_slice(&buffer[..n])?;
    Ok(response)
}

/// Check if daemon is running
pub async fn is_daemon_running() -> bool {
    let socket = socket_path();
    if !socket.exists() {
        return false;
    }

    // Try to connect and send health check
    matches!(
        send_to_daemon(DaemonRequest::Health).await,
        Ok(DaemonResponse::Ok)
    )
}

/// Stop the running daemon
pub async fn stop_daemon() -> Result<()> {
    match send_to_daemon(DaemonRequest::Shutdown).await {
        Ok(_) => {
            eprintln!("✓ Daemon stopped");
            Ok(())
        }
        Err(e) => {
            eprintln!("✗ Daemon not running or failed to stop: {}", e);
            Err(e)
        }
    }
}
