use chrono::{DateTime, Datelike, Local, TimeZone, Timelike, Utc, Weekday};
use triptych::recurrence::{Recurrence, extract};

/// A local wall-clock moment as UTC, so the tests hold in any time zone.
fn local(y: i32, m: u32, d: u32, h: u32) -> DateTime<Utc> {
    Local
        .with_ymd_and_hms(y, m, d, h, 0, 0)
        .earliest()
        .expect("valid local time")
        .with_timezone(&Utc)
}

fn ymd(dt: DateTime<Utc>) -> (i32, u32, u32) {
    let l = dt.with_timezone(&Local);
    (l.year(), l.month(), l.day())
}

#[test]
fn extracts_every_phrases_anywhere() {
    let cases = [
        ("standup every day", Recurrence::Daily, "standup"),
        ("every week review", Recurrence::Weekly, "review"),
        ("pay rent every month", Recurrence::Monthly, "pay rent"),
        ("inbox every weekday 9am", Recurrence::Weekdays, "inbox 9am"),
        (
            "gym Every Monday 6pm",
            Recurrence::On(Weekday::Mon),
            "gym monday 6pm",
        ),
        (
            "trash every fri",
            Recurrence::On(Weekday::Fri),
            "trash friday",
        ),
    ];
    for (input, rule, text) in cases {
        assert_eq!(extract(input), Some((rule, text.to_string())), "{input}");
    }
}

#[test]
fn bare_adverbs_count_only_at_the_end() {
    assert_eq!(
        extract("pay rent monthly #bills !!"),
        Some((Recurrence::Monthly, "pay rent #bills !!".to_string()))
    );
    assert_eq!(
        extract("water plants daily"),
        Some((Recurrence::Daily, "water plants".to_string()))
    );
    assert_eq!(extract("review weekly report"), None);
    assert_eq!(extract("daily standup notes"), None);
}

#[test]
fn unknown_or_absent_phrases_are_left_alone() {
    for input in [
        "buy milk",
        "every 2 weeks",
        "every man for himself",
        "every",
    ] {
        assert_eq!(extract(input), None, "{input}");
    }
}

#[test]
fn keys_round_trip() {
    let rules = [
        Recurrence::Daily,
        Recurrence::Weekdays,
        Recurrence::Weekly,
        Recurrence::Monthly,
        Recurrence::On(Weekday::Mon),
        Recurrence::On(Weekday::Tue),
        Recurrence::On(Weekday::Wed),
        Recurrence::On(Weekday::Thu),
        Recurrence::On(Weekday::Fri),
        Recurrence::On(Weekday::Sat),
        Recurrence::On(Weekday::Sun),
    ];
    for rule in rules {
        assert_eq!(Recurrence::from_key(rule.key()), Some(rule), "{rule:?}");
    }
    assert_eq!(Recurrence::from_key("yearly"), None);
}

#[test]
fn next_after_moves_by_the_rule_and_keeps_the_time_of_day() {
    let early = local(2026, 10, 1, 8); // before every anchor below
    let thu = local(2026, 10, 1, 9);
    let fri = local(2026, 10, 2, 9);
    let cases = [
        (Recurrence::Daily, thu, (2026, 10, 2)),
        (Recurrence::Weekly, thu, (2026, 10, 8)),
        (Recurrence::Weekdays, fri, (2026, 10, 5)),
        (Recurrence::Weekdays, thu, (2026, 10, 2)),
        (Recurrence::On(Weekday::Fri), thu, (2026, 10, 2)),
        (Recurrence::On(Weekday::Fri), fri, (2026, 10, 9)),
        (Recurrence::On(Weekday::Mon), fri, (2026, 10, 5)),
        (Recurrence::Monthly, thu, (2026, 11, 1)),
    ];
    for (rule, anchor, want) in cases {
        let next = rule.next_after(anchor, early);
        assert_eq!(ymd(next), want, "{rule:?} from {anchor}");
        assert_eq!(next.with_timezone(&Local).hour(), 9, "{rule:?} keeps 9:00");
    }
}

#[test]
fn monthly_clamps_a_short_month() {
    let next = Recurrence::Monthly.next_after(local(2026, 1, 31, 9), local(2026, 1, 1, 0));
    assert_eq!(ymd(next), (2026, 2, 28));
}

#[test]
fn a_late_finish_rolls_past_now() {
    let due = local(2026, 10, 1, 9);
    let now = local(2026, 10, 10, 12);
    let next = Recurrence::Daily.next_after(due, now);
    assert_eq!(ymd(next), (2026, 10, 11));
    let next = Recurrence::Weekly.next_after(due, now);
    assert_eq!(ymd(next), (2026, 10, 15));
}

#[test]
fn an_occurrence_landing_exactly_on_now_is_skipped() {
    let next = Recurrence::Daily.next_after(local(2026, 10, 1, 9), local(2026, 10, 2, 9));
    assert_eq!(ymd(next), (2026, 10, 3));
}

#[test]
fn weekly_and_weekdays_work_as_bare_trailing_words() {
    assert_eq!(
        extract("pay rent weekly"),
        Some((Recurrence::Weekly, "pay rent".to_string()))
    );
    assert_eq!(
        extract("standup weekdays"),
        Some((Recurrence::Weekdays, "standup".to_string()))
    );
}

#[test]
fn next_dates_keep_the_gap_between_scheduled_and_deadline() {
    let scheduled = local(2026, 10, 5, 9); // Monday
    let deadline = local(2026, 10, 9, 17); // Friday
    let now = local(2026, 10, 6, 12);
    let (s, d) = Recurrence::Weekly.next_dates(Some(scheduled), Some(deadline), now);
    assert_eq!(s.map(ymd), Some((2026, 10, 12)));
    assert_eq!(d.map(ymd), Some((2026, 10, 16)));
    assert_eq!(d.map(|d| d.with_timezone(&Local).hour()), Some(17));
}

#[test]
fn next_dates_for_an_undated_task_is_a_deadline_at_day_end() {
    let now = local(2026, 10, 6, 12);
    let (s, d) = Recurrence::Daily.next_dates(None, None, now);
    assert_eq!(s, None);
    let d = d.expect("deadline").with_timezone(&Local);
    assert_eq!((d.year(), d.month(), d.day()), (2026, 10, 7));
    assert_eq!((d.hour(), d.minute()), (23, 59));
}
