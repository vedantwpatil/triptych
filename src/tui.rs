//! The interactive terminal UI: setup, teardown and the event loop.

mod keys;
pub mod ui;

use crate::app::{App, ViewMode};
use crate::sync::{SyncConfig, SyncDaemon};
use crossterm::{
    event::{Event, EventStream},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use keys::KeyOutcome;
use ratatui::{
    Terminal,
    backend::{Backend, CrosstermBackend},
};
use std::io;
use tokio::signal;
use ui::ui;

/// Runs the UI until the user quits, with the background sync workers alive meanwhile.
pub async fn run(mut app: App) -> Result<(), crate::BoxError> {
    // Start the sync workers BEFORE entering the alternate screen so warmup messages print cleanly
    let sync_config = SyncConfig::from_env();
    app.nlp_parser_ref().set_wait_for_load(false);
    let daemon = SyncDaemon::start(app.db_pool.clone(), app.nlp_parser_ref(), &sync_config);

    app.load_tasks().await?;

    install_panic_hook();
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let tui_result = run_app(&mut terminal, app).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    daemon.shutdown().await?;
    tui_result?;
    Ok(())
}

/// Restore the terminal before a panic's default report prints, since unwinding past
/// `run_app` would otherwise skip the raw-mode/alternate-screen cleanup in `main`.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
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

    // Mail lands in the DB from background syncs (view entry, 60s poller); reload it while the
    // Email view is open so new messages show up without leaving and re-entering. Skipped while
    // the body popup is open: `get_recent` rows carry no `body_text`, so a reload would blank it.
    let mut mail_tick = tokio::time::interval(std::time::Duration::from_secs(2));

    loop {
        terminal.draw(|f| ui(f, &mut app))?;

        // Wait for either keyboard event or shutdown signal
        tokio::select! {
            // Keyboard event (async, zero lag!)
            maybe_event = reader.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) => {
                        if matches!(keys::handle_key_event(&mut app, key).await, KeyOutcome::Quit) {
                            return Ok(());
                        }
                    }
                    Some(Ok(_)) => {} // Other events (mouse, resize, etc.)
                    Some(Err(e)) => {
                        app.status_message = Some((format!("Input error: {e}"), std::time::Instant::now()));
                    }
                    None => break, // Stream ended
                }
            }

            Some(parsed) = app.deadline_rx.recv() => {
                if let Err(e) = app.apply_deadline_parse(parsed).await {
                    app.status_message = Some((format!("Error: {e}"), std::time::Instant::now()));
                }
            }

            Some(parsed) = app.task_rx.recv() => {
                if let Err(e) = app.apply_task_parse(parsed).await {
                    app.status_message = Some((format!("Error: {e}"), std::time::Instant::now()));
                }
            }

            Some(done) = app.mail_rx.recv() => {
                app.apply_mail_sync(done).await;
            }

            Some(done) = app.summary_rx.recv() => {
                app.apply_summary(done).await;
            }

            _ = mail_tick.tick(), if app.view_mode == ViewMode::Email && !app.email_detail_open => {
                let _ = app.refresh_emails().await;
            }

            // Shutdown signal
            _ = shutdown_rx.recv() => {
                return Ok(());
            }
        }
    }

    Ok(())
}
