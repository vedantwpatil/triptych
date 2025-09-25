use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};
use sqlx::{
    FromRow,
    migrate::MigrateDatabase,
    sqlite::{Sqlite, SqlitePool},
};
use std::io;

const DB_URL: &str = "sqlite:todo.db";

// Enum to hold represent application state
enum InputMode {
    Normal,
    Editing,
}

// A struct to hold our task data from the database
#[derive(Clone, FromRow)]
struct Task {
    id: i64,
    description: String,
    completed: bool,
    item_order: Option<i64>,
}

// App holds the state of our application
struct App {
    db_pool: SqlitePool,
    tasks: Vec<Task>,
    selected: usize,
    input_mode: InputMode,
    input_buffer: String,
}

impl App {
    // Create a new App instance with a database connection
    async fn new() -> Result<Self, sqlx::Error> {
        // Create DB if it doesn't exist
        if !Sqlite::database_exists(DB_URL).await.unwrap_or(false) {
            Sqlite::create_database(DB_URL).await?;
        }

        // Create a connection pool and run migrations
        let db_pool = SqlitePool::connect(DB_URL).await?;
        sqlx::migrate!("./migrations").run(&db_pool).await?;

        Ok(Self {
            db_pool,
            tasks: vec![],
            selected: 0,
            input_mode: InputMode::Normal,
            input_buffer: String::new(),
        })
    }

    // Load tasks from the database into app state
    async fn load_tasks(&mut self) -> Result<(), sqlx::Error> {
        self.tasks = sqlx::query_as::<_, Task>(
            "SELECT id, description, completed, item_order FROM tasks ORDER BY item_order ASC",
        )
        .fetch_all(&self.db_pool)
        .await?;

        if self.selected >= self.tasks.len() {
            self.selected = self.tasks.len().saturating_sub(1);
        }
        Ok(())
    }

    // Add a new task to the database according to position (selecting a task using the cursor)
    async fn add_task(&mut self, description: &str) -> Result<(), sqlx::Error> {
        let new_order: i64;

        if self.tasks.is_empty() {
            new_order = 0;
        } else if self.selected == 0 {
            sqlx::query("UPDATE tasks SET item_order = item_order + 1 WHERE item_order >= 0")
                .execute(&self.db_pool)
                .await?;
            new_order = 0;
        } else {
            let current_order = self.tasks[self.selected]
                .item_order
                .unwrap_or(self.tasks.len() as i64);

            // Shift all tasks that come after the current one.
            sqlx::query("UPDATE tasks SET item_order = item_order + 1 WHERE item_order > ?")
                .bind(current_order)
                .execute(&self.db_pool)
                .await?;

            new_order = current_order + 1;
        }

        // Insert the new task with its calculated order.
        sqlx::query("INSERT INTO tasks (description, completed, item_order) VALUES (?, ?, ?)")
            .bind(description)
            .bind(false)
            .bind(new_order)
            .execute(&self.db_pool)
            .await?;

        // Reload tasks from the database.
        self.load_tasks().await?;

        // Find the new position of the inserted task and update the cursor.
        self.selected = self
            .tasks
            .iter()
            .position(|t| t.item_order == Some(new_order))
            .unwrap_or(0);

        Ok(())
    }

    // Delete a selected task from the database
    async fn delete_task(&mut self) -> Result<(), sqlx::Error> {
        if self.tasks.is_empty() {
            // Do nothing
            return Ok(());
        }

        let task_id = self.tasks[self.selected].id;

        sqlx::query("DELETE FROM tasks WHERE id = ?")
            .bind(task_id)
            .execute(&self.db_pool)
            .await?;
        self.load_tasks().await?;
        Ok(())
    }
    // Toggle the completion status of the currently selected task
    async fn toggle_completed(&mut self) -> Result<(), sqlx::Error> {
        if self.tasks.is_empty() {
            return Ok(());
        }

        // Get the selected task and its new status
        let task = &self.tasks[self.selected];
        let new_status = !task.completed; // Flip the boolean status

        // Update the task in the database
        sqlx::query("UPDATE tasks SET completed = ? WHERE id = ?")
            .bind(new_status)
            .bind(task.id)
            .execute(&self.db_pool)
            .await?;

        // Reload tasks to reflect the change in the UI
        self.load_tasks().await?;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app and run it
    let mut app = App::new().await?;
    app.load_tasks().await?;
    let res = run_app(&mut terminal, app).await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        println!("{err:?}");
    }

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

fn ui(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([Constraint::Min(3), Constraint::Length(3)].as_ref())
        .split(f.area());

    let items: Vec<ListItem> = app
        .tasks
        .iter()
        .map(|task| {
            let status = if task.completed { "[x]" } else { "[ ]" };
            let content = format!("{} {}", status, task.description);
            ListItem::new(content)
        })
        .collect();

    let mut state = ListState::default();
    state.select(Some(app.selected));

    let tasks_list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("To-Do (q: quit, a: add, x: delete, k/j: move, ENTER: check's task on/off)"),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> "); // Symbol to show next to the selected item

    f.render_stateful_widget(tasks_list, chunks[0], &mut state);

    if let InputMode::Editing = app.input_mode {
        let input_box = Paragraph::new(app.input_buffer.as_str())
            .style(Style::default().fg(Color::Yellow))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("New Task (Enter to save, Esc to cancel)"),
            );
        f.render_widget(input_box, chunks[1]);

        f.set_cursor_position(
            // The new method takes a Position struct
            ratatui::layout::Position {
                x: chunks[1].x + app.input_buffer.chars().count() as u16 + 1,
                y: chunks[1].y + 1,
            },
        );
    }
}
