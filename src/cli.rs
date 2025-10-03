use clap::{Parser, Subcommand};

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
}
