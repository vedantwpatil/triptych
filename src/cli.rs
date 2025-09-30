use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Add a new task to the list
    Add {
        /// The description of the task
        description: String,
    },
    /// List all current tasks
    List,
    /// Mark a task as done by its ID
    Done {
        /// The ID of the task to mark as done
        id: i64,
    },
    /// Remove a task by its ID
    Rm {
        /// The ID of the task to remove
        id: i64,
    },
    /// Clear all completed tasks from the list
    Clear,
}
