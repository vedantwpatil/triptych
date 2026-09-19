use crate::app::resolve_local_datetime;
use crate::nlp::types::{Event, ParsedItem, Priority, Task};
use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveTime, Utc};
use chrono_english::{Dialect, parse_date_string};
use nom::{
    IResult,
    branch::alt,
    bytes::complete::{tag, tag_no_case, take_while1},
    character::complete::{char, digit1, multispace0, multispace1, space1},
    combinator::{map, map_res, opt, recognize, value},
    multi::many0,
    sequence::{pair, preceded, tuple},
};

/// Converts a `None` from a calendar `Option` (`and_hms_opt`, `with_day`, etc.) that's
/// provably `Some` by construction (values already in-range) into a nom parse failure
/// instead of panicking, so a future edit that breaks the invariant fails the parse
/// rather than crashing the process.
fn require<T>(opt: Option<T>, input: &str) -> Result<T, nom::Err<nom::error::Error<&str>>> {
    opt.ok_or_else(|| {
        nom::Err::Failure(nom::error::Error::new(input, nom::error::ErrorKind::Verify))
    })
}

// ============================================================================
// DATA STRUCTURES
// ============================================================================

#[derive(Debug, Clone)]
enum Segment {
    /// A meaningful piece of text for the title
    Text(String),
    /// A parsed temporal value (absolute or relative)
    Temporal(TemporalContext),
    /// A parsed tag (#work)
    Tag(String),
    /// A parsed priority marker (!, priority:high)
    Priority(Priority),
    /// A hard deadline ("by Friday", "due tomorrow")
    Deadline(DateTime<Utc>),
    /// An explicit task duration ("2h", "90m", "3 hours")
    ExplicitDuration(i32),
    /// A date with no time of day ("tomorrow", "next friday", "sep 25", "12/25")
    Date(DateTime<Utc>),
    /// A time of day with no date ("at 3pm", "6pm", "15:30")
    TimeOfDay(NaiveTime),
    /// A time-of-day range ("3pm-5pm")
    TimeRange(NaiveTime, NaiveTime),
}

#[derive(Debug, Clone)]
pub enum TemporalContext {
    /// A resolved point in time (eod, in 2 hours)
    Point(DateTime<Utc>),
    /// A resolved duration (for 3 days)
    Duration(Duration),
}

// ============================================================================
// MAIN PARSER
// ============================================================================

#[derive(Debug)]
pub struct RuleParser;

/// True if a deadline-intent word ("by"/"due"/"before") was left unresolved.
///
/// The word must appear as a standalone token that `parse_deadline_segment` could not turn into an
/// actual deadline (e.g. "before the end of next month"). The parser uses this to fall through to
/// Ollama instead of silently dropping the user's intended deadline.
#[must_use]
pub fn has_unresolved_deadline_intent(input: &str, resolved_deadline: bool) -> bool {
    if resolved_deadline {
        return false;
    }
    input
        .split_whitespace()
        .any(|w| matches!(w.to_lowercase().as_str(), "by" | "due" | "before"))
}

impl RuleParser {
    #[must_use]
    pub fn try_parse(input: &str) -> Option<ParsedItem> {
        let (remaining, segments) = parse_segments(input).ok()?;

        // If the parser didn't consume everything meaningful (unlikely with this architecture),
        // we append the rest to the title.
        let mut final_segments = segments;
        if !remaining.trim().is_empty() {
            final_segments.push(Segment::Text(remaining.trim().to_string()));
        }

        Self::assemble(final_segments)
    }

    fn assemble(segments: Vec<Segment>) -> Option<ParsedItem> {
        let mut title_parts = Vec::new();
        let mut tags = Vec::new();
        let mut priority = Priority::Medium;

        // Temporal assembly state
        let mut start_time: Option<DateTime<Utc>> = None;
        let mut end_time: Option<DateTime<Utc>> = None;
        let mut duration: Option<Duration> = None;
        let mut deadline: Option<DateTime<Utc>> = None;
        let mut explicit_duration_minutes: Option<i32> = None;
        let mut date: Option<DateTime<Utc>> = None;
        let mut time_of_day: Option<NaiveTime> = None;
        let mut time_range: Option<(NaiveTime, NaiveTime)> = None;

        for segment in segments {
            match segment {
                Segment::Text(t) => title_parts.push(t),
                Segment::Tag(t) => tags.push(t),
                Segment::Priority(p) => priority = p,
                Segment::Deadline(dt) => deadline = Some(dt),
                Segment::ExplicitDuration(mins) => explicit_duration_minutes = Some(mins),
                Segment::Date(dt) => date = Some(dt),
                Segment::TimeOfDay(t) => time_of_day = Some(t),
                Segment::TimeRange(from, to) => time_range = Some((from, to)),
                Segment::Temporal(temp) => match temp {
                    TemporalContext::Point(dt) => {
                        // If we already have a start time, maybe this is end time?
                        // For now, simpler logic: Last explicit date wins, or first?
                        // Let's say first explicit date is start.
                        if start_time.is_none() {
                            start_time = Some(dt);
                        } else {
                            // If we have two points, assume start -> end
                            end_time = Some(dt);
                        }
                    }
                    TemporalContext::Duration(d) => duration = Some(d),
                },
            }
        }

        // Merge a date and a time of day into one moment; a time with no date means today.
        if start_time.is_none() {
            let day = date.map_or_else(
                || Local::now().date_naive(),
                |dt| dt.with_timezone(&Local).date_naive(),
            );
            let at = |t: NaiveTime| resolve_local_datetime(day.and_time(t));
            if let Some((from, to)) = time_range {
                start_time = Some(at(from));
                end_time = Some(at(to));
            } else if let Some(t) = time_of_day {
                start_time = Some(at(t));
            } else {
                start_time = date;
            }
        }

        let title = title_parts.join(" ");

        // Logic to distinguish Task vs Event
        // Events need a clear Start AND (End or Duration)
        if let Some(start) = start_time {
            // Check for explicit end time or duration
            let calculated_end = end_time.or_else(|| duration.map(|d| start + d));

            if let Some(end) = calculated_end {
                // It has start and end, likely an Event
                return Some(ParsedItem::Event(Event {
                    title,
                    start_time: start,
                    end_time: Some(end),
                    location: None,
                    tags,
                }));
            }
            // It has a start/due date but no duration, likely a Task
            return Some(ParsedItem::Task(Task {
                title,
                due_date: Some(start),
                deadline,
                duration_minutes: explicit_duration_minutes,
                tags,
                priority,
                is_scheduled: true,
            }));
        }

        // If no time, it's a Task
        // Fallback: nothing worth keeping unless a deadline/duration was found
        if title.is_empty()
            && tags.is_empty()
            && deadline.is_none()
            && explicit_duration_minutes.is_none()
        {
            return None;
        }

        Some(ParsedItem::Task(Task {
            title,
            due_date: None,
            deadline,
            duration_minutes: explicit_duration_minutes,
            tags,
            priority,
            is_scheduled: false,
        }))
    }
}

// ============================================================================
// SEGMENT PARSERS (The "One Pass" Loop)
// ============================================================================

fn parse_segments(input: &str) -> IResult<&str, Vec<Segment>> {
    many0(preceded(
        multispace0,
        alt((
            // Order is critical here.
            // 1. Tags and Priority (unambiguous syntax)
            parse_tag_segment,
            parse_priority_segment,
            // 2. Deadline and explicit duration (unambiguous prefixes/suffixes)
            parse_deadline_segment,
            parse_bare_duration_segment,
            // 3. Temporal expressions (greedy but structured)
            parse_temporal_segment,
            // 4. Fallback: standard text
            parse_text_segment,
        )),
    ))(input)
}

/// Matches "by Friday", "due tomorrow", "before wed"
fn parse_deadline_segment(input: &str) -> IResult<&str, Segment> {
    let now = Local::now();
    let (input, _) = alt((tag_no_case("by"), tag_no_case("due"), tag_no_case("before")))(input)?;
    let (input, _) = space1(input)?;
    map_res(
        take_while1(|c: char| c.is_alphabetic()),
        move |word: &str| -> Result<Segment, &'static str> {
            let today = now.date_naive();
            let target_date = match word.to_lowercase().as_str() {
                "today" => today,
                "tomorrow" => today + Duration::days(1),
                _ => {
                    let target_weekday =
                        weekday_from_name(word).ok_or("not a recognized deadline target")?;
                    let days_ahead = (i64::from(target_weekday.num_days_from_monday())
                        - i64::from(today.weekday().num_days_from_monday())
                        + 7)
                        % 7;
                    today + Duration::days(days_ahead)
                }
            };

            let end_of_day = target_date.and_hms_opt(23, 59, 59).ok_or("invalid time")?;

            Ok(Segment::Deadline(crate::app::resolve_local_datetime(
                end_of_day,
            )))
        },
    )(input)
}

fn weekday_from_name(name: &str) -> Option<chrono::Weekday> {
    use chrono::Weekday::{Fri, Mon, Sat, Sun, Thu, Tue, Wed};
    Some(match name.to_lowercase().as_str() {
        "monday" | "mon" => Mon,
        "tuesday" | "tue" | "tues" => Tue,
        "wednesday" | "wed" => Wed,
        "thursday" | "thu" | "thurs" => Thu,
        "friday" | "fri" => Fri,
        "saturday" | "sat" => Sat,
        "sunday" | "sun" => Sun,
        _ => return None,
    })
}

/// Matches bare durations with an optional "for" prefix: "2h", "90m", "3 hours", "for 30m"
fn parse_bare_duration_segment(input: &str) -> IResult<&str, Segment> {
    let (input, _) = opt(pair(tag_no_case("for"), space1))(input)?;
    let (input, amount) = map_res(digit1, |s: &str| s.parse::<i64>())(input)?;
    let (input, _) = multispace0(input)?;
    let (input, unit) = alt((
        tag_no_case("hours"),
        tag_no_case("hour"),
        tag_no_case("hrs"),
        tag_no_case("hr"),
        tag_no_case("h"),
        tag_no_case("minutes"),
        tag_no_case("minute"),
        tag_no_case("mins"),
        tag_no_case("min"),
        tag_no_case("m"),
    ))(input)?;
    // Require a word boundary so "3 more" doesn't get eaten as "3 m[ore]"
    let (input, ()) = word_boundary(input)?;

    let minutes = if unit.to_lowercase().starts_with('h') {
        amount * 60
    } else {
        amount
    };

    Ok((
        input,
        Segment::ExplicitDuration(i32::try_from(minutes).unwrap_or(i32::MAX)),
    ))
}

/// A recoverable parse failure: `alt`/`many0` backtrack past it (unlike `nom::Err::Failure`).
fn reject<T>(input: &str) -> IResult<&str, T> {
    Err(nom::Err::Error(nom::error::Error::new(
        input,
        nom::error::ErrorKind::Verify,
    )))
}

/// Succeeds only if the next character isn't alphanumeric (or input is exhausted)
fn word_boundary(input: &str) -> IResult<&str, ()> {
    match input.chars().next() {
        Some(c) if c.is_alphanumeric() => reject(input),
        _ => Ok((input, ())),
    }
}

fn parse_tag_segment(input: &str) -> IResult<&str, Segment> {
    map(
        preceded(
            char('#'),
            take_while1(|c: char| c.is_alphanumeric() || c == '_' || c == '-'),
        ),
        |s: &str| Segment::Tag(s.to_string()),
    )(input)
}

fn parse_priority_segment(input: &str) -> IResult<&str, Segment> {
    let bang_priority = alt((
        value(Priority::Urgent, tag("!!!")),
        value(Priority::High, tag("!!")),
        value(Priority::Medium, tag("!")),
    ));

    let named_priority = preceded(
        tuple((tag_no_case("priority"), opt(char(':')), multispace0)),
        alt((
            value(Priority::Urgent, tag_no_case("urgent")),
            value(Priority::High, tag_no_case("high")),
            value(Priority::Medium, tag_no_case("medium")),
            value(Priority::Low, tag_no_case("low")),
        )),
    );

    map(alt((bang_priority, named_priority)), Segment::Priority)(input)
}

fn parse_text_segment(input: &str) -> IResult<&str, Segment> {
    // Consume until we hit whitespace or start of a special char (though special chars are handled by main loop alt)
    // Actually, we just take the next word. The loop `preceded(multispace0, ...)` handles the spacing.
    map(take_while1(|c: char| !c.is_whitespace()), |s: &str| {
        Segment::Text(s.to_string())
    })(input)
}

// ============================================================================
// TEMPORAL PARSERS (The Complex Logic)
// ============================================================================

fn parse_temporal_segment(input: &str) -> IResult<&str, Segment> {
    // Resolved against Local::now() at parse time.
    let now = Local::now();

    alt((
        // 1. Complex phrases ("day after tomorrow", "3pm-5pm")
        map(parse_day_after_tomorrow(now), Segment::Temporal),
        parse_time_range,
        // 2. Business terms ("eod", "cob")
        map(parse_business_time(now), Segment::Temporal),
        // 3. Durations ("in 2 hours", "for 3 days")
        map(parse_relative_duration(now), Segment::Temporal),
        // 4. Dates ("tomorrow", "on sep 25", "12/25") and times of day ("at 3pm", "15:30"),
        // kept apart so `assemble` can merge "tomorrow at 3pm" into one moment.
        parse_date_segment(now),
        parse_time_of_day_segment,
    ))(input)
}

/// Matches "day after tomorrow" specifically
fn parse_day_after_tomorrow(
    now: DateTime<Local>,
) -> impl FnMut(&str) -> IResult<&str, TemporalContext> {
    move |input| {
        let (input, _) = tuple((
            tag_no_case("day"),
            multispace1,
            tag_no_case("after"),
            multispace1,
            tag_no_case("tomorrow"),
        ))(input)?;

        let target = now + Duration::days(2);
        // Default to 9am
        let dt = resolve_local_datetime(require(target.date_naive().and_hms_opt(9, 0, 0), input)?);

        Ok((input, TemporalContext::Point(dt)))
    }
}

/// "3pm-5pm", "2-4pm" (the start inherits the end's am/pm), "15:00-17:00". Bare "5-7" is not a
/// range, so page or room numbers stay in the title.
fn parse_time_range(input: &str) -> IResult<&str, Segment> {
    let (rest, start) = parse_loose_time(input)?;
    let (rest, _) = tuple((multispace0, alt((tag("-"), tag("–"))), multispace0))(rest)?;
    let (rest, end) = parse_loose_time(rest)?;
    let (rest, ()) = word_boundary(rest)?;

    if start.pm.is_none() && end.pm.is_none() && !(start.colon && end.colon) {
        return reject(input);
    }
    match (start.to_time(end.pm), end.to_time(None)) {
        (Some(from), Some(to)) => Ok((rest, Segment::TimeRange(from, to))),
        _ => reject(input),
    }
}

/// "at 3pm", "6pm", "at 15:30". A bare number ("look at 5 things") is not a time.
fn parse_time_of_day_segment(input: &str) -> IResult<&str, Segment> {
    let (rest, _) = opt(pair(tag_no_case("at"), space1))(input)?;
    let (rest, time) = parse_loose_time(rest)?;
    let (rest, ()) = word_boundary(rest)?;

    match time.to_time(None) {
        Some(t) if time.pm.is_some() || time.colon => Ok((rest, Segment::TimeOfDay(t))),
        _ => reject(input),
    }
}

pub fn parse_business_time(
    now: DateTime<Local>,
) -> impl FnMut(&str) -> IResult<&str, TemporalContext> {
    move |input| {
        let (input, token) = alt((
            tag_no_case("eod"),
            tag_no_case("cob"),
            tag_no_case("eow"),
            tag_no_case("eom"),
        ))(input)?;

        let dt = match token.to_lowercase().as_str() {
            "eod" | "cob" => {
                resolve_local_datetime(require(now.date_naive().and_hms_opt(17, 0, 0), input)?)
            }
            "eow" => {
                let days_until_fri =
                    (4i64 - i64::from(now.weekday().num_days_from_monday()) + 7) % 7;
                let naive = require(
                    (now + Duration::days(days_until_fri))
                        .date_naive()
                        .and_hms_opt(17, 0, 0),
                    input,
                )?;
                resolve_local_datetime(naive)
            }
            "eom" => {
                // Reset the day to 1 first (always valid in every month) before
                // changing month/year, so a 31st never gets carried into a
                // shorter target month - e.g. Jan 31 -> with_month(2) would
                // compute "Feb 31", which doesn't exist and returns `None`.
                let first_of_this_month = require(now.with_day(1), input)?;
                let first_of_next_month = if now.month() == 12 {
                    let next_year = require(first_of_this_month.with_year(now.year() + 1), input)?;
                    require(next_year.with_month(1), input)?
                } else {
                    require(first_of_this_month.with_month(now.month() + 1), input)?
                };
                let naive = require(
                    (first_of_next_month - Duration::days(1))
                        .date_naive()
                        .and_hms_opt(17, 0, 0),
                    input,
                )?;
                resolve_local_datetime(naive)
            }
            // Unreachable by construction: `token` can only be "eod"/"cob"/"eow"/"eom",
            // the exact set `alt()` above matched — a real parse failure, not a panic,
            // if that invariant is ever broken by a future edit.
            _ => {
                return Err(nom::Err::Failure(nom::error::Error::new(
                    input,
                    nom::error::ErrorKind::Verify,
                )));
            }
        };

        Ok((input, TemporalContext::Point(dt)))
    }
}

/// Matches "in X mins", "for X hours"
fn parse_relative_duration(
    now: DateTime<Local>,
) -> impl FnMut(&str) -> IResult<&str, TemporalContext> {
    move |input| {
        let (input, prefix) = alt((tag_no_case("in"), tag_no_case("for")))(input)?;
        let (input, _) = space1(input)?;
        let (input, amount) = map_res(digit1, |s: &str| s.parse::<i64>())(input)?;
        let (input, _) = space1(input)?;
        let (input, unit) = alt((
            tag_no_case("minutes"),
            tag_no_case("mins"),
            tag_no_case("min"),
            tag_no_case("hours"),
            tag_no_case("hrs"),
            tag_no_case("hour"),
            tag_no_case("days"),
            tag_no_case("day"),
        ))(input)?;

        let dur = match unit.to_lowercase().as_str() {
            u if u.starts_with("min") => Duration::minutes(amount),
            u if u.starts_with("hour") || u.starts_with("hr") => Duration::hours(amount),
            u if u.starts_with("day") => Duration::days(amount),
            _ => Duration::seconds(0),
        };

        if prefix.to_lowercase() == "for" {
            Ok((input, TemporalContext::Duration(dur)))
        } else {
            // "in" implies a Deadline which usually means "next block".
            let target_time = (now + dur).with_timezone(&Utc);

            // Apply 15-minute quantization
            let quantized = quantize_time(target_time, 15);

            Ok((input, TemporalContext::Point(quantized)))
        }
    }
}

/// "tomorrow", "next friday", "sep 25", "12/25", each with an optional "on " in front.
fn parse_date_segment(now: DateTime<Local>) -> impl FnMut(&str) -> IResult<&str, Segment> {
    move |input| {
        let (rest, on) = opt(pair(tag_no_case("on"), space1))(input)?;
        let (rest, date) = alt((
            map_res(parse_chrono_candidate, |s| {
                parse_date_string(s, now, Dialect::Us)
                    .map(|dt| dt.with_timezone(&Utc))
                    .map_err(|_| "chrono parse failed")
            }),
            parse_numeric_date(now, on.is_some()),
        ))(rest)?;
        let (rest, ()) = word_boundary(rest)?;
        Ok((rest, Segment::Date(date)))
    }
}

/// US-style "12/25" or "12/25/2026".
///
/// A year-less date the calendar has already passed rolls to next year. "1/2" or "3/4" is far
/// likelier a fraction than a date, so a bare month/day needs a leading "on" or a day above 12.
pub fn parse_numeric_date(
    now: DateTime<Local>,
    has_on: bool,
) -> impl FnMut(&str) -> IResult<&str, DateTime<Utc>> {
    move |input| {
        let number = |i| map_res(digit1, str::parse::<u32>)(i);
        let (rest, month) = number(input)?;
        let (rest, _) = char('/')(rest)?;
        let (rest, day) = number(rest)?;
        let (rest, year) = opt(preceded(char('/'), number))(rest)?;

        if year.is_none() && !has_on && day <= 12 {
            return reject(input);
        }
        let today = now.date_naive();
        let full_year = year.map_or_else(
            || today.year(),
            |y| i32::try_from(if y < 100 { 2000 + y } else { y }).unwrap_or(0),
        );
        let midnight = NaiveDate::from_ymd_opt(full_year, month, day)
            .map(|d| {
                if year.is_none() && d < today {
                    d.with_year(d.year() + 1).unwrap_or(d)
                } else {
                    d
                }
            })
            .and_then(|d| d.and_hms_opt(0, 0, 0));
        midnight.map_or_else(|| reject(input), |m| Ok((rest, resolve_local_datetime(m))))
    }
}

/// Recognizes date phrases to prevent greedy text parsing: "tomorrow", "next monday", "jan 5"
fn parse_chrono_candidate(input: &str) -> IResult<&str, &str> {
    // Helper parsers to avoid the 21-tuple limit
    let parse_month_full = alt((
        tag_no_case("january"),
        tag_no_case("february"),
        tag_no_case("march"),
        tag_no_case("april"),
        tag_no_case("may"),
        tag_no_case("june"),
        tag_no_case("july"),
        tag_no_case("august"),
        tag_no_case("september"),
        tag_no_case("october"),
        tag_no_case("november"),
        tag_no_case("december"),
    ));

    let parse_month_abbr = alt((
        tag_no_case("jan"),
        tag_no_case("feb"),
        tag_no_case("mar"),
        tag_no_case("apr"),
        tag_no_case("jun"),
        tag_no_case("jul"),
        tag_no_case("aug"),
        tag_no_case("sep"),
        tag_no_case("oct"),
        tag_no_case("nov"),
        tag_no_case("dec"),
    ));

    // The Main Alt
    alt((
        // 1. Simple keywords (already return &str)
        alt((
            tag_no_case("tomorrow"),
            tag_no_case("today"),
            tag_no_case("yesterday"),
        )),
        // 2. Relative days (tuple returns complex type, must squash to &str)
        recognize(tuple((
            alt((
                tag_no_case("next"),
                tag_no_case("last"),
                tag_no_case("this"),
            )),
            space1,
            take_while1(|c: char| c.is_alphabetic()),
        ))),
        // 3. Bare weekday ("on sunday", "friday at 3pm"); full names only, "sat"/"sun" are words
        alt((
            tag_no_case("monday"),
            tag_no_case("tuesday"),
            tag_no_case("wednesday"),
            tag_no_case("thursday"),
            tag_no_case("friday"),
            tag_no_case("saturday"),
            tag_no_case("sunday"),
        )),
        // 4. Absolute dates (tuple returns complex type, must squash to &str)
        recognize(tuple((
            alt((parse_month_full, parse_month_abbr)),
            space1,
            digit1,
            // Optional suffixes
            opt(alt((
                tag_no_case("st"),
                tag_no_case("nd"),
                tag_no_case("rd"),
                tag_no_case("th"),
            ))),
        ))),
    ))(input)
}

// Helpers

/// A clock time as typed, before deciding whether it is really a time: "3", "3pm", "3:30", "15:30".
struct LooseTime {
    hour: u32,
    minute: u32,
    pm: Option<bool>,
    colon: bool,
}

impl LooseTime {
    /// 24h `NaiveTime`, or `None` if out of range. `inherited_pm` supplies a missing am/pm.
    fn to_time(&self, inherited_pm: Option<bool>) -> Option<NaiveTime> {
        let pm = self.pm.or(inherited_pm);
        if pm.is_some() && !(1..=12).contains(&self.hour) {
            return None;
        }
        NaiveTime::from_hms_opt(resolve_24h(self.hour, pm), self.minute, 0)
    }
}

fn parse_loose_time(input: &str) -> IResult<&str, LooseTime> {
    let (input, hour) = map_res(digit1, |s: &str| s.parse::<u32>())(input)?;
    let (input, minute) = opt(preceded(
        char(':'),
        map_res(digit1, |s: &str| s.parse::<u32>()),
    ))(input)?;
    let (input, _) = multispace0(input)?;
    let (input, am_pm) = opt(alt((tag_no_case("am"), tag_no_case("pm"))))(input)?;

    let time = LooseTime {
        hour,
        minute: minute.unwrap_or(0),
        pm: am_pm.map(|s| s.eq_ignore_ascii_case("pm")),
        colon: minute.is_some(),
    };
    Ok((input, time))
}

const fn resolve_24h(hour: u32, is_pm: Option<bool>) -> u32 {
    match (hour, is_pm) {
        (12, Some(true)) => 12, // 12 pm is noon
        (12, Some(false)) => 0, // 12 am is midnight
        (h, Some(true)) => h + 12,
        // No am/pm suffix (`None`) is assumed already 24h.
        (h, Some(false) | None) => h,
    }
}

fn quantize_time(dt: DateTime<Utc>, grid_minutes: i64) -> DateTime<Utc> {
    let seconds = dt.timestamp();
    let grid_seconds = grid_minutes * 60;

    // Round up to the next grid slot
    let remainder = seconds % grid_seconds;
    if remainder == 0 {
        dt
    } else {
        let diff = grid_seconds - remainder;
        dt + Duration::seconds(diff)
    }
}
