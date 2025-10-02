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
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::{Backend, CrosstermBackend},
};
use std::io;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli_args = Cli::parse();
    let mut app = App::build().await?;

    // Check if a subcommand was provided
    if let Some(command) = cli_args.command {
        // A subcommand was provided - handle it and exit
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

        // Exit after handling CLI command - don't start the TUI
        return Ok(());
    }

    // No subcommand was provided - start the TUI
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    app.load_tasks().await?;
    run_app(&mut terminal, app).await?;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}
async fn run_app<B: Backend>(terminal: &mut Terminal<B>, mut app: App) -> io::Result<()> {
    loop {
        terminal.draw(|f| ui(f, &app))?;

        if let Event::Key(key) = event::read()? {
            match app.input_mode {
                // Normal Mode
                InputMode::Normal => match key.code {
                    KeyCode::Char('q') => return Ok(()),
                    KeyCode::Char('a') => {
                        // Switch to editing mode and clear the buffer
                        app.input_mode = InputMode::Editing;
                        app.input_buffer.clear();
                    }
                    KeyCode::Char('x') => app.delete_task().await.unwrap(),
                    KeyCode::Enter => app.toggle_completed().await.unwrap(),
                    KeyCode::Char('k') => app.selected = app.selected.saturating_sub(1),
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

                // Editing Mode
                InputMode::Editing => match key.code {
                    KeyCode::Enter => {
                        // Save the new task and return to normal mode
                        let description = app.input_buffer.trim().to_string();
                        if !description.is_empty() {
                            app.add_task(&description).await.unwrap();
                        }
                        app.input_mode = InputMode::Normal;
                    }
                    KeyCode::Char(c) => {
                        // Add character to the input buffer
                        app.input_buffer.push(c);
                    }
                    KeyCode::Backspace => {
                        // Remove the last character
                        app.input_buffer.pop();
                    }
                    KeyCode::Esc => {
                        // Cancel and return to normal mode
                        app.input_mode = InputMode::Normal;
                    }
                    _ => {}
                },
            }
        }
    }
}
