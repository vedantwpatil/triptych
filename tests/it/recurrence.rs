use chrono::{DateTime, Datelike, Local, TimeZone, Timelike, Utc, Weekday, WeekdaySet};
use triptych::recurrence::{Recurrence, Unit, extract};

use Weekday::{Fri, Mon, Sat, Sun, Thu, Tue, Wed};

fn rule(every: u16, unit: Unit) -> Recurrence {
    Recurrence::new(every, unit).expect("valid interval")
}

fn on<const N: usize>(days: [Weekday; N]) -> Unit {
    Unit::On(WeekdaySet::from_array(days))
}

fn daily() -> Recurrence {
    rule(1, Unit::Day)
}

fn weekly() -> Recurrence {
    rule(1, Unit::Week)
}

fn monthly() -> Recurrence {
    rule(1, Unit::Month)
}

fn weekdays() -> Recurrence {
    rule(1, on([Mon, Tue, Wed, Thu, Fri]))
}

fn every_fri() -> Recurrence {
    rule(1, on([Fri]))
}

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
        ("standup every day", daily(), "standup"),
        ("every week review", weekly(), "review"),
        ("pay rent every month", monthly(), "pay rent"),
        ("inbox every weekday 9am", weekdays(), "inbox 9am"),
        ("gym Every Monday 6pm", rule(1, on([Mon])), "gym monday 6pm"),
        ("trash every fri", every_fri(), "trash friday"),
    ];
    for (input, want, text) in cases {
        assert_eq!(
            extract(input, Wed),
            Some((want, text.to_string())),
            "{input}"
        );
    }
}

#[test]
fn intervals_and_day_lists_parse() {
    let cases = [
        ("every 3 days", rule(3, Unit::Day), ""),
        ("every other day", rule(2, Unit::Day), ""),
        ("every 2 Weeks", rule(2, Unit::Week), ""),
        ("every 6 months", rule(6, Unit::Month), ""),
        ("every other friday", rule(2, on([Fri])), "friday"),
        ("every 2 weeks on fri", rule(2, on([Fri])), "friday"),
        (
            "every week on mon and wed",
            rule(1, on([Mon, Wed])),
            "monday",
        ),
        (
            "every mon, wed and fri",
            rule(1, on([Mon, Wed, Fri])),
            "monday",
        ),
        ("every tue/thu", rule(1, on([Tue, Thu])), "tuesday"),
        ("every tue thu", rule(1, on([Tue, Thu])), "tuesday"),
        ("every sat & sun", rule(1, on([Sat, Sun])), "saturday"),
        ("every mon,wed, #x", rule(1, on([Mon, Wed])), "monday #x"),
        ("every other mon wed", rule(2, on([Mon, Wed])), "monday"),
        ("every fri and buy milk", every_fri(), "friday and buy milk"),
        ("every weekdays", weekdays(), ""),
        ("gym biweekly #x", rule(2, Unit::Week), "gym #x"),
    ];
    for (input, want, text) in cases {
        assert_eq!(
            extract(input, Sun),
            Some((want, text.to_string())),
            "{input}"
        );
    }
}

#[test]
fn extract_leaves_the_soonest_day_from_today() {
    let input = "water every mon, wed and fri";
    for (today, day) in [
        (Sat, "monday"),
        (Mon, "wednesday"),
        (Tue, "wednesday"),
        (Wed, "friday"),
        (Thu, "friday"),
        (Fri, "monday"),
    ] {
        assert_eq!(
            extract(input, today).map(|(_, t)| t),
            Some(format!("water {day}"))
        );
    }
    // A single listed day equal to today still leaves that day (the search cycles back to it).
    assert_eq!(
        extract("water every wed", Wed).map(|(_, t)| t),
        Some("water wednesday".to_string())
    );
}

#[test]
fn bare_adverbs_count_only_at_the_end() {
    assert_eq!(
        extract("pay rent monthly #bills !!", Wed),
        Some((monthly(), "pay rent #bills !!".to_string()))
    );
    assert_eq!(
        extract("water plants daily", Wed),
        Some((daily(), "water plants".to_string()))
    );
    assert_eq!(extract("review weekly report", Wed), None);
    assert_eq!(extract("daily standup notes", Wed), None);
    assert_eq!(extract("biweekly sync notes", Wed), None);
}

#[test]
fn unknown_or_absent_phrases_are_left_alone() {
    for input in [
        "buy milk",
        "every man for himself",
        "every",
        "every 2",
        "every other",
        "every 0 days",
        "every 1000 days",
        "every 2 fri",
    ] {
        assert_eq!(extract(input, Wed), None, "{input}");
    }
    // Still weekly; the tail is just not a day list.
    assert_eq!(
        extract("every week on the 5th", Wed),
        Some((weekly(), "on the 5th".to_string()))
    );
}

#[test]
fn keys_round_trip() {
    let all = [Mon, Tue, Wed, Thu, Fri, Sat, Sun];
    let mut rules = vec![
        daily(),
        weekdays(),
        weekly(),
        monthly(),
        rule(3, Unit::Day),
        rule(2, Unit::Week),
        rule(999, Unit::Month),
        rule(1, on([Mon, Wed, Fri])),
        rule(2, on([Mon, Wed])),
        rule(3, on([Mon, Tue, Wed, Thu, Fri])),
    ];
    rules.extend(all.map(|d| rule(1, on([d]))));
    rules.extend(all.map(|d| rule(2, on([d]))));
    for r in rules {
        assert_eq!(Recurrence::from_key(&r.key()), Some(r), "{r:?}");
    }
    let keys = [
        (daily(), "daily"),
        (weekdays(), "weekdays"),
        (weekly(), "weekly"),
        (monthly(), "monthly"),
        (every_fri(), "fri"),
        (rule(1, on([Mon, Wed, Fri])), "mon,wed,fri"),
        (rule(3, Unit::Day), "every 3 days"),
        (rule(2, Unit::Week), "every 2 weeks"),
        (rule(3, Unit::Month), "every 3 months"),
        (rule(2, on([Mon, Wed])), "every 2 weeks on mon,wed"),
        (
            rule(2, on([Mon, Tue, Wed, Thu, Fri])),
            "every 2 weeks on mon,tue,wed,thu,fri",
        ),
    ];
    for (r, key) in keys {
        assert_eq!(r.key(), key);
    }
    // Keys already stored by older versions.
    for (key, want) in [
        ("daily", daily()),
        ("weekdays", weekdays()),
        ("sun", rule(1, on([Sun]))),
    ] {
        assert_eq!(Recurrence::from_key(key), Some(want), "{key}");
    }
    for bad in ["yearly", "", "daily #x", "every 0 days", "fri and"] {
        assert_eq!(Recurrence::from_key(bad), None, "{bad:?}");
    }
    assert_eq!(Recurrence::new(0, Unit::Day), None);
    assert_eq!(Recurrence::new(1000, Unit::Day), None);
    assert_eq!(Recurrence::new(1, Unit::On(WeekdaySet::EMPTY)), None);
}

#[test]
fn serde_is_the_key_string() {
    let r = rule(2, on([Mon, Wed]));
    let json = serde_json::to_string(&r).expect("serialize");
    assert_eq!(json, "\"every 2 weeks on mon,wed\"");
    assert_eq!(serde_json::from_str::<Recurrence>(&json).expect("parse"), r);
    assert_eq!(
        serde_json::from_str::<Recurrence>("\"fri\"").expect("legacy key"),
        every_fri()
    );
    assert!(serde_json::from_str::<Recurrence>("\"yearly\"").is_err());
}

#[test]
fn next_after_moves_by_the_rule_and_keeps_the_time_of_day() {
    let early = local(2026, 10, 1, 8); // before every anchor below
    let thu = local(2026, 10, 1, 9);
    let fri = local(2026, 10, 2, 9);
    let cases = [
        (daily(), thu, (2026, 10, 2)),
        (weekly(), thu, (2026, 10, 8)),
        (weekdays(), fri, (2026, 10, 5)),
        (weekdays(), thu, (2026, 10, 2)),
        (every_fri(), thu, (2026, 10, 2)),
        (every_fri(), fri, (2026, 10, 9)),
        (rule(1, on([Mon])), fri, (2026, 10, 5)),
        (monthly(), thu, (2026, 11, 1)),
        (rule(3, Unit::Day), thu, (2026, 10, 4)),
        (rule(2, Unit::Week), thu, (2026, 10, 15)),
        (rule(3, Unit::Month), thu, (2027, 1, 1)),
    ];
    for (r, anchor, want) in cases {
        let next = r.next_after(anchor, early);
        assert_eq!(ymd(next), want, "{r:?} from {anchor}");
        assert_eq!(next.with_timezone(&Local).hour(), 9, "{r:?} keeps 9:00");
    }
}

#[test]
fn interval_day_lists_skip_whole_weeks_only_when_wrapping() {
    let early = local(2026, 10, 1, 8);
    // 2026-10-05 is a Monday.
    let mon = local(2026, 10, 5, 9);
    let wed = local(2026, 10, 7, 9);
    let fri = local(2026, 10, 9, 9);
    let cases = [
        (rule(2, on([Mon, Wed])), mon, (2026, 10, 7)),
        (rule(2, on([Mon, Wed])), wed, (2026, 10, 19)),
        (rule(2, on([Fri])), fri, (2026, 10, 23)),
        (rule(3, on([Fri])), fri, (2026, 10, 30)),
        (rule(2, on([Mon, Fri])), wed, (2026, 10, 9)),
        (rule(2, on([Mon, Fri])), fri, (2026, 10, 19)),
    ];
    for (r, anchor, want) in cases {
        assert_eq!(
            ymd(r.next_after(anchor, early)),
            want,
            "{r:?} from {anchor}"
        );
    }
}

#[test]
fn monthly_clamps_a_short_month() {
    let next = monthly().next_after(local(2026, 1, 31, 9), local(2026, 1, 1, 0));
    assert_eq!(ymd(next), (2026, 2, 28));
}

#[test]
fn a_late_finish_rolls_past_now() {
    let due = local(2026, 10, 1, 9);
    let now = local(2026, 10, 10, 12);
    let next = daily().next_after(due, now);
    assert_eq!(ymd(next), (2026, 10, 11));
    let next = weekly().next_after(due, now);
    assert_eq!(ymd(next), (2026, 10, 15));
}

#[test]
fn an_occurrence_landing_exactly_on_now_is_skipped() {
    let next = daily().next_after(local(2026, 10, 1, 9), local(2026, 10, 2, 9));
    assert_eq!(ymd(next), (2026, 10, 3));
}

#[test]
fn weekly_and_weekdays_work_as_bare_trailing_words() {
    assert_eq!(
        extract("pay rent weekly", Wed),
        Some((weekly(), "pay rent".to_string()))
    );
    assert_eq!(
        extract("standup weekdays", Wed),
        Some((weekdays(), "standup".to_string()))
    );
}

#[test]
fn next_dates_keep_the_gap_between_scheduled_and_deadline() {
    let scheduled = local(2026, 10, 5, 9); // Monday
    let deadline = local(2026, 10, 9, 17); // Friday
    let now = local(2026, 10, 6, 12);
    let (s, d) = weekly().next_dates(Some(scheduled), Some(deadline), now);
    assert_eq!(s.map(ymd), Some((2026, 10, 12)));
    assert_eq!(d.map(ymd), Some((2026, 10, 16)));
    assert_eq!(d.map(|d| d.with_timezone(&Local).hour()), Some(17));
}

#[test]
fn next_dates_for_an_undated_task_is_a_deadline_at_day_end() {
    let now = local(2026, 10, 6, 12);
    let (s, d) = daily().next_dates(None, None, now);
    assert_eq!(s, None);
    let d = d.expect("deadline").with_timezone(&Local);
    assert_eq!((d.year(), d.month(), d.day()), (2026, 10, 7));
    assert_eq!((d.hour(), d.minute()), (23, 59));
}
