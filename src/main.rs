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
    widgets::{Block, Borders, List, ListItem},
};
use sqlx::{
    FromRow,
    migrate::MigrateDatabase,
    sqlite::{Sqlite, SqlitePool},
};
use std::io;
use std::time::Duration;

const DB_URL: &str = "sqlite:todo.db";

// A struct to hold our task data from the database
#[derive(Clone, FromRow)]
struct Task {
    id: i64,
    description: String,
    completed: bool,
}

// App holds the state of our application
struct App {
    db_pool: SqlitePool,
    tasks: Vec<Task>,
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
        })
    }

    // Load tasks from the database into app state
    async fn load_tasks(&mut self) -> Result<(), sqlx::Error> {
        self.tasks =
            sqlx::query_as::<_, Task>("SELECT id, description, completed FROM tasks ORDER BY id")
                .fetch_all(&self.db_pool)
                .await?;
        Ok(())
    }

    // Add a new task to the database
    async fn add_task(&mut self, description: &str) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO tasks (description, completed) VALUES (?, ?)")
            .bind(description)
            .bind(false) // New tasks are not completed
            .execute(&self.db_pool)
            .await?;

        self.load_tasks().await?;
        Ok(())
    }
    async fn delete_task(&mut self) -> Result<(), sqlx::Error> {
        //TODO: Finish the functionality for deleting a task
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

        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
        {
            if KeyCode::Char('q') == key.code {
                return Ok(());
            }
            if KeyCode::Char('a') == key.code {
                app.add_task("A new task from the TUI!").await.unwrap();
            }
            if KeyCode::Char('x') == key.code {}
        }
    }
}

fn ui(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([Constraint::Percentage(100)].as_ref())
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

    let tasks_list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("To-Do List (q to quit, a to add)"),
        )
        .highlight_style(
            Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD),
        );

    f.render_widget(tasks_list, chunks[0]);
}
