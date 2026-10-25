//! Repeat rules for tasks: finding the phrase in input text, the key stored in `tasks.recurrence`,
//! and the dates of the next occurrence. Terminal-free so `tests/it/recurrence.rs` reaches it.

use chrono::{
    DateTime, Datelike, Days, Local, Months, NaiveDate, NaiveDateTime, NaiveTime, Utc, Weekday,
    WeekdaySet,
};
use serde::{Deserialize, Serialize};

use crate::app::resolve_local_datetime;

/// Stops a runaway roll past `now` (a daily task left undone for ten years).
const MAX_STEPS: u32 = 4000;

/// Largest interval a phrase may ask for ("every 999 days").
const MAX_EVERY: u16 = 999;

const WORKDAYS: WeekdaySet = WeekdaySet::from_array([
    Weekday::Mon,
    Weekday::Tue,
    Weekday::Wed,
    Weekday::Thu,
    Weekday::Fri,
]);

/// "Every `every` `unit`s". Serialized as its [`key`](Self::key) string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct Recurrence {
    every: u16,
    unit: Unit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Day,
    Week,
    /// Same day of the month; a short month clamps (31 Jan becomes 28 Feb, then stays on the 28th).
    Month,
    /// The given weekdays, in every `every`-th week ("every friday", "every 2 weeks on mon,wed").
    On(WeekdaySet),
}

impl From<Recurrence> for String {
    fn from(rule: Recurrence) -> Self {
        rule.key()
    }
}

impl TryFrom<String> for Recurrence {
    type Error = String;

    fn try_from(key: String) -> Result<Self, String> {
        Self::from_key(&key).ok_or_else(|| format!("unknown repeat rule: {key}"))
    }
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
    /// `None` unless `every` is within `1..=999` and an `On` set is not empty.
    #[must_use]
    pub fn new(every: u16, unit: Unit) -> Option<Self> {
        ((1..=MAX_EVERY).contains(&every) && !matches!(unit, Unit::On(set) if set.is_empty()))
            .then_some(Self { every, unit })
    }

    /// The text stored in `tasks.recurrence`, also shown next to the `↻` mark. Always a phrase
    /// [`from_key`](Self::from_key) reads back; the interval-1 forms match the keys of older rows.
    #[must_use]
    pub fn key(self) -> String {
        let list = |set: WeekdaySet| {
            set.iter(Weekday::Mon)
                .map(|d| names(d).0)
                .collect::<Vec<_>>()
                .join(",")
        };
        match (self.unit, self.every) {
            (Unit::Day, 1) => "daily".into(),
            (Unit::Week, 1) => "weekly".into(),
            (Unit::Month, 1) => "monthly".into(),
            (Unit::On(set), 1) if set == WORKDAYS => "weekdays".into(),
            (Unit::On(set), 1) => list(set),
            (Unit::Day, n) => format!("every {n} days"),
            (Unit::Week, n) => format!("every {n} weeks"),
            (Unit::Month, n) => format!("every {n} months"),
            (Unit::On(set), n) => format!("every {n} weeks on {}", list(set)),
        }
    }

    /// Inverse of [`key`](Self::key); `None` for anything else.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        let whole = |words: &[&str]| {
            phrase_at(words)
                .filter(|&(_, n)| n == words.len())
                .map(|(rule, _)| rule)
        };
        let mut words: Vec<&str> = key.split_whitespace().collect();
        // Interval-1 day lists are stored without the leading "every" ("fri", "mon,wed").
        whole(&words).or_else(|| {
            words.insert(0, "every");
            whole(&words)
        })
    }

    /// The first date after `day` this rule allows.
    fn step(self, day: NaiveDate) -> NaiveDate {
        let plus = |n: u64| day.checked_add_days(Days::new(n)).unwrap_or(day);
        let every = u64::from(self.every);
        match self.unit {
            Unit::Day => plus(every),
            Unit::Week => plus(7 * every),
            Unit::Month => day
                .checked_add_months(Months::new(u32::from(self.every)))
                .unwrap_or(day),
            Unit::On(set) => (1..=7)
                .find(|&n| set.contains(plus(n).weekday()))
                .map_or(day, |n| {
                    // Landing in a new Monday-start week also skips the weeks in between.
                    let wrapped = plus(n).weekday().num_days_from_monday()
                        <= day.weekday().num_days_from_monday();
                    plus(if wrapped { n + 7 * (every - 1) } else { n })
                }),
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

/// Finds a repeat phrase in `input`. Returns the rule and `input` without the phrase.
///
/// A weekday rule leaves the long name of its day that comes soonest after `today` (today is
/// skipped: the date parser never resolves a weekday name to today), so `every friday` becomes
/// `friday` and the date parser still sets the first due date. `every
/// weekday` leaves nothing: a Monday-to-Friday task has no one first day.
///
/// Phrases: `every [N|other] day|week|month` (`week` may end `on <days>`), `every [other] <days>`,
/// `every weekday`, and the bare `daily`, `weekly`, `biweekly`, `monthly`, `weekdays`. `<days>` is
/// weekday names joined by commas, `/`, `&`, `and` or spaces. The bare forms count only at the end
/// (before any `#tag` or `!` words), so "review weekly report" stays a plain task. `None` when
/// there is no phrase; `input` is then untouched.
#[must_use]
pub fn extract(input: &str, today: Weekday) -> Option<(Recurrence, String)> {
    let words: Vec<&str> = input.split_whitespace().collect();
    let (at, used, rule) =
        (0..words.len()).find_map(|i| phrase_at(&words[i..]).map(|(rule, n)| (i, n, rule)))?;
    let mut rest = words[..at].to_vec();
    if let Unit::On(set) = rule.unit
        && set != WORKDAYS
        && let Some(day) = set.iter(today.succ()).next()
    {
        rest.push(names(day).1);
    }
    rest.extend_from_slice(&words[at + used..]);
    Some((rule, rest.join(" ")))
}

/// The phrase at the start of `words`: its rule and word count.
fn phrase_at(words: &[&str]) -> Option<(Recurrence, usize)> {
    let first = words.first()?.to_lowercase();
    if first == "every" {
        let (every, unit, n) = every_phrase(&words[1..])?;
        return Some((Recurrence::new(every, unit)?, n + 1));
    }
    let (every, unit) = match first.as_str() {
        "daily" | "everyday" => (1, Unit::Day),
        "weekly" => (1, Unit::Week),
        "biweekly" => (2, Unit::Week),
        "monthly" => (1, Unit::Month),
        "weekdays" => (1, Unit::On(WORKDAYS)),
        _ => return None,
    };
    let trailing = words[1..]
        .iter()
        .all(|w| w.starts_with('#') || w.starts_with('!'));
    trailing.then_some((Recurrence::new(every, unit)?, 1))
}

/// The interval, unit and word count of what follows `every`.
fn every_phrase(words: &[&str]) -> Option<(u16, Unit, usize)> {
    let first = words.first()?.to_lowercase();
    let numeric = first.starts_with(|c: char| c.is_ascii_digit());
    let (every, at) = match first.as_str() {
        "other" => (2, 1),
        _ if numeric => (first.parse().ok()?, 1),
        _ => (1, 0),
    };
    let (unit, used) = match words.get(at)?.to_lowercase().as_str() {
        "day" | "days" => (Unit::Day, at + 1),
        "month" | "months" => (Unit::Month, at + 1),
        "week" | "weeks" => words[at + 1..]
            .split_first()
            .filter(|(on, _)| on.eq_ignore_ascii_case("on"))
            .and_then(|(_, days)| daylist(days))
            .map_or((Unit::Week, at + 1), |(set, n)| (Unit::On(set), at + 2 + n)),
        "weekday" | "weekdays" if at == 0 => (Unit::On(WORKDAYS), 1),
        // A number before bare days ("every 2 fri") is not a phrase.
        _ if !numeric => {
            let (set, n) = daylist(&words[at..])?;
            (Unit::On(set), at + n)
        }
        _ => return None,
    };
    Some((every, unit, used))
}

/// The weekdays at the start of `words`, joined by commas, `/`, `&`, `and` or spaces, and the
/// word count. A joining word counts only when a day follows it ("fri and buy milk" is just fri).
fn daylist(words: &[&str]) -> Option<(WeekdaySet, usize)> {
    let mut set = WeekdaySet::EMPTY;
    let mut used = 0;
    loop {
        let join = usize::from(used > 0 && words.get(used).is_some_and(|w| is_join(w)));
        let Some(days) = words.get(used + join).and_then(|w| word_days(w)) else {
            break;
        };
        set = set.union(days);
        used += join + 1;
    }
    (used > 0).then_some((set, used))
}

fn is_join(word: &str) -> bool {
    word.eq_ignore_ascii_case("and") || word.chars().all(|c| matches!(c, ',' | '/' | '&'))
}

/// The weekdays in one word: `fri`, `mon,wed`, `tue/thu,`; `None` if any part is not a weekday.
fn word_days(word: &str) -> Option<WeekdaySet> {
    let lower = word.to_lowercase();
    let mut set = WeekdaySet::EMPTY;
    for part in lower.split([',', '/', '&']).filter(|p| !p.is_empty()) {
        set.insert(part.parse().ok()?);
    }
    (!set.is_empty()).then_some(set)
}
