//! File-based `tracing` setup.

/// Route `tracing::*` calls to a file instead of stderr.
///
/// The callers are the background IMAP sync's connection/fetch chatter (see `src/email/client.rs`,
/// `src/sync/mail.rs`, `App::sync_email_accounts`). Those calls fire from `tokio::spawn`ed tasks that
/// can run at any time while the TUI has raw-mode control of the terminal; writing them straight to
/// stderr punched text into the middle of the user's screen mid-navigation. `email sync`/`email
/// list`'s own summary lines (`println!`/`eprintln!` in `handle_cli_command`) are separate and
/// unaffected - those are a one-shot CLI command's direct, expected output.
///
/// The returned guard must stay alive for the process's lifetime (`tracing-appender`'s non-blocking
/// writer flushes on drop) - bind it in `main` and never drop it explicitly.
pub fn init_tracing() -> tracing_appender::non_blocking::WorkerGuard {
    let log_path = std::env::var("TRIPTYCH_LOG_PATH").map_or_else(
        |_| std::env::temp_dir().join("triptych.log"),
        std::path::PathBuf::from,
    );
    let dir = log_path.parent().filter(|p| !p.as_os_str().is_empty());
    let dir = dir.unwrap_or_else(|| std::path::Path::new("."));
    let file_name = log_path
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("triptych.log"));
    let (non_blocking, guard) =
        tracing_appender::non_blocking(tracing_appender::rolling::never(dir, file_name));

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(non_blocking)
        .with_ansi(false)
        .init();

    guard
}
