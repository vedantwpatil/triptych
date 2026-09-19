use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveTime, Timelike, Utc};
use triptych::nlp::ParsedItem;
use triptych::nlp::rules::*;
use triptych::nlp::types::{Event, Task};

fn parse_task(input: &str) -> Task {
    match RuleParser::try_parse(input).expect("expected a parsed item") {
        ParsedItem::Task(task) => task,
        ParsedItem::Event(event) => panic!("expected Task, got Event: {event:?}"),
    }
}

#[test]
fn deadline_by_weekday_sets_end_of_day() {
    let task = parse_task("finish slides by friday");
    let deadline = task.deadline.expect("deadline should be set");
    let local = deadline.with_timezone(&Local);
    assert_eq!(local.weekday(), chrono::Weekday::Fri);
    assert_eq!(local.time(), NaiveTime::from_hms_opt(23, 59, 59).unwrap());
    assert_eq!(task.title, "finish slides");
}

#[test]
fn bare_hour_duration_converts_to_minutes() {
    let task = parse_task("write report 3h");
    assert_eq!(task.duration_minutes, Some(180));
    assert_eq!(task.title, "write report");
}

#[test]
fn bare_minute_duration_is_not_hours() {
    let task = parse_task("quick call 90m");
    assert_eq!(task.duration_minutes, Some(90));
}

#[test]
fn deadline_and_duration_combine_without_becoming_an_event() {
    let task = parse_task("MATH 475 homework by wednesday 3h");
    assert!(task.deadline.is_some());
    assert_eq!(task.duration_minutes, Some(180));
    assert_eq!(task.title, "MATH 475 homework");
    assert!(task.due_date.is_none());
}

#[test]
fn word_boundary_guard_prevents_false_positive_duration() {
    // "3 more things" must not be misparsed as a "3m" duration.
    let task = parse_task("buy 3 more things");
    assert_eq!(task.duration_minutes, None);
    assert_eq!(task.title, "buy 3 more things");
}

#[test]
fn due_tomorrow_sets_deadline() {
    let task = parse_task("call dentist due tomorrow");
    let deadline = task.deadline.expect("deadline should be set");
    let tomorrow = (Local::now() + Duration::days(1)).date_naive();
    assert_eq!(deadline.with_timezone(&Local).date_naive(), tomorrow);
    assert_eq!(task.title, "call dentist");
}

/// Regression: "eom" used to call `.with_month(...)` before resetting the
/// day to 1, so parsing this on the last day of a 31-day month whose
/// successor is shorter (Jan -> Feb) computed an invalid "Feb 31" and
/// panicked. `now` is injected directly (bypassing `Local::now()`) so the
/// test is deterministic regardless of what day it actually runs on.
#[test]
fn eom_on_the_31st_does_not_panic_rolling_into_a_shorter_month() {
    let now = NaiveDate::from_ymd_opt(2026, 1, 31)
        .unwrap()
        .and_hms_opt(12, 0, 0)
        .unwrap()
        .and_local_timezone(Local)
        .unwrap();

    let (_, temporal) = parse_business_time(now)("eom").expect("eom should parse");
    let TemporalContext::Point(dt) = temporal else {
        panic!("expected a Point");
    };
    let local = dt.with_timezone(&Local);
    assert_eq!(
        local.date_naive(),
        NaiveDate::from_ymd_opt(2026, 1, 31).unwrap()
    );
    assert_eq!(local.time(), NaiveTime::from_hms_opt(17, 0, 0).unwrap());
}

/// December's "eom" must roll into January *of the following year*, not
/// panic or wrap within the same year.
#[test]
fn eom_in_december_rolls_into_next_year() {
    let now = NaiveDate::from_ymd_opt(2026, 12, 15)
        .unwrap()
        .and_hms_opt(12, 0, 0)
        .unwrap()
        .and_local_timezone(Local)
        .unwrap();

    let (_, temporal) = parse_business_time(now)("eom").expect("eom should parse");
    let TemporalContext::Point(dt) = temporal else {
        panic!("expected a Point");
    };
    let local = dt.with_timezone(&Local);
    assert_eq!(
        local.date_naive(),
        NaiveDate::from_ymd_opt(2026, 12, 31).unwrap()
    );
}

fn parse_event(input: &str) -> Event {
    match RuleParser::try_parse(input).expect("expected a parsed item") {
        ParsedItem::Event(event) => event,
        ParsedItem::Task(task) => panic!("expected Event, got Task: {task:?}"),
    }
}

fn local_hm(dt: DateTime<Utc>) -> (NaiveDate, u32, u32) {
    let local = dt.with_timezone(&Local);
    (local.date_naive(), local.hour(), local.minute())
}

fn tomorrow() -> NaiveDate {
    (Local::now() + Duration::days(1)).date_naive()
}

#[test]
fn bare_weekday_is_the_next_such_day() {
    let today = Local::now().date_naive();
    for name in [
        "monday",
        "tuesday",
        "wednesday",
        "thursday",
        "friday",
        "saturday",
        "sunday",
    ] {
        for input in [format!("call mom on {name}"), format!("call mom {name}")] {
            let task = parse_task(&input);
            let (date, ..) = local_hm(task.due_date.unwrap_or_else(|| panic!("no date: {input}")));
            assert_eq!(
                date.weekday().to_string().to_lowercase(),
                name[..3],
                "{input}"
            );
            assert!(
                date > today && date <= today + chrono::Duration::days(7),
                "{input} -> {date}"
            );
            assert_eq!(task.title, "call mom", "{input}");
        }
    }
}

#[test]
fn weekday_combines_with_time_and_ignores_abbreviations() {
    let task = parse_task("study group friday at 3pm");
    let (date, h, m) = local_hm(task.due_date.unwrap());
    assert_eq!((date.weekday(), h, m), (chrono::Weekday::Fri, 15, 0));
    assert_eq!(task.title, "study group");

    let task = parse_task("fix sat nav sundays");
    assert!(task.due_date.is_none());
    assert_eq!(task.title, "fix sat nav sundays");
}

#[test]
fn date_and_time_of_day_merge_into_one_moment() {
    let task = parse_task("submit report tomorrow at 3pm");
    assert_eq!(local_hm(task.due_date.unwrap()), (tomorrow(), 15, 0));
    assert_eq!(task.title, "submit report");
}

#[test]
fn time_with_no_date_means_today() {
    let today = Local::now().date_naive();
    for (input, hm) in [
        ("call mom at 3pm", (15, 0)),
        ("standup 6pm", (18, 0)),
        ("gym 15:30", (15, 30)),
    ] {
        let task = parse_task(input);
        assert_eq!(
            local_hm(task.due_date.unwrap()),
            (today, hm.0, hm.1),
            "{input}"
        );
    }
}

#[test]
fn month_day_and_weekday_combine_with_time() {
    let task = parse_task("dentist Sep 25 at 2pm");
    let (date, h, m) = local_hm(task.due_date.unwrap());
    assert_eq!((date.month(), date.day(), h, m), (9, 25, 14, 0));
    assert_eq!(task.title, "dentist");

    let task = parse_task("review next friday at 10am");
    let (date, h, _) = local_hm(task.due_date.unwrap());
    assert_eq!((date.weekday(), h), (chrono::Weekday::Fri, 10));
}

#[test]
fn time_range_becomes_an_event_with_its_duration() {
    let event = parse_event("team sync 3pm-5pm");
    let (_, h, _) = local_hm(event.start_time);
    assert_eq!(h, 15);
    assert_eq!(
        (event.end_time.unwrap() - event.start_time).num_minutes(),
        120
    );
    assert_eq!(event.title, "team sync");

    let event = parse_event("lab tomorrow 2-4pm");
    assert_eq!(local_hm(event.start_time), (tomorrow(), 14, 0));
    assert_eq!(local_hm(event.end_time.unwrap()), (tomorrow(), 16, 0));
}

#[test]
fn plain_numbers_are_not_times_or_ranges() {
    for input in [
        "read pages 5-7",
        "look at 5 things",
        "buy 1/2 cup sugar",
        "room 24",
    ] {
        let item = RuleParser::try_parse(input);
        assert!(
            item.is_none_or(|i| matches!(&i, ParsedItem::Task(t) if t.due_date.is_none())),
            "{input} parsed as a time"
        );
    }
}

#[test]
fn out_of_range_clock_values_fall_back_to_text() {
    for input in ["meet 25:99", "call 13pm", "call 0am"] {
        let item = RuleParser::try_parse(input);
        assert!(
            item.is_none_or(|i| matches!(&i, ParsedItem::Task(t) if t.due_date.is_none())),
            "{input} parsed as a time"
        );
    }
}

#[test]
fn for_prefix_is_part_of_an_explicit_duration() {
    let task = parse_task("stretch for 30m");
    assert_eq!(task.duration_minutes, Some(30));
    assert_eq!(task.title, "stretch");
}

#[test]
fn numeric_date_needs_year_on_or_day_above_twelve() {
    let now = NaiveDate::from_ymd_opt(2026, 9, 19)
        .unwrap()
        .and_hms_opt(12, 0, 0)
        .unwrap()
        .and_local_timezone(Local)
        .unwrap();
    let date = |input| {
        parse_numeric_date(now, false)(input).map(|(_, dt)| dt.with_timezone(&Local).date_naive())
    };

    assert_eq!(
        date("12/25").unwrap(),
        NaiveDate::from_ymd_opt(2026, 12, 25).unwrap()
    );
    assert_eq!(
        date("9/13").unwrap(),
        NaiveDate::from_ymd_opt(2027, 9, 13).unwrap()
    );
    assert_eq!(
        date("3/4/2028").unwrap(),
        NaiveDate::from_ymd_opt(2028, 3, 4).unwrap()
    );
    assert!(date("1/2").is_err());
    assert!(date("13/45").is_err());
    assert!(parse_numeric_date(now, true)("1/2").is_ok());
}
