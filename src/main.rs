mod app;
mod cli;
mod daemon;
mod nlp;
mod sync;
mod ui;

use crate::app::{BlockFormState, CalendarInputMode, InputMode, ViewMode};
use crate::ui::ui;
mod migrations;
use app::App;
use clap::Parser;
use cli::{Cli, Commands, ScheduleCommands};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use daemon::{DaemonRequest, DaemonResponse};
use futures::StreamExt;
use migrations::run_calendar_migration;
use ratatui::{
    Terminal,
    backend::{Backend, CrosstermBackend},
};
use std::io;
use sync::{SyncConfig, SyncDaemon};
use tokio::signal;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli_args = Cli::parse();

    // Handle daemon commands first
    if let Some(Commands::Daemon) = &cli_args.command {
        let app = App::build().await?;
        daemon::start_daemon(app.db_pool.clone(), app.nlp_parser_ref()).await?;
        return Ok(());
    }

    if let Some(Commands::Stop) = &cli_args.command {
        daemon::stop_daemon().await?;
        return Ok(());
    }

    if let Some(Commands::Status) = &cli_args.command {
        if daemon::is_daemon_running().await {
            println!("✓ Daemon is running");
        } else {
            println!("✗ Daemon is not running");
            println!("  Start with: triptych daemon");
        }
        return Ok(());
    }

    // Build app for other commands
    let mut app = App::build().await?;

    if let Err(e) = run_calendar_migration(&app.db_pool).await {
        eprintln!("⚠️  Calendar migration failed: {}", e);
        eprintln!("   Calendar features will be disabled");
    }

    // Check if a subcommand was provided
    if let Some(command) = cli_args.command {
        let result = handle_cli_command(&mut app, command).await;
        return result;
    }

    // No subcommand - start the TUI (with sync daemon)
    // Start sync daemon BEFORE entering alternate screen so warmup messages print cleanly
    let sync_config = SyncConfig::from_env();
    let daemon = SyncDaemon::start(app.db_pool.clone(), app.nlp_parser_ref(), sync_config).await?;

    app.load_tasks().await?;

    // Now enter TUI mode
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let tui_result = run_app(&mut terminal, app).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    daemon.shutdown().await?;
    tui_result?;
    Ok(())
}

async fn handle_cli_command(
    app: &mut App,
    command: Commands,
) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        Commands::Add { description } => {
            // Try daemon first for instant response
            if daemon::is_daemon_running().await {
                match daemon::send_to_daemon(DaemonRequest::AddTask {
                    description: description.clone(),
                })
                .await
                {
                    Ok(DaemonResponse::TaskAdded { id }) => {
                        println!("✓ Added task: \"{}\" (ID: {}, via daemon)", description, id);
                        return Ok(());
                    }
                    Ok(DaemonResponse::Error(e)) => {
                        eprintln!("⚠️  Daemon error: {}", e);
                        eprintln!("   Falling back to direct mode...");
                    }
                    Err(e) => {
                        eprintln!("⚠️  Daemon communication error: {}", e);
                        eprintln!("   Falling back to direct mode...");
                    }
                    _ => {
                        eprintln!("⚠️  Unexpected daemon response");
                        eprintln!("   Falling back to direct mode...");
                    }
                }
            }

            // Fallback: direct execution
            match app.add_task(&description).await {
                Ok(_) => println!("✓ Added task: \"{}\"", description),
                Err(e) => {
                    eprintln!("✗ Error adding task: {}", e);
                    std::process::exit(1);
                }
            }
        }

        Commands::List => {
            // List command logic (unchanged)
            match app.get_enhanced_task_list().await {
                Ok(enhanced_tasks) => {
                    if enhanced_tasks.is_empty() {
                        println!("📝 No tasks yet! Add one with: triptych add \"Your task\"");
                    } else {
                        println!("📋 Current Tasks:");
                        for enhanced in &enhanced_tasks {
                            let task = &enhanced.task;
                            let status = if task.completed { "✓" } else { "○" };
                            let mut indicators = Vec::new();

                            match task.priority {
                                3 => indicators.push("[HIGH]".to_string()),
                                2 => indicators.push("[MED]".to_string()),
                                1 => indicators.push("[LOW]".to_string()),
                                _ => {}
                            }

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

        Commands::Schedule(schedule_cmd) => match schedule_cmd {
            ScheduleCommands::Import { file, clear } => {
                if clear {
                    app.clear_all_schedule_blocks().await?;
                    println!("Cleared existing blocks");
                }
                match app.import_schedule_from_toml(&file).await {
                    Ok(count) => println!("✓ Imported {} schedule blocks from {:?}", count, file),
                    Err(e) => {
                        eprintln!("✗ Import failed: {}", e);
                        std::process::exit(1);
                    }
                }
            }
            ScheduleCommands::Export { file } => match app.export_schedule_to_toml(&file).await {
                Ok(count) => println!("✓ Exported {} schedule blocks to {:?}", count, file),
                Err(e) => {
                    eprintln!("✗ Export failed: {}", e);
                    std::process::exit(1);
                }
            },
            ScheduleCommands::Show => {
                app.print_schedule_summary().await?;
            }
            ScheduleCommands::Clear => {
                let count = app.clear_all_schedule_blocks().await?;
                println!("🧹 Cleared {} schedule blocks", count);
            }
        },

        _ => unreachable!("Daemon commands handled earlier"),
    }

    Ok(())
}

async fn run_app<B: Backend>(terminal: &mut Terminal<B>, mut app: App) -> io::Result<()>
where
    std::io::Error: std::convert::From<<B as ratatui::backend::Backend>::Error>,
{
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
                            InputMode::Normal => {
                                match app.view_mode {
                                    ViewMode::TodoList => match key.code {
                                        KeyCode::Char('q') => return Ok(()),
                                        KeyCode::Char('c') => { app.toggle_to_calendar().await; }
                                        KeyCode::Char('a') => {
                                            app.input_mode = InputMode::Editing;
                                            app.input_buffer.clear();
                                        }
                                        KeyCode::Char('x') => {
                                            if let Err(e) = app.delete_task().await {
                                                app.status_message = Some((format!("Error: {}", e), std::time::Instant::now()));
                                            }
                                        }
                                        KeyCode::Char('s') => {
                                            if let Err(e) = app.auto_schedule_task().await {
                                                app.status_message = Some((format!("Error: {}", e), std::time::Instant::now()));
                                            }
                                        }
                                        KeyCode::Enter => {
                                            if let Err(e) = app.toggle_completed().await {
                                                app.status_message = Some((format!("Error: {}", e), std::time::Instant::now()));
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
                                    ViewMode::Calendar => match app.calendar_input_mode {
                                        CalendarInputMode::Navigate => match key.code {
                                            KeyCode::Char('q') => return Ok(()),
                                            KeyCode::Char('t') | KeyCode::Esc => { app.toggle_to_todo().await; }
                                            KeyCode::Char('j') | KeyCode::Down => app.calendar_move_down(),
                                            KeyCode::Char('k') | KeyCode::Up => app.calendar_move_up(),
                                            KeyCode::Char('h') | KeyCode::Left => app.calendar_move_left(),
                                            KeyCode::Char('l') | KeyCode::Right => app.calendar_move_right(),
                                            KeyCode::Char('H') => { app.prev_week().await; }
                                            KeyCode::Char('L') => { app.next_week().await; }
                                            KeyCode::Char('n') => {
                                                app.block_form = BlockFormState::new_at(app.selected_time_slot);
                                                app.calendar_input_mode = CalendarInputMode::BlockForm;
                                            }
                                            KeyCode::Char('s') => {
                                                app.task_picker_selected = 0;
                                                app.calendar_input_mode = CalendarInputMode::TaskPicker;
                                            }
                                            KeyCode::Char('a') => {
                                                app.input_buffer.clear();
                                                app.calendar_input_mode = CalendarInputMode::TaskInput;
                                            }
                                            KeyCode::Char('d') => {
                                                if let Err(e) = app.delete_block_at_selected_cell().await {
                                                    app.status_message = Some((format!("Error: {}", e), std::time::Instant::now()));
                                                }
                                            }
                                            _ => {}
                                        },
                                        CalendarInputMode::BlockForm => match key.code {
                                            KeyCode::Esc => {
                                                app.calendar_input_mode = CalendarInputMode::Navigate;
                                            }
                                            KeyCode::Tab => app.block_form.next_field(),
                                            KeyCode::BackTab => app.block_form.prev_field(),
                                            KeyCode::Enter => {
                                                if !app.block_form.title.is_empty()
                                                    && let Err(e) = app.create_schedule_block().await {
                                                        app.status_message = Some((format!("Error: {}", e), std::time::Instant::now()));
                                                    }
                                            }
                                            KeyCode::Char(c) => {
                                                match app.block_form.active_field {
                                                    crate::app::BlockFormField::BlockType => {
                                                        if c == 'j' {
                                                            app.block_form.cycle_block_type(true);
                                                        } else if c == 'k' {
                                                            app.block_form.cycle_block_type(false);
                                                        }
                                                    }
                                                    crate::app::BlockFormField::StartTime => {
                                                        if c.is_ascii_digit() || c == ':' {
                                                            app.block_form.start_time.push(c);
                                                        }
                                                    }
                                                    crate::app::BlockFormField::EndTime => {
                                                        if c.is_ascii_digit() || c == ':' {
                                                            app.block_form.end_time.push(c);
                                                        }
                                                    }
                                                    crate::app::BlockFormField::Title => {
                                                        app.block_form.title.push(c);
                                                    }
                                                }
                                            }
                                            KeyCode::Backspace => {
                                                match app.block_form.active_field {
                                                    crate::app::BlockFormField::StartTime => {
                                                        app.block_form.start_time.pop();
                                                    }
                                                    crate::app::BlockFormField::EndTime => {
                                                        app.block_form.end_time.pop();
                                                    }
                                                    crate::app::BlockFormField::Title => {
                                                        app.block_form.title.pop();
                                                    }
                                                    _ => {}
                                                }
                                            }
                                            _ => {}
                                        },
                                        CalendarInputMode::TaskPicker => match key.code {
                                            KeyCode::Esc => {
                                                app.calendar_input_mode = CalendarInputMode::Navigate;
                                            }
                                            KeyCode::Char('j') | KeyCode::Down => {
                                                let max = app.unscheduled_tasks().len().saturating_sub(1);
                                                if app.task_picker_selected < max {
                                                    app.task_picker_selected += 1;
                                                }
                                            }
                                            KeyCode::Char('k') | KeyCode::Up => {
                                                app.task_picker_selected = app.task_picker_selected.saturating_sub(1);
                                            }
                                            KeyCode::Enter => {
                                                if !app.unscheduled_tasks().is_empty() {
                                                    if let Err(e) = app.schedule_task_to_selected_cell().await {
                                                        app.status_message = Some((format!("Error: {}", e), std::time::Instant::now()));
                                                    }
                                                }
                                            }
                                            _ => {}
                                        },
                                        CalendarInputMode::TaskInput => match key.code {
                                            KeyCode::Esc => {
                                                app.input_buffer.clear();
                                                app.calendar_input_mode = CalendarInputMode::Navigate;
                                            }
                                            KeyCode::Enter => {
                                                let description = app.input_buffer.trim().to_string();
                                                if !description.is_empty()
                                                    && let Err(e) = app.add_task_at_selected_cell(&description).await {
                                                        app.status_message = Some((format!("Error: {}", e), std::time::Instant::now()));
                                                    }
                                                app.input_buffer.clear();
                                                app.calendar_input_mode = CalendarInputMode::Navigate;
                                            }
                                            KeyCode::Char(c) => {
                                                app.input_buffer.push(c);
                                            }
                                            KeyCode::Backspace => {
                                                app.input_buffer.pop();
                                            }
                                            _ => {}
                                        },
                                    },
                                }
                            }

                            InputMode::Editing => match key.code {
                                KeyCode::Enter => {
                                    let description = app.input_buffer.trim().to_string();
                                    if !description.is_empty()
                                        && let Err(e) = app.add_task(&description).await {
                                            app.status_message = Some((format!("Error: {}", e), std::time::Instant::now()));
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
                        app.status_message = Some((format!("Input error: {}", e), std::time::Instant::now()));
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
