//! Canvas assignment sync: a Canvas calendar feed (`.ics`) becomes todo tasks.
//!
//! Each event is upserted keyed on the feed `UID` (`tasks.external_id`). Parsing and upsert are
//! terminal-free so `tests/it/canvas.rs` can reach them; the worker in `sync/canvas.rs` and
//! `triptych canvas sync` both call [`sync`].

use std::str::FromStr;

use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};
use icalendar::{Calendar, CalendarDateTime, Component, DatePerhapsTime};
use sqlx::SqlitePool;

use crate::app::{classify_task, default_duration_for_category, is_web_url};

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
    /// The assignment page first, then the links in its description. May be empty.
    pub links: Vec<String>,
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

/// The digits straight after `marker`, if any.
fn id_after<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    let rest = text.split_once(marker)?.1;
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

/// The assignment page for a feed event `URL`. Canvas points events at its calendar view
/// (`https://host/calendar?include_contexts=course_14672&month=09#assignment_94491`); the course
/// and assignment ids in it give the page itself (`https://host/courses/14672/assignments/94491`).
/// Any other `http(s)` URL is kept as it is.
fn page_url(feed_url: &str) -> Option<String> {
    let url = feed_url.trim();
    if !is_web_url(url) {
        return None;
    }
    let course = id_after(url, "include_contexts=course_");
    let assignment = id_after(url, "#assignment_");
    let (Some(course), Some(assignment)) = (course, assignment) else {
        return Some(url.to_string());
    };
    let host_start = url.find("://")? + 3;
    let host_end = url[host_start..]
        .find('/')
        .map_or(url.len(), |i| host_start + i);
    Some(format!(
        "{}/courses/{course}/assignments/{assignment}",
        &url[..host_end]
    ))
}

/// Every `http(s)` link in the event's HTML description (`X-ALT-DESC`), in order, without repeats.
fn description_links(html: &str) -> Vec<String> {
    let mut links: Vec<String> = Vec::new();
    let mut rest = html;
    while let Some((_, after)) = rest.split_once("href=") {
        rest = after;
        let Some(quote) = after.chars().next().filter(|c| matches!(c, '"' | '\'')) else {
            continue;
        };
        let body = &after[1..];
        let Some(end) = body.find(quote) else {
            break;
        };
        let url = body[..end].trim().replace("&amp;", "&");
        if is_web_url(&url) && !links.contains(&url) {
            links.push(url);
        }
        rest = &body[end..];
    }
    links
}

/// The stored form of a task's links; `None` when there are none.
fn links_json(links: &[String]) -> Option<String> {
    if links.is_empty() {
        return None;
    }
    serde_json::to_string(links).ok()
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
            let mut links: Vec<String> = event.get_url().and_then(page_url).into_iter().collect();
            for link in event
                .property_value("X-ALT-DESC")
                .map(description_links)
                .unwrap_or_default()
            {
                if !links.contains(&link) {
                    links.push(link);
                }
            }
            Some(Assignment {
                uid: uid.to_string(),
                title: title.to_string(),
                due: due_from(&event.get_start()?)?,
                links,
            })
        })
        .collect()
}

/// A task already in the database: id, completed, deadline, description, links.
type KnownTask = (i64, bool, Option<DateTime<Utc>>, String, Option<String>);

/// Inserts new assignments and moves the deadline of known, unfinished ones.
///
/// A known task's completion, priority and tags are never touched, so local edits survive a
/// re-poll. Its title is rewritten only while it still equals the raw feed title (imported before
/// titles were tidied); a reworded title stays. Its links follow the feed, finished or not.
/// Assignments that vanish from the feed are left alone.
///
/// # Errors
///
/// Returns an error if a database query fails.
pub async fn upsert(pool: &SqlitePool, items: &[Assignment]) -> Result<SyncReport, sqlx::Error> {
    let mut report = SyncReport::default();
    for item in items {
        let tidy = tidy_title(&item.title);
        let links = links_json(&item.links);
        let known: Option<KnownTask> = sqlx::query_as(
            "SELECT id, completed, deadline, description, links FROM tasks WHERE external_id = ?",
        )
        .bind(&item.uid)
        .fetch_optional(pool)
        .await?;
        match known {
            None => {
                let category = classify_task(&item.title);
                sqlx::query(
                    "INSERT INTO tasks (description, completed, item_order, priority, deadline, \
                     duration_minutes, task_category, external_id, links) \
                     VALUES (?, false, (SELECT COALESCE(MAX(item_order), -1) + 1 FROM tasks), ?, ?, ?, ?, ?, ?)",
                )
                .bind(&tidy)
                .bind(NEW_TASK_PRIORITY)
                .bind(item.due)
                .bind(default_duration_for_category(category))
                .bind(category)
                .bind(&item.uid)
                .bind(&links)
                .execute(pool)
                .await?;
                report.added += 1;
            }
            Some((id, completed, deadline, description, stored_links)) => {
                let moved = !completed
                    && (deadline != Some(item.due)
                        || (description == item.title && tidy != item.title));
                if moved {
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
                }
                let relinked = stored_links != links;
                if relinked {
                    sqlx::query("UPDATE tasks SET links = ? WHERE id = ?")
                        .bind(&links)
                        .bind(id)
                        .execute(pool)
                        .await?;
                }
                if moved || relinked {
                    report.updated += 1;
                }
            }
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
