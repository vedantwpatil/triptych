//! Non-interactive CLI subcommands (`add`, `list`, `schedule ...`, `email ...`).

use super::daemon::{self, DaemonRequest, DaemonResponse};
use crate::app::App;
use crate::cli::{Commands, EmailCommands, ScheduleCommands};
use crate::email::{EmailConfig, ImapMailSource, MailSource, message, store};
use crate::urgency;

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

        Commands::List => {
            // List command logic (unchanged)
            match app.get_enhanced_task_list().await {
                Ok(enhanced_tasks) => {
                    if enhanced_tasks.is_empty() {
                        println!("∅ No tasks yet! Add one with: triptych add \"Your task\"");
                    } else {
                        println!("▸ Current Tasks:");
                        for enhanced in &enhanced_tasks {
                            let task = &enhanced.task;
                            let status = if task.completed { "✓" } else { "○" };
                            let mut indicators = Vec::new();

                            if let Some((_, badge)) =
                                urgency::priority_badge(task, chrono::Utc::now())
                            {
                                indicators.push(badge);
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

                            if let (Some(deadline), false) = (task.deadline, task.completed) {
                                indicators
                                    .push(urgency::deadline_badge(deadline, chrono::Local::now()));
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

        Commands::Email(email_cmd) => match email_cmd {
            EmailCommands::Sync => {
                let configs = EmailConfig::all_from_env();
                if configs.is_empty() {
                    eprintln!(
                        "✗ Email not configured (set TRIPTYCH_EMAIL_ENABLED=true and IMAP_* in .env)"
                    );
                    std::process::exit(1);
                }

                let mut any_failed = false;
                for config in &configs {
                    let cursor =
                        store::get_sync_cursor(&app.db_pool, &config.account, &config.imap_folder)
                            .await
                            .map_err(|e| e.to_string())?;

                    let source = ImapMailSource::new(config.clone());
                    match source.fetch_new(cursor).await {
                        Ok((uid_validity, raw_messages)) => {
                            let epoch_changed = match (cursor, uid_validity) {
                                // `c.uid_validity` was itself stored from a `u32`
                                // (see `email/client.rs`), so this round-trip always fits.
                                (Some(c), Some(current)) => {
                                    u32::try_from(c.uid_validity).unwrap_or(0) != current
                                }
                                _ => false,
                            };
                            if epoch_changed {
                                eprintln!(
                                    "  [{}] UIDVALIDITY changed; resyncing recent mail instead of resuming",
                                    config.account
                                );
                            }

                            let fetched_max_uid = raw_messages.iter().map(|(uid, _, _)| *uid).max();

                            let new_emails: Vec<_> = raw_messages
                                .into_iter()
                                .filter_map(|(uid, raw, header_only)| {
                                    message::parse_raw(
                                        &config.account,
                                        uid,
                                        &config.imap_folder,
                                        &raw,
                                        header_only,
                                    )
                                    .ok()
                                })
                                .collect();

                            let count = new_emails.len();
                            store::insert_new(&app.db_pool, &new_emails)
                                .await
                                .map_err(|e| e.to_string())?;
                            // See sync/mail.rs's sync_mail: skip persisting a synthetic
                            // `last_uid = 0` when the epoch changed but nothing came back,
                            // so the next sync retries the properly-capped catch-up.
                            if let Some(validity) = uid_validity
                                && !(epoch_changed && fetched_max_uid.is_none())
                            {
                                let prior_uid = if epoch_changed {
                                    0
                                } else {
                                    cursor.map_or(0, |c| c.last_uid)
                                };
                                let last_uid = fetched_max_uid
                                    .map_or(prior_uid, |uid| i64::from(uid).max(prior_uid));
                                store::set_sync_cursor(
                                    &app.db_pool,
                                    &config.account,
                                    &config.imap_folder,
                                    store::SyncCursor {
                                        uid_validity: i64::from(validity),
                                        last_uid,
                                    },
                                )
                                .await
                                .map_err(|e| e.to_string())?;
                            }
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

        // Daemon/Stop/Status are filtered out in `main` before this fn is called;
        // reaching here means that filtering has a bug, not a real user path.
        _ => return Err("unexpected daemon command reached handle_cli_command".into()),
    }

    Ok(())
}
