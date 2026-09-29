//! Canvas assignment sync: a Canvas calendar feed (`.ics`) becomes todo tasks.
//!
//! Each event is upserted keyed on the feed `UID` (`tasks.external_id`). Parsing and upsert are
//! terminal-free so `tests/it/canvas.rs` can reach them; the worker in `sync/canvas.rs` and
//! `triptych canvas sync` both call [`sync`].

use std::str::FromStr;

use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use icalendar::{Calendar, CalendarDateTime, Component, DatePerhapsTime};
use sqlx::SqlitePool;

use crate::app::{classify_task, default_duration_for_category};

/// Env var holding the secret Canvas calendar feed URL. Never log it.
pub const FEED_URL_ENV: &str = "CANVAS_ICS_URL";

/// Priority given to a new assignment (MED); the urgency display raises it as the due date nears.
const NEW_TASK_PRIORITY: i32 = 1;

/// Canvas titles end in a long section tag, `Quiz 3 [CS-472-001/002-XLIST-202615]`. Returns the
/// title as `CS-472: Quiz 3`, or the title unchanged when it has no recognisable course tag.
#[must_use]
pub fn tidy_title(raw: &str) -> String {
    let raw = raw.trim();
    let Some((name, tag)) = raw.strip_suffix(']').and_then(|rest| rest.rsplit_once('[')) else {
        return raw.to_string();
    };
    let mut parts = tag.split('-');
    let dept = parts.next().map(str::trim).unwrap_or_default();
    let number: String = parts
        .next()
        .unwrap_or_default()
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect();
    let name = name.trim();
    let is_dept = !dept.is_empty() && dept.chars().all(|c| c.is_ascii_alphabetic());
    if !is_dept || !number.starts_with(|c: char| c.is_ascii_digit()) || name.is_empty() {
        return raw.to_string();
    }
    format!("{dept}-{number}: {name}")
}

/// Splits a tidied title into its course code and the rest: `CS-472: Quiz 3` gives
/// `("CS-472", "Quiz 3")`. `None` for any other text.
#[must_use]
pub fn split_course(title: &str) -> Option<(&str, &str)> {
    let (code, rest) = title.split_once(": ")?;
    let (dept, number) = code.split_once('-')?;
    let valid = !dept.is_empty()
        && dept.chars().all(|c| c.is_ascii_alphabetic())
        && number.starts_with(|c: char| c.is_ascii_digit())
        && number.chars().all(|c| c.is_ascii_alphanumeric());
    valid.then_some((code, rest))
}

/// One feed event, reduced to what a task needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assignment {
    pub uid: String,
    pub title: String,
    pub due: DateTime<Utc>,
}

/// What one sync changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub added: usize,
    pub updated: usize,
}

/// The feed URL from the environment, `None` when unset or blank.
#[must_use]
pub fn feed_url_from_env() -> Option<String> {
    std::env::var(FEED_URL_ENV)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// An all-day event has a date but no time: due at the end of that local day.
fn end_of_local_day(date: NaiveDate) -> Option<DateTime<Utc>> {
    let naive = date.and_hms_opt(23, 59, 0)?;
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|dt| dt.with_timezone(&Utc))
}

fn due_from(dpt: &DatePerhapsTime) -> Option<DateTime<Utc>> {
    match dpt {
        DatePerhapsTime::Date(date) => end_of_local_day(*date),
        DatePerhapsTime::DateTime(CalendarDateTime::Floating(naive)) => Some(naive.and_utc()),
        DatePerhapsTime::DateTime(cdt) => cdt.try_into_utc(),
    }
}

/// Every usable `VEVENT` in the feed. Events without a `UID`, a start time or a title are skipped;
/// text that is not a calendar yields an empty list.
#[must_use]
pub fn parse_feed(ics: &str) -> Vec<Assignment> {
    let Ok(calendar) = Calendar::from_str(ics) else {
        return Vec::new();
    };
    calendar
        .events()
        .filter_map(|event| {
            let uid = event.get_uid()?.trim();
            let title = event.get_summary()?.trim();
            if uid.is_empty() || title.is_empty() {
                return None;
            }
            Some(Assignment {
                uid: uid.to_string(),
                title: title.to_string(),
                due: due_from(&event.get_start()?)?,
            })
        })
        .collect()
}

/// Inserts new assignments and moves the deadline of known, unfinished ones.
///
/// A known task's completion, priority and tags are never touched, so local edits survive a
/// re-poll. Its title is rewritten only while it still equals the raw feed title (imported before
/// titles were tidied); a reworded title stays. Assignments that vanish from the feed are left alone.
///
/// # Errors
///
/// Returns an error if a database query fails.
pub async fn upsert(pool: &SqlitePool, items: &[Assignment]) -> Result<SyncReport, sqlx::Error> {
    let mut report = SyncReport::default();
    for item in items {
        let tidy = tidy_title(&item.title);
        let known: Option<(i64, bool, Option<DateTime<Utc>>, String)> = sqlx::query_as(
            "SELECT id, completed, deadline, description FROM tasks WHERE external_id = ?",
        )
        .bind(&item.uid)
        .fetch_optional(pool)
        .await?;
        match known {
            None => {
                let category = classify_task(&item.title);
                sqlx::query(
                    "INSERT INTO tasks (description, completed, item_order, priority, deadline, \
                     duration_minutes, task_category, external_id) \
                     VALUES (?, false, (SELECT COALESCE(MAX(item_order), -1) + 1 FROM tasks), ?, ?, ?, ?, ?)",
                )
                .bind(&tidy)
                .bind(NEW_TASK_PRIORITY)
                .bind(item.due)
                .bind(default_duration_for_category(category))
                .bind(category)
                .bind(&item.uid)
                .execute(pool)
                .await?;
                report.added += 1;
            }
            Some((id, false, deadline, description))
                if deadline != Some(item.due)
                    || (description == item.title && tidy != item.title) =>
            {
                let description = if description == item.title {
                    tidy
                } else {
                    description
                };
                sqlx::query(
                    "UPDATE tasks SET deadline = ?, description = ?, notified_tier = \
                     CASE WHEN deadline IS ? THEN notified_tier ELSE 0 END WHERE id = ?",
                )
                .bind(item.due)
                .bind(description)
                .bind(item.due)
                .bind(id)
                .execute(pool)
                .await?;
                report.updated += 1;
            }
            Some(_) => {}
        }
    }
    Ok(report)
}

/// Downloads the feed text. Errors carry no URL: it is a secret token.
///
/// # Errors
///
/// Returns a message if the request fails or the server answers with an error status.
pub async fn fetch_feed(url: &str) -> Result<String, String> {
    let response = reqwest::Client::builder()
        .user_agent(concat!("triptych/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.without_url().to_string())?
        .get(url)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|e| e.without_url().to_string())?;
    response
        .text()
        .await
        .map_err(|e| e.without_url().to_string())
}

/// Fetches the feed at `url` and upserts its events.
///
/// # Errors
///
/// Returns a message if the download or a database write fails.
pub async fn sync(pool: &SqlitePool, url: &str) -> Result<SyncReport, String> {
    let ics = fetch_feed(url).await?;
    upsert(pool, &parse_feed(&ics))
        .await
        .map_err(|e| e.to_string())
}
