//! Triptych: a keyboard-driven terminal todo list, weekly calendar and email client.
//!
//! The binary (`src/main.rs`) is a thin wrapper around [`run`]; everything else lives here so the
//! integration tests in `tests/` can exercise it.

pub mod app;
mod cli;
pub mod email;
pub mod logging;
pub mod migrations;
pub mod nlp;
mod sync;
mod tui;
pub mod urgency;

pub use tui::ui;

use app::App;
use clap::Parser;
use cli::{Cli, Commands, commands, daemon};
use migrations::{run_calendar_migration, run_email_migration};

/// The error type of every fallible top-level entry point: callers only report it, never branch on it.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Parses the command line and runs it: a daemon command, a one-shot subcommand, or the TUI.
///
/// # Errors
/// Returns any database, IPC or terminal error from the command that ran.
pub async fn run() -> Result<(), BoxError> {
    // Loads `.env` from the cwd or a parent dir into the process env, without
    // overriding any var already set (so a real shell export, or the sandbox
    // env `tests/it/cli.rs` passes to the child process, always wins). Nothing
    // else in this codebase reads `.env` - previously every IMAP_*/
    // TRIPTYCH_EMAIL_ENABLED var only took effect if the launching shell had
    // sourced `.env` itself; a fresh terminal or process launched without
    // that meant `EmailConfig::all_from_env()` silently returned empty and
    // mail sync no-op'd, with no error surfaced anywhere obvious.
    let _ = dotenvy::dotenv();

    let cli_args = Cli::parse();

    // Handle daemon commands first
    if matches!(&cli_args.command, Some(Commands::Daemon)) {
        let app = App::build().await?;
        daemon::start_daemon(app.db_pool.clone(), app.nlp_parser_ref()).await?;
        return Ok(());
    }

    if matches!(&cli_args.command, Some(Commands::Stop)) {
        daemon::stop_daemon().await?;
        return Ok(());
    }

    if matches!(&cli_args.command, Some(Commands::Status)) {
        if daemon::is_daemon_running().await {
            println!("✓ Daemon is running");
        } else {
            println!("✗ Daemon is not running");
            println!("  Start with: triptych daemon");
            std::process::exit(1);
        }
        return Ok(());
    }

    // Build app for other commands
    let mut app = App::build().await?;

    if let Err(e) = run_calendar_migration(&app.db_pool).await {
        eprintln!("⚠ Calendar migration failed: {e}");
        eprintln!("   Calendar features will be disabled");
    }

    if let Err(e) = run_email_migration(&app.db_pool).await {
        eprintln!("⚠ Email migration failed: {e}");
        eprintln!("   Email features will be disabled");
    }

    // Check if a subcommand was provided
    if let Some(command) = cli_args.command {
        return commands::handle_cli_command(&mut app, command).await;
    }

    // No subcommand: start the TUI
    tui::run(app).await
}
