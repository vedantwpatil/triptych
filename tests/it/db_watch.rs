use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use triptych::app::DbWatch;

#[tokio::test]
async fn watch_flags_a_commit_from_another_connection_once() {
    let path = std::env::temp_dir().join(format!("triptych-watch-{}.db", std::process::id()));
    let pool = SqlitePoolOptions::new()
        .max_connections(3)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&path)
                .create_if_missing(true),
        )
        .await
        .expect("file pool");
    sqlx::query("CREATE TABLE t (x INTEGER)")
        .execute(&pool)
        .await
        .expect("create");

    let mut watch = DbWatch::new(&pool).await.expect("watch");
    assert!(!watch.changed().await, "nothing written yet");

    sqlx::query("INSERT INTO t VALUES (1)")
        .execute(&pool)
        .await
        .expect("insert");
    assert!(watch.changed().await, "a pool write must be seen");
    assert!(!watch.changed().await, "and reported only once");

    pool.close().await;
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
}
