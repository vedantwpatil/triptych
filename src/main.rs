mod app;
mod ui;

use crate::app::InputMode;
use crate::ui::ui;
use app::App;
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
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create and run app
    let mut app = App::build().await?;
    app.load_tasks().await?;
    run_app(&mut terminal, app).await?;

    // Restore terminal
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
