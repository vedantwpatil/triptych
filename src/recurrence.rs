//! Repeat rules for tasks: finding the phrase in input text, the key stored in `tasks.recurrence`,
//! and the dates of the next occurrence. Terminal-free so `tests/it/recurrence.rs` reaches it.

use chrono::{
    DateTime, Datelike, Days, Local, Months, NaiveDate, NaiveDateTime, NaiveTime, Utc, Weekday,
};
use serde::{Deserialize, Serialize};

use crate::app::resolve_local_datetime;

/// Stops a runaway roll past `now` (a daily task left undone for ten years).
const MAX_STEPS: u32 = 4000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Recurrence {
    Daily,
    /// Monday to Friday.
    Weekdays,
    Weekly,
    /// Same day of the month; a short month clamps (31 Jan becomes 28 Feb, then stays on the 28th).
    Monthly,
    /// Every given weekday ("every friday").
    On(Weekday),
}

/// `(short, long)` lowercase names; `long` is what the date parser resolves, `short` is the stored key.
const fn names(day: Weekday) -> (&'static str, &'static str) {
    match day {
        Weekday::Mon => ("mon", "monday"),
        Weekday::Tue => ("tue", "tuesday"),
        Weekday::Wed => ("wed", "wednesday"),
        Weekday::Thu => ("thu", "thursday"),
        Weekday::Fri => ("fri", "friday"),
        Weekday::Sat => ("sat", "saturday"),
        Weekday::Sun => ("sun", "sunday"),
    }
}

impl Recurrence {
    /// The text stored in `tasks.recurrence`, also shown next to the `↻` mark.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Weekdays => "weekdays",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
            Self::On(day) => names(day).0,
        }
    }

    /// Inverse of [`key`](Self::key); `None` for anything else.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "daily" => Some(Self::Daily),
            "weekdays" => Some(Self::Weekdays),
            "weekly" => Some(Self::Weekly),
            "monthly" => Some(Self::Monthly),
            other => other.parse::<Weekday>().ok().map(Self::On),
        }
    }

    /// The first date after `day` this rule allows.
    fn step(self, day: NaiveDate) -> NaiveDate {
        let plus = |n: u64| day.checked_add_days(Days::new(n)).unwrap_or(day);
        match self {
            Self::Daily => plus(1),
            Self::Weekly => plus(7),
            Self::Monthly => day.checked_add_months(Months::new(1)).unwrap_or(day),
            Self::Weekdays => (1..=3)
                .map(plus)
                .find(|d| d.weekday().number_from_monday() <= 5)
                .unwrap_or(day),
            Self::On(want) => (1..=7)
                .map(plus)
                .find(|d| d.weekday() == want)
                .unwrap_or(day),
        }
    }

    /// The occurrence after `anchor`, at the same local time of day and later than `now`: finishing a
    /// task days late rolls forward instead of leaving an overdue copy behind.
    #[must_use]
    pub fn next_after(self, anchor: DateTime<Utc>, now: DateTime<Utc>) -> DateTime<Utc> {
        let local = anchor.with_timezone(&Local).naive_local();
        let (mut day, time) = (local.date(), local.time());
        let mut next = anchor;
        for _ in 0..MAX_STEPS {
            day = self.step(day);
            next = resolve_local_datetime(day.and_time(time));
            if next > now {
                break;
            }
        }
        next
    }

    /// `(scheduled_at, deadline)` for the next occurrence of a task with these dates. Both move by the
    /// same local wall-clock amount, keeping the gap between them. A task with neither date gets a
    /// deadline at the end of the next occurrence day.
    #[must_use]
    pub fn next_dates(
        self,
        scheduled: Option<DateTime<Utc>>,
        deadline: Option<DateTime<Utc>>,
        now: DateTime<Utc>,
    ) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
        let naive = |d: DateTime<Utc>| d.with_timezone(&Local).naive_local();
        let Some(primary) = scheduled.or(deadline) else {
            let day = now.with_timezone(&Local).date_naive();
            let end = day
                .and_hms_opt(23, 59, 59)
                .unwrap_or_else(|| day.and_time(NaiveTime::MIN));
            return (
                None,
                Some(self.next_after(resolve_local_datetime(end), now)),
            );
        };
        let old = naive(primary);
        let shift = naive(self.next_after(primary, now)) - old;
        let moved = |d: DateTime<Utc>| {
            let base: NaiveDateTime = naive(d);
            resolve_local_datetime(base.checked_add_signed(shift).unwrap_or(base))
        };
        (scheduled.map(moved), deadline.map(moved))
    }
}

/// Finds a repeat phrase in `input`. Returns the rule and `input` without the phrase, with `every
/// friday` left as `friday` so the date parser still sets the first due date.
///
/// Phrases: `every day|week|weekday|month|<weekday>`, `daily`, `weekly`, `monthly`, `weekdays`. The
/// bare forms count only at the end (before any `#tag` or `!` words), so "review weekly report"
/// stays a plain task. `None` when there is no phrase; `input` is then untouched.
#[must_use]
pub fn extract(input: &str) -> Option<(Recurrence, String)> {
    let words: Vec<&str> = input.split_whitespace().collect();
    let (at, used, rule) =
        (0..words.len()).find_map(|i| phrase_at(&words[i..]).map(|(rule, n)| (i, n, rule)))?;
    let mut rest = words[..at].to_vec();
    if let Recurrence::On(day) = rule {
        rest.push(names(day).1);
    }
    rest.extend_from_slice(&words[at + used..]);
    Some((rule, rest.join(" ")))
}

/// The phrase at the start of `words`: its rule and word count.
fn phrase_at(words: &[&str]) -> Option<(Recurrence, usize)> {
    let first = words.first()?.to_lowercase();
    if first == "every" {
        let unit = words.get(1)?.to_lowercase();
        let rule = match unit.as_str() {
            "day" => Recurrence::Daily,
            "week" => Recurrence::Weekly,
            "weekday" => Recurrence::Weekdays,
            "month" => Recurrence::Monthly,
            other => Recurrence::On(other.parse::<Weekday>().ok()?),
        };
        return Some((rule, 2));
    }
    let rule = match first.as_str() {
        "daily" | "everyday" => Recurrence::Daily,
        "weekly" => Recurrence::Weekly,
        "monthly" => Recurrence::Monthly,
        "weekdays" => Recurrence::Weekdays,
        _ => return None,
    };
    let trailing = words[1..]
        .iter()
        .all(|w| w.starts_with('#') || w.starts_with('!'));
    trailing.then_some((rule, 1))
}
