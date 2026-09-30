pub mod commands;
pub mod daemon;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "triptych")]
#[command(about = "Terminal productivity suite", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Add a new task
    Add { description: String },

    /// List all tasks
    List,

    /// Mark a task as done
    Done { id: i64 },

    /// Remove a task
    Rm { id: i64 },

    /// Clear completed tasks
    Clear,

    /// Start the background daemon
    Daemon,

    /// Stop the background daemon
    Stop,

    /// Check daemon status
    Status,

    /// Schedule management commands
    #[command(subcommand)]
    Schedule(ScheduleCommands),

    /// Email commands
    #[command(subcommand)]
    Email(EmailCommands),

    /// Canvas assignment commands
    #[command(subcommand)]
    Canvas(CanvasCommands),
}

#[derive(Subcommand)]
pub enum CanvasCommands {
    /// One-shot fetch of the Canvas feed (`CANVAS_ICS_URL`) into the todo list
    Sync,
}

#[derive(Subcommand)]
pub enum EmailCommands {
    /// One-shot fetch of new mail over IMAP into local storage
    Sync {
        /// Forget sync cursors first, so the last 6 months are fetched again (duplicates are skipped)
        #[arg(long)]
        backfill: bool,
    },

    /// Print recently stored emails
    List,
}

#[derive(Subcommand)]
pub enum ScheduleCommands {
    /// Import schedule blocks from a TOML file
    Import {
        /// Path to TOML file
        file: PathBuf,
        /// Clear existing blocks before import
        #[arg(long)]
        clear: bool,
    },

    /// Export current schedule to a TOML file
    Export {
        /// Output file path
        #[arg(default_value = "schedule.toml")]
        file: PathBuf,
    },

    /// Show current week's schedule
    Show,

    /// Clear all schedule blocks
    Clear,

    /// Reallocate all deadline-bearing tasks to available deepwork/admin blocks
    Reallocate,
}
