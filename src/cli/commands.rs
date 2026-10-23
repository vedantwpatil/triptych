//! Non-interactive CLI subcommands (`add`, `list`, `schedule ...`, `email ...`).

use super::daemon::{self, DaemonRequest, DaemonResponse};
use crate::app::{App, open_pool};
use crate::canvas;
use crate::cli::{CanvasCommands, Commands, EmailCommands, NotifyCommands, ScheduleCommands};
use crate::email::{EmailConfig, store, sync::sync_account};
use crate::migrations::run_calendar_migration;
use crate::{jobs, notify, urgency};

// One match arm per CLI subcommand - splitting it up would scatter each
// subcommand's handling across functions without reducing complexity.
#[allow(clippy::too_many_lines)]
pub async fn handle_cli_command(app: &mut App, command: Commands) -> Result<(), crate::BoxError> {
    match command {
        Commands::Add { description } => {
            if description.trim().is_empty() {
                eprintln!("✗ Task description cannot be empty");
                std::process::exit(1);
            }
            // Try daemon first for instant response
            if daemon::is_daemon_running().await {
                match daemon::send_to_daemon(DaemonRequest::AddTask {
                    description: description.clone(),
                })
                .await
                {
                    Ok(DaemonResponse::TaskAdded { id }) => {
                        println!("✓ Added task: \"{description}\" (ID: {id}, via daemon)");
                        return Ok(());
                    }
                    Ok(DaemonResponse::Error(e)) => {
                        eprintln!("▲ Daemon error: {e}");
                        eprintln!("   Falling back to direct mode...");
                    }
                    Err(e) => {
                        eprintln!("▲ Daemon communication error: {e}");
                        eprintln!("   Falling back to direct mode...");
                    }
                    _ => {
                        eprintln!("▲ Unexpected daemon response");
                        eprintln!("   Falling back to direct mode...");
                    }
                }
            }

            // Fallback: direct execution
            match app.add_task(&description).await {
                Ok(id) => println!("✓ Added task: \"{description}\" (ID: {id})"),
                Err(e) => {
                    eprintln!("✗ Error adding task: {e}");
                    std::process::exit(1);
                }
            }
        }

        Commands::List { all } => {
            // List command logic (unchanged)
            match app.get_enhanced_task_list().await {
                Ok(enhanced_tasks) => {
                    let visible: Vec<_> = enhanced_tasks
                        .iter()
                        .filter(|e| all || !e.task.completed)
                        .collect();
                    if enhanced_tasks.is_empty() {
                        println!("∅ No tasks yet! Add one with: triptych add \"Your task\"");
                    } else if visible.is_empty() {
                        println!("∅ No open tasks (`triptych list --all` shows completed)");
                    } else {
                        println!("▸ Current Tasks:");
                        for enhanced in &visible {
                            let task = &enhanced.task;
                            let status = if task.completed { "✓" } else { "○" };
                            let mut indicators = Vec::new();

                            if let Some((_, badge)) =
                                urgency::priority_badge(task, crate::clock::now())
                            {
                                indicators.push(badge);
                            }

                            if let Some(scheduled) = task.scheduled_at {
                                let scheduled = scheduled.with_timezone(&chrono::Local);
                                let now = crate::clock::local_now();
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

                            if let (Some(deadline), false) = (task.deadline, task.completed) {
                                indicators.push(urgency::deadline_badge(
                                    deadline,
                                    crate::clock::local_now(),
                                ));
                            }

                            if let Some(rule) = &task.recurrence {
                                indicators.push(format!("[↻ {rule}]"));
                            }

                            let indicators_str = if indicators.is_empty() {
                                String::new()
                            } else {
                                format!("{} ", indicators.join(" "))
                            };

                            let tags_display = if enhanced.tags.is_empty() {
                                String::new()
                            } else {
                                format!(" #{}", enhanced.tags.join(" #"))
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
                    eprintln!("✗ Error loading tasks: {e}");
                    std::process::exit(1);
                }
            }
        }

        Commands::Done { id } => match app.complete_task_by_id(id).await {
            Ok(true) => {
                if let Ok(Some(task)) = app.get_task_by_id(id).await {
                    println!("✓ Marked task as done: \"{}\"", task.description);
                } else {
                    println!("✓ Marked task {id} as done");
                }
            }
            Ok(false) => {
                eprintln!("✗ Task with ID {id} not found");
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("✗ Error completing task: {e}");
                std::process::exit(1);
            }
        },

        Commands::Rm { id } => match app.remove_task_by_id(id).await {
            Ok(true) => println!("✓ Removed task with ID {id}"),
            Ok(false) => {
                eprintln!("✗ Task with ID {id} not found");
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("✗ Error removing task: {e}");
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
                eprintln!("✗ Error clearing completed tasks: {e}");
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
                    Ok(count) => {
                        println!("✓ Imported {count} schedule blocks from {}", file.display());
                    }
                    Err(e) => {
                        eprintln!("✗ Import failed: {e}");
                        std::process::exit(1);
                    }
                }
            }
            ScheduleCommands::Export { file } => match app.export_schedule_to_toml(&file).await {
                Ok(count) => {
                    println!("✓ Exported {count} schedule blocks to {}", file.display());
                }
                Err(e) => {
                    eprintln!("✗ Export failed: {e}");
                    std::process::exit(1);
                }
            },
            ScheduleCommands::Show => {
                app.print_schedule_summary().await?;
            }
            ScheduleCommands::Clear => {
                let count = app.clear_all_schedule_blocks().await?;
                println!("✓ Cleared {count} schedule blocks");
            }
            ScheduleCommands::Reallocate => match app.reallocate_all_tasks().await {
                Ok(result) => {
                    if let Some(summary) = result.conflict_summary() {
                        println!("▲ {summary}");
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
                    eprintln!("✗ Reallocation failed: {e}");
                    std::process::exit(1);
                }
            },
        },

        Commands::Jobs { all } => {
            let new = jobs::scan(&app.db_pool, &jobs::Profile::load(), crate::clock::now())
                .await
                .map_err(|e| e.to_string())?;
            let rows = jobs::list(&app.db_pool, all)
                .await
                .map_err(|e| e.to_string())?;
            if rows.is_empty() {
                println!("∅ No job postings yet! They come from digest mail: triptych email sync");
            }
            for (score, company, role, status, reason) in rows {
                println!("{score:>3}  {company} - {role}  [{status}] {reason}");
            }
            println!("✓ {} new task(s)", new.len());
        }
        Commands::Canvas(CanvasCommands::Sync) => {
            let Some(url) = canvas::feed_url_from_env() else {
                eprintln!(
                    "✗ Canvas not configured (set {} in .env)",
                    canvas::FEED_URL_ENV
                );
                std::process::exit(1);
            };
            match canvas::sync(&app.db_pool, &url).await {
                Ok(r) => println!("✓ Canvas: {} added, {} updated", r.added, r.updated),
                Err(e) => {
                    eprintln!("✗ Canvas sync failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::Email(email_cmd) => match email_cmd {
            EmailCommands::Sync { backfill } => {
                let configs = EmailConfig::all_from_env();
                if configs.is_empty() {
                    eprintln!(
                        "✗ Email not configured (set TRIPTYCH_EMAIL_ENABLED=true and IMAP_* in .env)"
                    );
                    std::process::exit(1);
                }

                if backfill {
                    store::clear_sync_cursors(&app.db_pool)
                        .await
                        .map_err(|e| e.to_string())?;
                }

                let mut any_failed = false;
                for config in &configs {
                    match sync_account(&app.db_pool, config, true).await {
                        Ok(report) => {
                            if report.epoch_changed {
                                eprintln!(
                                    "  [{}] UIDVALIDITY changed; resyncing recent mail instead of resuming",
                                    config.account
                                );
                            }
                            println!("✓ [{}] Synced {} new email(s)", config.account, report.new);
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
                    println!("∅ No emails yet! Sync with: triptych email sync");
                } else {
                    for email in &emails {
                        let status = if email.is_read { " " } else { "*" };
                        let from = email.from_name.as_deref().unwrap_or(&email.from_addr);
                        println!(
                            "  {} ({}) [{}] {:20} {} (ID: {})",
                            status,
                            email.account,
                            email
                                .date_utc
                                .with_timezone(&chrono::Local)
                                .format("%m/%d %H:%M"),
                            from,
                            email.subject,
                            email.id
                        );
                    }
                }
            }
        },

        // Daemon/Stop/Status/Notify are handled in `run` before this fn is called;
        // reaching here means that routing has a bug, not a real user path.
        _ => return Err("unexpected command reached handle_cli_command".into()),
    }

    Ok(())
}

/// `triptych notify ...`. Runs before `App::build()` (see `run`): it needs only the database, and
/// the `LaunchAgent` calls `check` every minute, so it must not pay the NLP parser's Ollama probe.
pub async fn handle_notify_command(command: NotifyCommands) -> Result<(), crate::BoxError> {
    match command {
        NotifyCommands::Check => {
            if !notify::enabled() {
                println!("✗ Alerts are off ({})", notify::NOTIFY_ENV);
                return Ok(());
            }
            let pool = open_pool().await?;
            run_calendar_migration(&pool).await?;
            let report = notify::check_once(&pool, crate::clock::now()).await?;
            match report.shown {
                0 if report.failed == 0 => println!("No alerts due"),
                0 => {}
                n => println!("✓ Sent {n} alert(s)"),
            }
            if report.failed > 0 {
                eprintln!(
                    "▲ {} alert(s) could not be shown (check TRIPTYCH_NOTIFY_CMD); retrying next run",
                    report.failed
                );
            }
        }
        NotifyCommands::Install => match notify::agent::install() {
            Ok(path) => {
                println!("✓ Installed {}", path.display());
                println!("  Alerts now run every minute, TUI open or not.");
            }
            Err(e) => {
                eprintln!("✗ {e}");
                std::process::exit(1);
            }
        },
        NotifyCommands::Uninstall => match notify::agent::uninstall() {
            Ok(true) => println!("✓ Removed the alert agent"),
            Ok(false) => println!("No alert agent installed"),
            Err(e) => {
                eprintln!("✗ {e}");
                std::process::exit(1);
            }
        },
    }
    Ok(())
}
