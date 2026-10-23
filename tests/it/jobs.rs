//! `jobs`: digest parsing, rule scoring, and `scan` creating (and not recreating) tasks.

use chrono::{DateTime, Duration, Utc};
use sqlx::SqlitePool;
use triptych::email::links::{Link, anchors, from_json, to_json};
use triptych::jobs::digest::{Posting, parse_digest};
use triptych::jobs::score::{Profile, score};
use triptych::jobs::{list, scan};
use triptych::notify::{new_mail_alert, shortlist_alert};

const SENDER: &str = "noreply@swelist.com";

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
    triptych::migrations::run_email_migration(&pool)
        .await
        .expect("email schema");
    pool
}

/// The shape of a real digest: intro (with a colon), marker, postings, marker, footer.
fn digest(postings: &[&str]) -> String {
    let mut lines = vec![
        "Hey there! Here is your daily update (9/26) of tech internships from swelist: ...",
        "Second intro line",
        "Link to Simplify",
    ];
    lines.extend_from_slice(postings);
    lines.extend([
        "See more details by visiting the repo: Summer2026-Internships",
        "Click here to Unsubscribe",
    ]);
    lines.join("\n")
}

const POSTINGS: [&str; 6] = [
    " Acme Corp:Software Engineer Intern - Summer 2027",
    "Globex: Hardware Validation Intern",
    "Initech: ML Engineer Intern",
    "Initech: ML Engineer Intern",
    "Umbrella: PhD Research Intern",
    "Hooli: Engineering Intern",
];

fn posting(role: &str) -> Posting {
    Posting {
        company: "Co".into(),
        role: role.into(),
        url: None,
    }
}

async fn seed(
    pool: &SqlitePool,
    from: &str,
    body: &str,
    links: Option<&str>,
    date: DateTime<Utc>,
) -> i64 {
    sqlx::query(
        "INSERT INTO email_messages \
         (uid, message_id, account, from_addr, subject, date_utc, body_text, links) \
         VALUES (abs(random()), hex(randomblob(8)), 'a', ?, '52 New Internships', ?, ?, ?)",
    )
    .bind(from)
    .bind(date)
    .bind(body)
    .bind(links)
    .execute(pool)
    .await
    .expect("insert digest")
    .last_insert_rowid()
}

#[test]
fn parse_digest_reads_only_the_postings_and_collapses_repeats() {
    let found = parse_digest(&digest(&POSTINGS), &[]);
    let names: Vec<_> = found
        .iter()
        .map(|p| format!("{}|{}", p.company, p.role))
        .collect();
    assert_eq!(
        names,
        [
            "Acme Corp|Software Engineer Intern - Summer 2027",
            "Globex|Hardware Validation Intern",
            "Initech|ML Engineer Intern",
            "Umbrella|PhD Research Intern",
            "Hooli|Engineering Intern",
        ]
    );
}

#[test]
fn parse_digest_fails_closed_when_a_marker_is_missing() {
    let body = digest(&POSTINGS);
    assert!(parse_digest(&body.replace("Link to Simplify", "Other"), &[]).is_empty());
    assert!(parse_digest(&body.replace("See more details", "Other"), &[]).is_empty());
    assert!(parse_digest("just a note: hello", &[]).is_empty());
}

#[test]
fn parse_digest_pairs_a_posting_with_the_anchor_of_the_same_text() {
    let links = [
        Link {
            text: "Click here to Unsubscribe".into(),
            href: "https://x.test/unsub".into(),
        },
        Link {
            text: "Initech: ML Engineer Intern".into(),
            href: "https://boards.greenhouse.io/initech/jobs/1".into(),
        },
    ];
    let found = parse_digest(&digest(&POSTINGS), &links);
    let url = |company: &str| {
        found
            .iter()
            .find(|p| p.company == company)
            .and_then(|p| p.url.clone())
    };
    assert_eq!(
        url("Initech").as_deref(),
        Some("https://boards.greenhouse.io/initech/jobs/1")
    );
    assert_eq!(url("Acme Corp"), None);
}

#[test]
fn scoring_matches_whole_words_and_applies_the_excludes() {
    let profile = Profile::default();
    let check = |role: &str| {
        let v = score(&posting(role), &profile);
        (v.score, v.hard_no, v.shortlisted(&profile))
    };
    assert_eq!(check("Software Engineer Intern"), (2, false, true));
    assert_eq!(check("ML Engineer Intern"), (2, false, true));
    assert_eq!(check("HTML Designer Intern"), (0, false, false));
    assert_eq!(check("Hardware Validation Intern"), (-6, false, false));
    assert_eq!(check("Data Analyst Intern"), (-1, false, false));
    assert_eq!(check("Software Intern (MS/PhD)"), (2, true, false));
    assert_eq!(check("Software Intern - Sophomore Only"), (2, true, false));
}

#[test]
fn a_profile_file_overrides_only_the_fields_it_names() {
    let profile: Profile = toml::from_str("shortlist_at = 5\nup = [\"rust\"]").unwrap();
    assert_eq!(profile.shortlist_at, 5);
    assert_eq!(profile.up, ["rust"]);
    assert_eq!(profile.senders, [SENDER]);
}

#[tokio::test]
async fn scan_makes_tasks_for_good_fits_once_and_never_recreates_deleted_ones() {
    let pool = test_pool().await;
    let now = Utc::now();
    let links = to_json(&[Link {
        text: "Initech: ML Engineer Intern".into(),
        href: "https://boards.greenhouse.io/initech/jobs/1".into(),
    }]);
    seed(&pool, SENDER, &digest(&POSTINGS), links.as_deref(), now).await;

    let made = scan(&pool, &Profile::default(), now).await.unwrap();
    assert_eq!(
        made,
        [
            "Acme Corp - Software Engineer Intern - Summer 2027",
            "Initech - ML Engineer Intern"
        ]
    );
    let tasks: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT description, links FROM tasks ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        tasks[0].0,
        "Apply: Acme Corp - Software Engineer Intern - Summer 2027"
    );
    assert_eq!(tasks[0].1, None);
    assert_eq!(
        tasks[1].1.as_deref(),
        Some(r#"["https://boards.greenhouse.io/initech/jobs/1"]"#)
    );

    assert_eq!(list(&pool, false).await.unwrap().len(), 2);
    let all = list(&pool, true).await.unwrap();
    assert_eq!(all.len(), 5, "repeat collapsed, skipped ones kept");
    assert_eq!(all[4].1, "Globex", "worst last");

    assert!(
        scan(&pool, &Profile::default(), now)
            .await
            .unwrap()
            .is_empty()
    );
    sqlx::query("DELETE FROM tasks")
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        scan(&pool, &Profile::default(), now)
            .await
            .unwrap()
            .is_empty()
    );
    let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(left, 0, "a deleted task stays deleted");
}

#[tokio::test]
async fn scan_caps_tasks_per_digest_and_ignores_stale_digests_and_other_senders() {
    let pool = test_pool().await;
    let now = Utc::now();
    let capped = Profile {
        max_per_digest: 1,
        ..Profile::default()
    };
    seed(&pool, SENDER, &digest(&POSTINGS), None, now).await;
    seed(
        &pool,
        SENDER,
        &digest(&["Old Co: Software Intern"]),
        None,
        now - Duration::days(30),
    )
    .await;
    seed(
        &pool,
        "someone@example.com",
        &digest(&["Other Co: Software Intern"]),
        None,
        now,
    )
    .await;

    let made = scan(&pool, &capped, now).await.unwrap();
    assert_eq!(made, ["Acme Corp - Software Engineer Intern - Summer 2027"]);
    let over: Vec<(String, String)> =
        sqlx::query_as("SELECT status, reason FROM job_postings WHERE company = 'Initech'")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        over,
        [("skipped".to_string(), "+ml (over cap)".to_string())]
    );
    let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM job_postings")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, 5, "no row for the stale digest or the other sender");
}

#[test]
fn anchors_keep_web_links_with_their_text_and_drop_repeats() {
    let html = r#"<p><a href="https://x.test/a?b=1&amp;c=2">Acme:  <b>SWE</b></a>
        <a href='mailto:a@b.test'>mail</a> <A HREF=https://y.test/z>Y</A>
        <a class="c" href="https://x.test/a?b=1&amp;c=2">Acme: SWE</a> <a>no href</a></p>"#;
    let found = anchors(html);
    assert_eq!(
        found,
        [
            Link {
                text: "Acme: SWE".into(),
                href: "https://x.test/a?b=1&c=2".into()
            },
            Link {
                text: "Y".into(),
                href: "https://y.test/z".into()
            },
        ]
    );
    assert_eq!(from_json(to_json(&found).as_deref()), found);
    assert_eq!(to_json(&[]), None);
    assert!(from_json(Some("not json")).is_empty());
}

#[tokio::test]
async fn digest_mail_does_not_alert_as_plain_mail_but_other_mail_does() {
    let pool = test_pool().await;
    let now = Utc::now();
    seed(&pool, SENDER, &digest(&POSTINGS), None, now).await;
    assert_eq!(new_mail_alert(&pool, "a", 0, now).await.unwrap(), None);
    seed(&pool, "friend@example.com", "hi", None, now).await;
    assert!(new_mail_alert(&pool, "a", 0, now).await.unwrap().is_some());
}

#[test]
fn shortlist_alert_names_the_postings() {
    assert_eq!(shortlist_alert(&[]), None);
    let one = shortlist_alert(&["Acme - SWE".to_string()]).unwrap();
    assert_eq!(
        (one.title.as_str(), one.body.as_str()),
        ("1 job to apply to", "Acme - SWE")
    );
    let many: Vec<String> = (1..=5).map(|n| format!("Co{n} - SWE")).collect();
    let alert = shortlist_alert(&many).unwrap();
    assert_eq!(alert.title, "5 jobs to apply to");
    assert_eq!(alert.body, "Co1 - SWE\nCo2 - SWE\nCo3 - SWE\n+2 more");
}

#[tokio::test]
async fn the_email_migration_is_idempotent() {
    let pool = test_pool().await;
    triptych::migrations::run_email_migration(&pool)
        .await
        .expect("second run");
}
