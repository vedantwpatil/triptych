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
}
