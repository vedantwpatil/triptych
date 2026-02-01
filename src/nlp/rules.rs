use crate::nlp::types::{Event, ParsedItem, Priority, Task};
use chrono::{DateTime, Datelike, Duration, Local, Utc};
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
}

#[derive(Debug, Clone)]
enum TemporalContext {
    /// A resolved point in time (tomorrow, next friday, 5pm)
    Point(DateTime<Utc>),
    /// A resolved duration (for 2 hours)
    Duration(Duration),
    /// A time range (3pm-5pm) - implies both point and duration logic
    Range {
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    },
}

// ============================================================================
// MAIN PARSER
// ============================================================================

pub struct RuleParser;

impl RuleParser {
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

        for segment in segments {
            match segment {
                Segment::Text(t) => title_parts.push(t),
                Segment::Tag(t) => tags.push(t),
                Segment::Priority(p) => priority = p,
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
                    TemporalContext::Range { start, end } => {
                        start_time = Some(start);
                        end_time = Some(end);
                    }
                },
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
            } else {
                // It has a start/due date but no duration, likely a Task
                return Some(ParsedItem::Task(Task {
                    title,
                    due_date: Some(start),
                    tags,
                    priority,
                    is_scheduled: true,
                }));
            }
        }

        // If no time, it's a Task
        // Fallback: If title is empty but we have tags/priority, we still want to parse?
        if title.is_empty() && tags.is_empty() {
            return None;
        }

        Some(ParsedItem::Task(Task {
            title,
            due_date: None,
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
            // 2. Temporal expressions (greedy but structured)
            parse_temporal_segment,
            // 3. Fallback: standard text
            parse_text_segment,
        )),
    ))(input)
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
    // We try various time strategies.
    // Note: We need to pass `now` down for resolution, or use a closure strategy.
    // For simplicity here, we resolve using Local::now() inside the parser map.

    let now = Local::now();

    alt((
        // 1. Complex Phrases ("day after tomorrow", "3pm-5pm")
        map(parse_day_after_tomorrow(now), Segment::Temporal),
        map(parse_time_range(now), Segment::Temporal),
        // 2. Business Terms ("eod", "cob")
        map(parse_business_time(now), Segment::Temporal),
        // 3. Durations ("in 2 hours", "for 30 mins")
        map(parse_relative_duration(now), Segment::Temporal),
        // 4. Chrono-English Delegation (Dates, Weekdays, "tomorrow")
        // We must identify *valid* chrono strings first so we don't feed random title words
        map_res(parse_chrono_candidate, move |s| {
            // We use map_res to return a Result. If chrono fails, nom backtracks!
            match parse_date_string(s, now, Dialect::Us) {
                Ok(dt) => Ok(Segment::Temporal(TemporalContext::Point(
                    dt.with_timezone(&Utc),
                ))),
                Err(_) => Err("chrono parse failed"),
            }
        }),
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
        let dt = target
            .date_naive()
            .and_hms_opt(9, 0, 0)
            .unwrap()
            .and_local_timezone(Local)
            .unwrap();

        Ok((input, TemporalContext::Point(dt.with_timezone(&Utc))))
    }
}

fn parse_time_range(now: DateTime<Local>) -> impl FnMut(&str) -> IResult<&str, TemporalContext> {
    move |input| {
        let (input, (start_h, start_m, start_ampm)) = parse_loose_time(input)?;
        let (input, _) = tuple((multispace0, alt((tag("-"), tag("–"))), multispace0))(input)?;
        let (input, (end_h, end_m, end_ampm)) = parse_loose_time(input)?;

        // Context Inference: "2-4pm" implies "2pm-4pm"
        // If start has no AM/PM, but end does, inherit it?
        // Logic: If start < end (12h), inherit. If start > end (e.g. 11-1pm), start is AM, end is PM.
        // Simplified heuristic: If start has no suffix, use end's suffix.
        let effective_start_ampm = start_ampm.or(end_ampm);

        let s_hour = resolve_24h(start_h, effective_start_ampm);
        let e_hour = resolve_24h(end_h, end_ampm);

        let start_dt = now
            .date_naive()
            .and_hms_opt(s_hour, start_m, 0)
            .unwrap()
            .and_local_timezone(Local)
            .unwrap();

        let end_dt = now
            .date_naive()
            .and_hms_opt(e_hour, end_m, 0)
            .unwrap()
            .and_local_timezone(Local)
            .unwrap();

        Ok((
            input,
            TemporalContext::Range {
                start: start_dt.with_timezone(&Utc),
                end: end_dt.with_timezone(&Utc),
            },
        ))
    }
}

fn parse_business_time(now: DateTime<Local>) -> impl FnMut(&str) -> IResult<&str, TemporalContext> {
    move |input| {
        let (input, token) = alt((
            tag_no_case("eod"),
            tag_no_case("cob"),
            tag_no_case("eow"),
            tag_no_case("eom"),
        ))(input)?;

        let dt = match token.to_lowercase().as_str() {
            "eod" | "cob" => now
                .date_naive()
                .and_hms_opt(17, 0, 0)
                .unwrap()
                .and_local_timezone(Local)
                .unwrap(),
            "eow" => {
                let days_until_fri = (4i64 - now.weekday().num_days_from_monday() as i64 + 7) % 7;
                (now + Duration::days(days_until_fri))
                    .date_naive()
                    .and_hms_opt(17, 0, 0)
                    .unwrap()
                    .and_local_timezone(Local)
                    .unwrap()
            }
            "eom" => {
                // Naive end of month calculation
                let next_month = if now.month() == 12 {
                    now.with_year(now.year() + 1)
                        .unwrap()
                        .with_month(1)
                        .unwrap()
                        .with_day(1)
                        .unwrap()
                } else {
                    now.with_month(now.month() + 1)
                        .unwrap()
                        .with_day(1)
                        .unwrap()
                };
                (next_month - Duration::days(1))
                    .date_naive()
                    .and_hms_opt(17, 0, 0)
                    .unwrap()
                    .and_local_timezone(Local)
                    .unwrap()
            }
            _ => unreachable!(),
        };

        Ok((input, TemporalContext::Point(dt.with_timezone(&Utc))))
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

/// Recognizes strings that look like dates to prevent greedy text parsing
/// e.g. "tomorrow", "next monday", "jan 5"
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
        // 3. Absolute dates (tuple returns complex type, must squash to &str)
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
        // 4. Explicit time (preceded returns complex type, must squash)
        recognize(preceded(
            pair(tag_no_case("at"), space1),
            parse_loose_time, // parse_loose_time returns a tuple, recognize fixes it
        )),
    ))(input)
}

// Helpers
fn parse_loose_time(input: &str) -> IResult<&str, (u32, u32, Option<bool>)> {
    let (input, hour) = map_res(digit1, |s: &str| s.parse::<u32>())(input)?;
    let (input, minute) = opt(preceded(
        char(':'),
        map_res(digit1, |s: &str| s.parse::<u32>()),
    ))(input)?;
    let (input, _) = multispace0(input)?;
    let (input, am_pm) = opt(alt((tag_no_case("am"), tag_no_case("pm"))))(input)?;

    let is_pm = am_pm.map(|s| s.to_lowercase() == "pm");
    Ok((input, (hour, minute.unwrap_or(0), is_pm)))
}

fn resolve_24h(hour: u32, is_pm: Option<bool>) -> u32 {
    match (hour, is_pm) {
        (12, Some(true)) => 12, // 12 pm is noon
        (12, Some(false)) => 0, // 12 am is midnight
        (h, Some(true)) => h + 12,
        (h, Some(false)) => h,
        (h, None) => h, // Assume 24h if no am/pm
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
