//! `canvas`: feed parsing and the idempotent upsert into `tasks`.

use chrono::{DateTime, Local, TimeZone, Timelike, Utc};
use sqlx::SqlitePool;
use triptych::canvas::{Assignment, parse_feed, split_course, tidy_title, upsert};

const FEED: &str = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//canvas//EN\r\n\
BEGIN:VEVENT\r\nUID:event-assignment-1\r\nSUMMARY:Essay 2 [ENG 101]\r\nDTSTART:20261015T235900Z\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nUID:event-assignment-2\r\nSUMMARY:Reading quiz\r\nDTSTART;VALUE=DATE:20261020\r\nEND:VEVENT\r\n\
BEGIN:VEVENT\r\nSUMMARY:No uid, skipped\r\nDTSTART:20261021T100000Z\r\nEND:VEVENT\r\n\
END:VCALENDAR\r\n";

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

fn assignment(uid: &str, title: &str, due: DateTime<Utc>) -> Assignment {
    Assignment {
        uid: uid.to_string(),
        title: title.to_string(),
        due,
    }
}

#[test]
fn parse_feed_reads_timed_and_all_day_events_and_skips_incomplete_ones() {
    let items = parse_feed(FEED);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].uid, "event-assignment-1");
    assert_eq!(items[0].title, "Essay 2 [ENG 101]");
    assert_eq!(
        items[0].due,
        Utc.with_ymd_and_hms(2026, 10, 15, 23, 59, 0).unwrap()
    );
    let all_day = items[1].due.with_timezone(&Local);
    assert_eq!((all_day.hour(), all_day.minute()), (23, 59));
}

#[test]
fn parse_feed_of_garbage_is_empty() {
    assert!(parse_feed("<html>not a calendar</html>").is_empty());
}

#[tokio::test]
async fn upsert_is_idempotent_and_updates_only_the_deadline_of_open_tasks() {
    let pool = test_pool().await;
    let due = Utc.with_ymd_and_hms(2026, 10, 15, 23, 59, 0).unwrap();
    let item = assignment("u1", "Essay 2", due);

    let first = upsert(&pool, std::slice::from_ref(&item)).await.unwrap();
    assert_eq!((first.added, first.updated), (1, 0));
    let again = upsert(&pool, std::slice::from_ref(&item)).await.unwrap();
    assert_eq!((again.added, again.updated), (0, 0));

    // A local reword survives; a moved due date is picked up.
    sqlx::query("UPDATE tasks SET description = 'my essay'")
        .execute(&pool)
        .await
        .unwrap();
    let moved = assignment("u1", "Essay 2 (renamed)", due + chrono::Duration::days(2));
    let r = upsert(&pool, std::slice::from_ref(&moved)).await.unwrap();
    assert_eq!((r.added, r.updated), (0, 1));
    let (desc, deadline): (String, DateTime<Utc>) =
        sqlx::query_as("SELECT description, deadline FROM tasks")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(desc, "my essay");
    assert_eq!(deadline, moved.due);

    // A finished task keeps its old deadline.
    sqlx::query("UPDATE tasks SET completed = true")
        .execute(&pool)
        .await
        .unwrap();
    let later = assignment("u1", "Essay 2", due + chrono::Duration::days(9));
    let r = upsert(&pool, &[later]).await.unwrap();
    assert_eq!((r.added, r.updated), (0, 0));
}

#[test]
fn tidy_title_moves_the_course_code_to_the_front() {
    assert_eq!(
        tidy_title("Quiz 3 [CS-472-001/002-XLIST-202615]"),
        "CS-472: Quiz 3"
    );
    assert_eq!(
        tidy_title("Short Homework 1 [LING-101-001 - FA 26-27]"),
        "LING-101: Short Homework 1"
    );
    assert_eq!(tidy_title("Essay [ENG 101]"), "Essay [ENG 101]");
    assert_eq!(tidy_title("Plain title"), "Plain title");
}

#[tokio::test]
async fn upsert_tidies_new_titles_and_old_raw_ones_but_not_rewords() {
    let pool = test_pool().await;
    let due = Utc.with_ymd_and_hms(2026, 10, 15, 23, 59, 0).unwrap();
    let raw = "Quiz 3 [CS-472-001-FA]";
    upsert(
        &pool,
        &[
            assignment("u1", raw, due),
            assignment("u2", "Lab [CS-472-001-FA]", due),
        ],
    )
    .await
    .unwrap();
    // u1 predates tidy titles; u2 was reworded by the user.
    sqlx::query("UPDATE tasks SET description = ? WHERE external_id = 'u1'")
        .bind(raw)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE tasks SET description = 'mine' WHERE external_id = 'u2'")
        .execute(&pool)
        .await
        .unwrap();
    let r = upsert(
        &pool,
        &[
            assignment("u1", raw, due),
            assignment("u2", "Lab [CS-472-001-FA]", due),
        ],
    )
    .await
    .unwrap();
    assert_eq!(r.updated, 1);
    let names: Vec<(String,)> =
        sqlx::query_as("SELECT description FROM tasks ORDER BY external_id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        names,
        [("CS-472: Quiz 3".to_string(),), ("mine".to_string(),)]
    );
}

#[test]
fn split_course_separates_the_code_from_tidy_titles_only() {
    assert_eq!(split_course("CS-472: Quiz 3"), Some(("CS-472", "Quiz 3")));
    assert_eq!(
        split_course("LING-101: Essay: draft"),
        Some(("LING-101", "Essay: draft"))
    );
    assert_eq!(split_course("Note: call mom"), None);
    assert_eq!(split_course("no colon here"), None);
}
