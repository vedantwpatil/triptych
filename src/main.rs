mod app;
mod cli;
mod daemon;
mod email;
mod keys;
mod nlp;
mod sync;
mod ui;

use crate::keys::KeyOutcome;
use crate::ui::ui;
mod migrations;
use app::App;
use clap::Parser;
use cli::{Cli, Commands, EmailCommands, ScheduleCommands};
use email::{EmailConfig, ImapMailSource, MailSource, message, store};
use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture, Event, EventStream},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use daemon::{DaemonRequest, DaemonResponse};
use futures::StreamExt;
use migrations::{run_calendar_migration, run_email_migration};
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
        eprintln!("⚠ Calendar migration failed: {}", e);
        eprintln!("   Calendar features will be disabled");
    }

    if let Err(e) = run_email_migration(&app.db_pool).await {
        eprintln!("⚠ Email migration failed: {}", e);
        eprintln!("   Email features will be disabled");
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
    install_panic_hook();
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
                        eprintln!("⚠ Daemon error: {}", e);
                        eprintln!("   Falling back to direct mode...");
                    }
                    Err(e) => {
                        eprintln!("⚠ Daemon communication error: {}", e);
                        eprintln!("   Falling back to direct mode...");
                    }
                    _ => {
                        eprintln!("⚠ Unexpected daemon response");
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
                        println!("ℹ No tasks yet! Add one with: triptych add \"Your task\"");
                    } else {
                        println!("▸ Current Tasks:");
                        for enhanced in &enhanced_tasks {
                            let task = &enhanced.task;
                            let status = if task.completed { "✓" } else { "○" };
                            let mut indicators = Vec::new();

                            match task.priority {
                                3 => indicators.push("[URGENT]".to_string()),
                                2 => indicators.push("[HIGH]".to_string()),
                                1 => indicators.push("[MED]".to_string()),
                                _ => {}
                            }

                            if let Some(scheduled) = task.scheduled_at {
                                let scheduled = scheduled.with_timezone(&chrono::Local);
                                let now = chrono::Local::now();
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
                    println!("✓ No completed tasks to clear");
                } else {
                    println!(
                        "✓ Cleared {} completed task{}",
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
                println!("✓ Cleared {} schedule blocks", count);
            }
            ScheduleCommands::Reallocate => match app.reallocate_all_tasks().await {
                Ok(result) => {
                    if let Some(summary) = result.conflict_summary() {
                        println!("⚠ {}", summary);
                        for conflict in &result.conflicts {
                            println!(
                                "  - \"{}\" (ID: {}) needs {}m, got {}m (due {}) - {}",
                                conflict.description,
                                conflict.task_id,
                                conflict.needed_minutes,
                                conflict.allocated_minutes,
                                conflict
                                    .deadline
                                    .with_timezone(&chrono::Local)
                                    .format("%a %m/%d %H:%M"),
                                conflict.reason
                            );
                        }
                    } else {
                        println!("✓ All deadline tasks fit within available blocks");
                    }
                }
                Err(e) => {
                    eprintln!("✗ Reallocation failed: {}", e);
                    std::process::exit(1);
                }
            },
        },

        Commands::Email(email_cmd) => match email_cmd {
            EmailCommands::Sync => {
                let configs = EmailConfig::all_from_env();
                if configs.is_empty() {
                    eprintln!("✗ Email not configured (set TRIPTYCH_EMAIL_ENABLED=true and IMAP_* in .env)");
                    std::process::exit(1);
                }

                let mut any_failed = false;
                for config in &configs {
                    let last_uid = store::max_uid(&app.db_pool, &config.account, &config.imap_folder)
                        .await
                        .map_err(|e| e.to_string())?;

                    let source = ImapMailSource::new(config.clone());
                    match source.fetch_new(last_uid.map(|uid| uid as u32)).await {
                        Ok(raw_messages) => {
                            let new_emails: Vec<_> = raw_messages
                                .into_iter()
                                .filter_map(|(uid, raw)| {
                                    message::parse_raw(&config.account, uid, &config.imap_folder, &raw)
                                        .ok()
                                })
                                .collect();

                            let count = new_emails.len();
                            store::insert_new(&app.db_pool, &new_emails)
                                .await
                                .map_err(|e| e.to_string())?;
                            println!("✓ [{}] Synced {} new email(s)", config.account, count);
                        }
                        Err(e) => {
                            eprintln!("✗ [{}] Sync failed: {}", config.account, e);
                            any_failed = true;
                        }
                    }
                }

                if any_failed {
                    std::process::exit(1);
                }
            }
            EmailCommands::List => {
                let emails = store::get_recent(&app.db_pool, 50)
                    .await
                    .map_err(|e| e.to_string())?;
                if emails.is_empty() {
                    println!("📭 No emails yet! Sync with: triptych email sync");
                } else {
                    for email in &emails {
                        let status = if email.is_read { " " } else { "*" };
                        let from = email.from_name.as_deref().unwrap_or(&email.from_addr);
                        println!(
                            "  {} ({}) [{}] {:20} {} (ID: {})",
                            status,
                            email.account,
                            email.date_utc.with_timezone(&chrono::Local).format("%m/%d %H:%M"),
                            from,
                            email.subject,
                            email.id
                        );
                    }
                }
            }
        },

        _ => unreachable!("Daemon commands handled earlier"),
    }

    Ok(())
}

/// Restore the terminal before a panic's default report prints, since unwinding past
/// `run_app` would otherwise skip the raw-mode/alternate-screen cleanup in `main`.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        default_hook(panic_info);
    }));
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
        terminal.draw(|f| ui(f, &mut app))?;

        // Wait for either keyboard event or shutdown signal
        tokio::select! {
            // Keyboard event (async, zero lag!)
            maybe_event = reader.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) => {
                        if let KeyOutcome::Quit = keys::handle_key_event(&mut app, key).await {
                            return Ok(());
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
