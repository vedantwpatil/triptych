use chrono::{DateTime, Duration, Local, TimeZone, Utc};
use triptych::app::Task;
use triptych::urgency::*;

fn task(
    priority: i32,
    scheduled_in: Option<Duration>,
    deadline_in: Option<Duration>,
) -> (Task, DateTime<Utc>) {
    let now = Utc.with_ymd_and_hms(2026, 9, 19, 12, 0, 0).unwrap();
    let task = Task {
        id: 1,
        description: "t".into(),
        completed: false,
        item_order: None,
        scheduled_at: scheduled_in.map(|d| now + d),
        deadline: deadline_in.map(|d| now + d),
        duration_minutes: None,
        priority,
        tags: None,
        task_category: None,
    };
    (task, now)
}

#[test]
fn priority_rises_in_tiers_as_the_date_nears() {
    let cases = [
        (Duration::days(30), 0),
        (Duration::days(6), 1),
        (Duration::days(3), 2),
        (Duration::hours(25), 2),
        (Duration::hours(24), 3),
        (Duration::hours(-5), 3),
    ];
    for (left, want) in cases {
        let (t, now) = task(0, None, Some(left));
        assert_eq!(effective_priority(&t, now), want, "low task due in {left}");
    }
}

#[test]
fn never_lowers_a_typed_priority_and_uses_the_earlier_date() {
    let (t, now) = task(3, None, Some(Duration::days(30)));
    assert_eq!(effective_priority(&t, now), 3);
    let (t, now) = task(0, Some(Duration::hours(2)), Some(Duration::days(30)));
    assert_eq!(effective_priority(&t, now), 3);
}

#[test]
fn undated_and_finished_tasks_keep_their_own_priority() {
    let (t, now) = task(0, None, None);
    assert_eq!(effective_priority(&t, now), 0);
    let (mut t, now) = task(0, None, Some(Duration::hours(1)));
    t.completed = true;
    assert_eq!(effective_priority(&t, now), 0);
}

#[test]
fn badge_marks_only_automatic_raises() {
    let (t, now) = task(0, None, Some(Duration::hours(2)));
    assert_eq!(priority_badge(&t, now), Some((3, "[URGENT↑]".to_string())));
    let (t, now) = task(3, None, Some(Duration::hours(2)));
    assert_eq!(priority_badge(&t, now), Some((3, "[URGENT]".to_string())));
    let (t, now) = task(0, None, None);
    assert_eq!(priority_badge(&t, now), Some((0, "[LOW]".to_string())));
}

#[test]
fn deadline_badge_names_the_day() {
    let now = Local.with_ymd_and_hms(2026, 9, 19, 12, 0, 0).unwrap();
    let end_of = |d: u32| {
        Local
            .with_ymd_and_hms(2026, 9, d, 23, 59, 59)
            .unwrap()
            .with_timezone(&Utc)
    };
    assert_eq!(deadline_badge(end_of(19), now), "[DUE TODAY]");
    assert_eq!(deadline_badge(end_of(20), now), "[DUE TMR]");
    assert_eq!(deadline_badge(end_of(25), now), "[DUE Fri]");
    assert_eq!(deadline_badge(end_of(30), now), "[DUE 09/30]");
    let five_pm = Local
        .with_ymd_and_hms(2026, 9, 19, 17, 0, 0)
        .unwrap()
        .with_timezone(&Utc);
    assert_eq!(deadline_badge(five_pm, now), "[DUE TODAY 5:00pm]");
    let earlier = Local
        .with_ymd_and_hms(2026, 9, 19, 9, 0, 0)
        .unwrap()
        .with_timezone(&Utc);
    assert_eq!(deadline_badge(earlier, now), "[OVERDUE]");
}
