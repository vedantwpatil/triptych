use crate::app::{NewTask, insert_task_row};
use crate::nlp::{NLPParser, types::ParseResult};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal;

// Socket path (will be in /tmp on Unix systems). Honors `TRIPTYCH_SOCKET_PATH` so tests/tooling
// can run an isolated daemon without colliding with a real one on the shared default path.
fn socket_path() -> PathBuf {
    std::env::var("TRIPTYCH_SOCKET_PATH").map_or_else(
        |_| std::env::temp_dir().join("triptych.sock"),
        PathBuf::from,
    )
}

/// Longest request or response accepted; anything bigger fails to parse rather than fill memory.
const MAX_MESSAGE: u64 = 1 << 20;
/// How long the daemon waits for a client to finish sending its request (clients send at once).
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// How long a client waits for the daemon. Covers a parse that waits out a cold model load (up to
/// 90s, see `OLLAMA_LOAD_TIMEOUT_MS`) and still frees the CLI if the daemon is stuck.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

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

    // A live daemon answers the health check; anything else at this path is a stale socket.
    if is_daemon_running().await {
        anyhow::bail!("Daemon already running at {}", socket.display());
    }
    let _ = std::fs::remove_file(&socket);

    let listener = UnixListener::bind(&socket)
        .context(format!("Failed to bind to socket: {}", socket.display()))?;

    eprintln!("[Daemon] Started at {}", socket.display());
    eprintln!("[Daemon] Pre-warming Ollama and loading cache...");

    // Load the model in the background so the daemon answers at once; a parse that arrives
    // mid-load waits for it (see `OllamaClient::parse`).
    let warm_nlp = nlp.clone();
    tokio::spawn(async move {
        let warmup_start = std::time::Instant::now();
        warm_nlp.prewarm().await;
        eprintln!(
            "[Daemon] Pre-warmed in {:.2}s",
            warmup_start.elapsed().as_secs_f64()
        );
    });

    // Preload the cache from the database in the background too: each parse can wait on Ollama,
    // and until the accept loop runs, clients (and `is_daemon_running`) would hang on the socket.
    let (cache_db, cache_nlp) = (db.clone(), nlp.clone());
    tokio::spawn(async move {
        let cache_start = std::time::Instant::now();
        let rows: Vec<(String, i64)> = match sqlx::query_as(
            r"
            SELECT natural_language_input, COUNT(*) as count
            FROM tasks
            WHERE natural_language_input IS NOT NULL
            GROUP BY natural_language_input
            ORDER BY count DESC
            LIMIT 100
            ",
        )
        .fetch_all(&cache_db)
        .await
        {
            Ok(rows) => rows,
            Err(e) => {
                eprintln!("[Daemon] Cache preload failed: {e}");
                return;
            }
        };

        let mut loaded = 0;
        for (input, _) in rows {
            let _ = cache_nlp.parse(&input).await;
            loaded += 1;
        }

        eprintln!(
            "[Daemon] Loaded {} cache entries in {:.2}s",
            loaded,
            cache_start.elapsed().as_secs_f64()
        );
    });
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
                                        eprintln!("[Daemon] Client error: {e}");
                                    }
                                }
                                _ = shutdown_rx.recv() => {
                                    eprintln!("[Daemon] Client connection closed due to shutdown");
                                }
                            }
                        });
                    }
                    Err(e) => {
                        eprintln!("[Daemon] Accept error: {e}");
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
    // Read request: the client half-closes after writing, so EOF marks its end. A client that
    // connects and stays silent is dropped instead of holding this task forever.
    let mut buffer = Vec::new();
    tokio::time::timeout(
        READ_TIMEOUT,
        (&mut stream).take(MAX_MESSAGE).read_to_end(&mut buffer),
    )
    .await
    .context("Client sent no complete request in time")?
    .context("Failed to read from socket")?;

    if buffer.is_empty() {
        return Ok(());
    }

    let request: DaemonRequest =
        serde_json::from_slice(&buffer).context("Failed to parse request")?;

    // Process request
    let response = match request {
        DaemonRequest::Parse { input } => match nlp.parse(&input).await {
            Ok(result) => DaemonResponse::ParseResult(result),
            Err(e) => DaemonResponse::Error(format!("Parse error: {e}")),
        },

        DaemonRequest::AddTask { description } => {
            match add_task_to_db(&db, &nlp, &description).await {
                Ok(id) => DaemonResponse::TaskAdded { id },
                Err(e) => DaemonResponse::Error(format!("Database error: {e}")),
            }
        }

        DaemonRequest::Shutdown => {
            // Send OK then exit. This bypasses the accept loop's own shutdown path, so
            // clean up the socket here too - otherwise it's left stale on disk (harmless
            // for a future `daemon` start, which force-removes it anyway, but confusing
            // for anything checking `socket.exists()` as a running/not-running signal).
            let response_bytes = serde_json::to_vec(&DaemonResponse::Ok)?;
            stream.write_all(&response_bytes).await?;
            let _ = std::fs::remove_file(socket_path());
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

/// Add a task to the database (daemon version). Persists deadline/duration like
/// `App::add_task`, but doesn't reallocate (no App/pool of blocks here) - run
/// `triptych schedule reallocate` (or reopen the TUI) to pick up new deadlines.
async fn add_task_to_db(db: &SqlitePool, nlp: &Arc<NLPParser>, description: &str) -> Result<i64> {
    anyhow::ensure!(!description.trim().is_empty(), "task description is empty");
    let parse_result = nlp.parse(description).await?;

    let (title, scheduled_at, priority, tags, deadline, duration_minutes) =
        crate::app::extract_task_fields(parse_result.item);

    Ok(insert_task_row(
        db,
        &NewTask {
            title: &title,
            input: description,
            item_order: None,
            priority,
            tags: &tags,
            scheduled_at,
            deadline,
            duration_minutes,
            recurrence: parse_result.recurrence,
        },
    )
    .await?)
}

/// Send a request to the daemon
pub async fn send_to_daemon(request: DaemonRequest) -> Result<DaemonResponse> {
    tokio::time::timeout(REQUEST_TIMEOUT, exchange(request))
        .await
        .context("Daemon did not answer in time")?
}

async fn exchange(request: DaemonRequest) -> Result<DaemonResponse> {
    let socket = socket_path();

    if !socket.exists() {
        anyhow::bail!("Daemon not running (socket not found)");
    }

    let mut stream = UnixStream::connect(&socket)
        .await
        .context("Failed to connect to daemon")?;

    // Send request; half-closing tells the daemon it is complete.
    let request_bytes = serde_json::to_vec(&request)?;
    stream.write_all(&request_bytes).await?;
    stream.shutdown().await?;

    // Read response until the daemon closes the connection.
    let mut buffer = Vec::new();
    stream.take(MAX_MESSAGE).read_to_end(&mut buffer).await?;

    Ok(serde_json::from_slice(&buffer)?)
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
    if !is_daemon_running().await {
        anyhow::bail!("Daemon not running");
    }
    send_to_daemon(DaemonRequest::Shutdown)
        .await
        .context("Failed to stop daemon")?;
    eprintln!("✓ Daemon stopped");
    Ok(())
}
