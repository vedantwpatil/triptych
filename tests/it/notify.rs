//! `notify`: deadline alert tiers, batching and idempotence.

use std::path::Path;

use chrono::{DateTime, Duration, Utc};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use triptych::notify::agent::{db_file, plist};
use triptych::notify::due_alerts;

async fn test_pool() -> SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("in-memory pool");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("base schema");
    triptych::migrations::run_calendar_migration(&pool)
        .await
        .expect("calendar schema");
    pool
}

async fn add(pool: &SqlitePool, desc: &str, deadline: Option<DateTime<Utc>>, done: bool) {
    sqlx::query(
        "INSERT INTO tasks (description, completed, item_order, priority, deadline) VALUES (?, ?, 0, 1, ?)",
    )
    .bind(desc)
    .bind(done)
    .bind(deadline)
    .execute(pool)
    .await
    .expect("insert");
}

#[tokio::test]
async fn alerts_once_per_tier_and_skips_far_done_and_overdue_tasks() {
    let pool = test_pool().await;
    let now = Utc::now();
    add(&pool, "soon", Some(now + Duration::hours(10)), false).await;
    add(&pool, "far", Some(now + Duration::days(3)), false).await;
    add(&pool, "done", Some(now + Duration::hours(2)), true).await;
    add(&pool, "late", Some(now - Duration::hours(2)), false).await;
    add(&pool, "none", None, false).await;

    let first = due_alerts(&pool, now).await.unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].title, "Due within 24 hours");
    assert!(first[0].body.contains("soon (in 10h 0m)"));
    assert!(
        due_alerts(&pool, now).await.unwrap().is_empty(),
        "no repeat"
    );

    // Closer in, the same task alerts again at the 1-hour tier, once.
    let later = now + Duration::minutes(570);
    let second = due_alerts(&pool, later).await.unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].title, "Due within 1 hour");
    assert!(due_alerts(&pool, later).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_moved_deadline_alerts_again_and_long_lists_are_batched() {
    let pool = test_pool().await;
    let now = Utc::now();
    for i in 0..5 {
        add(
            &pool,
            &format!("t{i}"),
            Some(now + Duration::minutes(30 + i)),
            false,
        )
        .await;
    }
    let alerts = due_alerts(&pool, now).await.unwrap();
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].body.lines().count(), 4);
    assert!(alerts[0].body.ends_with("+2 more"));

    sqlx::query("UPDATE tasks SET deadline = ?, notified_tier = 0 WHERE description = 't0'")
        .bind(now + Duration::minutes(20))
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(due_alerts(&pool, now).await.unwrap().len(), 1);
}

#[tokio::test]
async fn the_time_left_switches_from_minutes_to_hours_at_sixty() {
    let pool = test_pool().await;
    let now = Utc::now();
    add(
        &pool,
        "just under",
        Some(now + Duration::minutes(59)),
        false,
    )
    .await;
    add(&pool, "exactly", Some(now + Duration::minutes(60)), false).await;
    add(&pool, "over", Some(now + Duration::minutes(61)), false).await;

    // 59 and 60 minutes are the 1-hour tier, 61 the 24-hour tier: two alerts.
    let alerts = due_alerts(&pool, now).await.unwrap();
    let body = alerts
        .iter()
        .map(|a| a.body.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    for line in [
        "just under (in 59m)",
        "exactly (in 1h 0m)",
        "over (in 1h 1m)",
    ] {
        assert!(body.contains(line), "{line:?} not in {body:?}");
    }
}

#[tokio::test]
async fn a_list_exactly_at_the_cap_has_no_more_line() {
    let pool = test_pool().await;
    let now = Utc::now();
    for i in 0..3 {
        add(
            &pool,
            &format!("t{i}"),
            Some(now + Duration::minutes(30 + i)),
            false,
        )
        .await;
    }

    let body = due_alerts(&pool, now).await.unwrap().remove(0).body;

    assert_eq!(body.lines().count(), 3, "{body:?}");
    assert!(!body.contains("more"), "{body:?}");
}

/// The TUI worker and the `LaunchAgent` can check at the same instant; each task must still alert once.
#[tokio::test]
async fn concurrent_checks_alert_each_task_once() {
    let dir = std::env::temp_dir().join(format!("triptych_notify_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let opts = SqliteConnectOptions::new()
        .filename(dir.join("race.db"))
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(opts)
        .await
        .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    triptych::migrations::run_calendar_migration(&pool)
        .await
        .unwrap();
    let now = Utc::now();
    for i in 0..3 {
        add(
            &pool,
            &format!("t{i}"),
            Some(now + Duration::minutes(30 + i)),
            false,
        )
        .await;
    }

    let (a, b) = tokio::join!(due_alerts(&pool, now), due_alerts(&pool, now));
    let listed: usize = a
        .unwrap()
        .iter()
        .chain(&b.unwrap())
        .map(|alert| alert.body.lines().filter(|l| l.contains("(in")).count())
        .sum();
    assert_eq!(
        listed, 3,
        "each task alerts exactly once across both checks"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn db_file_resolves_sqlite_urls_against_the_cwd() {
    let cwd = Path::new("/work");
    assert_eq!(db_file("sqlite:todo.db", cwd), Some("/work/todo.db".into()));
    assert_eq!(
        db_file("sqlite:/abs/t.db?mode=rwc", cwd),
        Some("/abs/t.db".into())
    );
    assert_eq!(db_file("sqlite:///abs/t.db", cwd), Some("/abs/t.db".into()));
    assert_eq!(db_file("sqlite::memory:", cwd), None);
    assert_eq!(db_file("postgres://host/db", cwd), None);
}

/// macOS only: the generated plist must parse, or launchd would refuse it.
#[test]
fn plist_passes_plutil_lint() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    if !cfg!(target_os = "macos") {
        return;
    }
    let text = plist(
        Path::new("/bin/triptych"),
        Path::new("/a & b"),
        Path::new("/a & b/todo.db"),
        Path::new("/tmp/n.log"),
    );
    let mut child = Command::new("plutil")
        .args(["-lint", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn plist_runs_notify_check_with_pinned_paths_and_escapes_xml() {
    let p = plist(
        Path::new("/bin/triptych"),
        Path::new("/a & b"),
        Path::new("/a & b/todo.db"),
        Path::new("/tmp/n.log"),
    );
    assert!(
        p.contains("<string>/bin/triptych</string><string>notify</string><string>check</string>")
    );
    assert!(p.contains("<key>WorkingDirectory</key><string>/a &amp; b</string>"));
    assert!(p.contains("<key>DATABASE_URL</key><string>sqlite:/a &amp; b/todo.db</string>"));
    assert!(p.contains("<key>StartInterval</key><integer>60</integer>"));
    assert!(p.contains("<key>RunAtLoad</key><true/>"));
}
