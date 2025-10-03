mod app;
mod cli;
mod nlp;
mod sync;
mod ui;

use crate::app::InputMode;
use crate::ui::ui;
use app::App;
use clap::Parser;
use cli::{Cli, Commands};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use ratatui::{
    Terminal,
    backend::{Backend, CrosstermBackend},
};
use std::io;
use sync::{SyncConfig, SyncDaemon};
use tokio::signal;
use tracing_appender::rolling::{RollingFileAppender, Rotation};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let file_appender = RollingFileAppender::new(
        Rotation::DAILY,
        "logs", // Log directory
        "triptych.log",
    );

    tracing_subscriber::fmt()
        .with_writer(file_appender)
        .with_ansi(false) // No color codes in file
        .init();

    let cli_args = Cli::parse();
    let mut app = App::build().await?;

    // Start background sync daemon before TUI
    let sync_config = SyncConfig::default();
    let daemon = SyncDaemon::start(app.db_pool.clone(), app.nlp_parser_ref(), sync_config).await?;

    // Give warmup time to complete
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Check if a subcommand was provided
    if let Some(command) = cli_args.command {
        let result = handle_cli_command(&mut app, command).await;
        daemon.shutdown().await?;
        return result;
    }

    // Now start the TUI
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    app.load_tasks().await?;

    // Run TUI
    let tui_result = run_app(&mut terminal, app).await;

    // Cleanup terminal (always runs, even on Ctrl+C via run_app)
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    // Gracefully shutdown daemon
    daemon.shutdown().await?;

    tui_result?;
    Ok(())
}

// Extract CLI command handling into separate function for cleaner code
async fn handle_cli_command(
    app: &mut App,
    command: Commands,
) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        Commands::Add { description } => match app.add_task(&description).await {
            Ok(_) => println!("✓ Added task: \"{}\"", description),
            Err(e) => {
                eprintln!("✗ Error adding task: {}", e);
                std::process::exit(1);
            }
        },

        Commands::List => {
            match app.get_enhanced_task_list().await {
                Ok(enhanced_tasks) => {
                    if enhanced_tasks.is_empty() {
                        println!(
                            "📝 No tasks yet! Add one with: {} add \"Your task description\"",
                            std::env::args()
                                .next()
                                .unwrap_or_else(|| "todo".to_string())
                        );
                    } else {
                        println!("📋 Current Tasks:");
                        for enhanced in &enhanced_tasks {
                            let task = &enhanced.task;
                            let status = if task.completed { "✓" } else { "○" };

                            let mut indicators = Vec::new();

                            // Priority indicator
                            match task.priority {
                                3 => indicators.push("[HIGH]".to_string()),
                                2 => indicators.push("[MED]".to_string()),
                                1 => indicators.push("[LOW]".to_string()),
                                _ => {}
                            }

                            // Schedule indicator
                            if let Some(scheduled) = task.scheduled_at {
                                let now = chrono::Utc::now();
                                let scheduled_date = scheduled.date_naive();
                                let today = now.date_naive();
                                let tomorrow = today + chrono::Duration::days(1);

                                let date_text = if scheduled_date == today {
                                    "[TODAY]"
                                } else if scheduled_date == tomorrow {
                                    "[TOMORROW]"
                                } else {
                                    &format!("[{}]", scheduled.format("%m/%d"))
                                };
                                indicators.push(date_text.to_string());
                            }

                            let indicators_str = if indicators.is_empty() {
                                String::new()
                            } else {
                                format!("{} ", indicators.join(" "))
                            };

                            let tags_display = if !enhanced.tags.is_empty() {
                                format!(" #{}", enhanced.tags.join(" #"))
                            } else {
                                String::new()
                            };

                            let description = if task.completed {
                                format!("\x1b[9m{}\x1b[0m", task.description)
                            } else {
                                task.description.clone()
                            };

                            println!(
                                "  {} {}{} (ID: {}){}",
                                status, indicators_str, description, task.id, tags_display
                            );
                        }
                    }
                }
                Err(e) => {
                    eprintln!("✗ Error loading tasks: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::Done { id } => match app.complete_task_by_id(id).await {
            Ok(true) => {
                if let Ok(Some(task)) = app.get_task_by_id(id).await {
                    println!("✓ Marked task as done: \"{}\"", task.description);
                } else {
                    println!("✓ Marked task {} as done", id);
                }
            }
            Ok(false) => {
                eprintln!("✗ Task with ID {} not found", id);
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("✗ Error completing task: {}", e);
                std::process::exit(1);
            }
        },

        Commands::Rm { id } => match app.remove_task_by_id(id).await {
            Ok(true) => println!("✓ Removed task with ID {}", id),
            Ok(false) => {
                eprintln!("✗ Task with ID {} not found", id);
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("✗ Error removing task: {}", e);
                std::process::exit(1);
            }
        },

        Commands::Clear => match app.clear_completed_tasks().await {
            Ok(count) => {
                if count == 0 {
                    println!("🧹 No completed tasks to clear");
                } else {
                    println!(
                        "🧹 Cleared {} completed task{}",
                        count,
                        if count == 1 { "" } else { "s" }
                    );
                }
            }
            Err(e) => {
                eprintln!("✗ Error clearing completed tasks: {}", e);
                std::process::exit(1);
            }
        },
    }

    Ok(())
}

async fn run_app<B: Backend>(terminal: &mut Terminal<B>, mut app: App) -> io::Result<()> {
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::mpsc::channel::<()>(1);

    // Spawn Ctrl+C handler
    tokio::spawn(async move {
        if signal::ctrl_c().await.is_ok() {
            let _ = shutdown_tx.send(()).await;
        }
    });

    // Create async event stream (crossterm's async API)
    let mut reader = EventStream::new();

    loop {
        terminal.draw(|f| ui(f, &app))?;

        // Wait for either keyboard event or shutdown signal
        tokio::select! {
            // Keyboard event (async, zero lag!)
            maybe_event = reader.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) => {
                        match app.input_mode {
                            InputMode::Normal => match key.code {
                                KeyCode::Char('q') => return Ok(()),
                                KeyCode::Char('a') => {
                                    app.input_mode = InputMode::Editing;
                                    app.input_buffer.clear();
                                }
                                KeyCode::Char('x') => {
                                    if let Err(e) = app.delete_task().await {
                                        eprintln!("Error deleting task: {}", e);
                                    }
                                }
                                KeyCode::Enter => {
                                    if let Err(e) = app.toggle_completed().await {
                                        eprintln!("Error toggling task: {}", e);
                                    }
                                }
                                KeyCode::Char('k') => {
                                    app.selected = app.selected.saturating_sub(1);
                                }
                                KeyCode::Char('j') => {
                                    if !app.tasks.is_empty() {
                                        let max = app.tasks.len() - 1;
                                        if app.selected < max {
                                            app.selected += 1;
                                        }
                                    }
                                }
                                _ => {}
                            },

                            InputMode::Editing => match key.code {
                                KeyCode::Enter => {
                                    let description = app.input_buffer.trim().to_string();
                                    if !description.is_empty()
                                        && let Err(e) = app.add_task(&description).await {
                                            eprintln!("Error adding task: {}", e);
                                        }
                                    app.input_mode = InputMode::Normal;
                                }
                                KeyCode::Char(c) => {
                                    app.input_buffer.push(c);
                                }
                                KeyCode::Backspace => {
                                    app.input_buffer.pop();
                                }
                                KeyCode::Esc => {
                                    app.input_mode = InputMode::Normal;
                                }
                                _ => {}
                            },
                        }
                    }
                    Some(Ok(_)) => {} // Other events (mouse, resize, etc.)
                    Some(Err(e)) => {
                        eprintln!("Error reading event: {}", e);
                    }
                    None => break, // Stream ended
                }
            }

            // Shutdown signal
            _ = shutdown_rx.recv() => {
                return Ok(());
            }
        }
    }

    Ok(())
}
