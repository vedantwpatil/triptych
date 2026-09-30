//! `links`: reading a task's URLs and the `O` popup state. Nothing here opens a browser: the
//! `o`/`Enter` paths that spawn the opener are covered by the TUI suite, which points
//! `TRIPTYCH_OPEN_CMD` at a recorder.

use sqlx::SqlitePool;
use triptych::app::{App, is_web_url, link_label, parse_links};

async fn app_with_links(links: Option<&str>) -> App {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("in-memory pool");
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    triptych::migrations::run_calendar_migration(&pool)
        .await
        .unwrap();
    insert_task(&pool, links).await;
    let mut app = App::new(pool).await;
    app.load_tasks().await.unwrap();
    app
}

async fn insert_task(pool: &SqlitePool, links: Option<&str>) {
    sqlx::query(
        "INSERT INTO tasks (description, completed, item_order, links) VALUES ('t', 0, 0, ?)",
    )
    .bind(links)
    .execute(pool)
    .await
    .unwrap();
}

#[test]
fn parse_links_reads_a_json_list_and_shrugs_off_anything_else() {
    assert_eq!(
        parse_links(Some(r#"["https://a.example","https://b.example"]"#)),
        ["https://a.example", "https://b.example"]
    );
    assert!(parse_links(None).is_empty());
    assert!(parse_links(Some("not json")).is_empty());
}

#[test]
fn only_http_links_count_as_web_urls() {
    assert!(is_web_url("https://a.example"));
    assert!(is_web_url("HTTP://a.example"));
    assert!(!is_web_url("file:///etc/passwd"));
    assert!(!is_web_url("mailto:a@b.c"));
    assert!(!is_web_url("javascript:alert(1)"));
}

#[test]
fn link_label_drops_the_scheme_and_www_and_truncates() {
    assert_eq!(
        link_label("https://www.a.example/x?y=1", 40),
        "a.example/x?y=1"
    );
    assert_eq!(link_label("http://a.example/x", 40), "a.example/x");
    assert_eq!(link_label("https://a.example/abcdefghij", 10), "a.example…");
}

#[tokio::test]
async fn links_popup_opens_only_for_a_task_with_several_links() {
    let mut app = app_with_links(Some(r#"["https://a.example","https://b.example"]"#)).await;
    app.open_links_popup();
    assert!(app.links_open);
    assert_eq!(app.links, ["https://a.example", "https://b.example"]);
    assert_eq!(app.selected_link, 0);

    // A number with no link behind it leaves the popup as it is.
    app.open_link_number(0);
    app.open_link_number(3);
    assert!(app.links_open);

    app.close_links_popup();
    assert!(!app.links_open);
    assert!(app.links.is_empty());
}

#[tokio::test]
async fn a_task_without_links_reports_it_and_opens_nothing() {
    let mut app = app_with_links(None).await;
    app.open_links_popup();
    assert!(!app.links_open);
    assert_eq!(
        app.status_message.as_ref().map(|(m, _)| m.as_str()),
        Some("No links on this task")
    );

    app.status_message = None;
    app.open_first_link();
    assert_eq!(
        app.status_message.as_ref().map(|(m, _)| m.as_str()),
        Some("No links on this task")
    );
}
