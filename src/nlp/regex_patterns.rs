use crate::nlp::types::{Event, ParsedItem, Priority, Task};
use chrono::{DateTime, Datelike, Duration, Local, TimeZone, Utc};
use once_cell::sync::Lazy;
use regex::Regex;

// Compile regex patterns once at startup
static TOMORROW_TIME: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)tomorrow\s+(?:at\s+)?(\d{1,2})(?::(\d{2}))?\s*(am|pm)?").unwrap()
});

static TODAY_TIME: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)today\s+(?:at\s+)?(\d{1,2})(?::(\d{2}))?\s*(am|pm)?").unwrap());

static NEXT_WEEK_DAY: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)next\s+(monday|tuesday|wednesday|thursday|friday|saturday|sunday)").unwrap()
});

static SPECIFIC_TIME: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(?:at\s+)?(\d{1,2})(?::(\d{2}))?\s*(am|pm)").unwrap());

static TAG_PATTERN: Lazy<Regex> = Lazy::new(|| Regex::new(r"#(\w+)").unwrap());

static PRIORITY_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)(!{1,3}|priority:\s*(low|medium|high|urgent))").unwrap());

pub struct RegexParser;

impl RegexParser {
    pub fn try_parse(input: &str) -> Option<ParsedItem> {
        // Try parsing as task with temporal info
        if let Some(task) = Self::parse_task(input) {
            return Some(ParsedItem::Task(task));
        }

        // Try parsing as event
        if let Some(event) = Self::parse_event(input) {
            return Some(ParsedItem::Event(event));
        }

        None
    }

    fn parse_task(input: &str) -> Option<Task> {
        // Extract temporal information
        let due_date = Self::extract_datetime(input);

        // Extract tags
        let tags: Vec<String> = TAG_PATTERN
            .captures_iter(input)
            .map(|cap| cap[1].to_string())
            .collect();

        // Extract priority
        let priority = Self::extract_priority(input);

        // Clean title by removing temporal markers and tags
        let title = Self::clean_title(input);

        // Must have some temporal marker for regex fast path
        if due_date.is_none() && tags.is_empty() {
            return None;
        }

        Some(Task {
            title,
            due_date,
            tags,
            priority,
            is_scheduled: due_date.is_some(),
        })
    }

    fn parse_event(input: &str) -> Option<Event> {
        // Events must have explicit time
        let start_time = Self::extract_datetime(input)?;

        // Extract tags
        let tags: Vec<String> = TAG_PATTERN
            .captures_iter(input)
            .map(|cap| cap[1].to_string())
            .collect();

        let title = Self::clean_title(input);

        Some(Event {
            title,
            start_time,
            end_time: None, // Can be enhanced with duration parsing
            location: None,
            tags,
        })
    }

    fn extract_datetime(input: &str) -> Option<DateTime<Utc>> {
        let now = Local::now();

        // Try "tomorrow at 3pm" pattern
        if let Some(caps) = TOMORROW_TIME.captures(input) {
            let hour = caps.get(1)?.as_str().parse::<u32>().ok()?;
            let minute = caps
                .get(2)
                .and_then(|m| m.as_str().parse::<u32>().ok())
                .unwrap_or(0);
            let is_pm = caps
                .get(3)
                .map(|s| s.as_str().to_lowercase() == "pm")
                .unwrap_or(false);

            let adjusted_hour = if is_pm && hour != 12 { hour + 12 } else { hour };

            let tomorrow = now + Duration::days(1);
            return Local
                .with_ymd_and_hms(
                    tomorrow.year(),
                    tomorrow.month(),
                    tomorrow.day(),
                    adjusted_hour,
                    minute,
                    0,
                )
                .single()
                .map(|dt| dt.with_timezone(&Utc));
        }

        // Try "today at 3pm" pattern
        if let Some(caps) = TODAY_TIME.captures(input) {
            let hour = caps.get(1)?.as_str().parse::<u32>().ok()?;
            let minute = caps
                .get(2)
                .and_then(|m| m.as_str().parse::<u32>().ok())
                .unwrap_or(0);
            let is_pm = caps
                .get(3)
                .map(|s| s.as_str().to_lowercase() == "pm")
                .unwrap_or(false);

            let adjusted_hour = if is_pm && hour != 12 { hour + 12 } else { hour };

            return Local
                .with_ymd_and_hms(now.year(), now.month(), now.day(), adjusted_hour, minute, 0)
                .single()
                .map(|dt| dt.with_timezone(&Utc));
        }

        // Try "next Monday" pattern
        if let Some(caps) = NEXT_WEEK_DAY.captures(input) {
            let target_day = caps.get(1)?.as_str();
            let days_ahead = Self::days_until_next_weekday(target_day)?;
            let target_date = now + Duration::days(days_ahead);

            return Local
                .with_ymd_and_hms(
                    target_date.year(),
                    target_date.month(),
                    target_date.day(),
                    9, // Default to 9 AM
                    0,
                    0,
                )
                .single()
                .map(|dt| dt.with_timezone(&Utc));
        }

        None
    }

    fn days_until_next_weekday(day: &str) -> Option<i64> {
        let target = match day.to_lowercase().as_str() {
            "monday" => 0,
            "tuesday" => 1,
            "wednesday" => 2,
            "thursday" => 3,
            "friday" => 4,
            "saturday" => 5,
            "sunday" => 6,
            _ => return None,
        };

        let now = Local::now();
        let current = now.weekday().num_days_from_monday() as i64;
        let days = (target - current + 7) % 7;
        Some(if days == 0 { 7 } else { days })
    }

    fn extract_priority(input: &str) -> Priority {
        if let Some(caps) = PRIORITY_PATTERN.captures(input) {
            if let Some(exclamation) = caps.get(1) {
                let exc = exclamation.as_str();
                return match exc.len() {
                    3 => Priority::Urgent,
                    2 => Priority::High,
                    1 => Priority::Medium,
                    _ => Priority::Low,
                };
            }

            if let Some(priority_word) = caps.get(2) {
                return match priority_word.as_str().to_lowercase().as_str() {
                    "urgent" => Priority::Urgent,
                    "high" => Priority::High,
                    "medium" => Priority::Medium,
                    _ => Priority::Low,
                };
            }
        }
        Priority::Medium
    }

    fn clean_title(input: &str) -> String {
        let mut cleaned = input.to_string();

        // Remove temporal markers
        cleaned = TOMORROW_TIME.replace_all(&cleaned, "").to_string();
        cleaned = TODAY_TIME.replace_all(&cleaned, "").to_string();
        cleaned = NEXT_WEEK_DAY.replace_all(&cleaned, "").to_string();
        cleaned = SPECIFIC_TIME.replace_all(&cleaned, "").to_string();

        // Remove tags
        cleaned = TAG_PATTERN.replace_all(&cleaned, "").to_string();

        // Remove priority markers
        cleaned = PRIORITY_PATTERN.replace_all(&cleaned, "").to_string();

        // Clean up whitespace
        cleaned
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tomorrow_parsing() {
        let result = RegexParser::try_parse("Submit report tomorrow at 3pm #work");
        assert!(result.is_some());

        if let Some(ParsedItem::Task(task)) = result {
            assert_eq!(task.title, "Submit report");
            assert!(task.due_date.is_some());
            assert_eq!(task.tags, vec!["work"]);
        }
    }

    #[test]
    fn test_priority_parsing() {
        let result = RegexParser::try_parse("Fix bug today at 2pm !!!");
        assert!(result.is_some());

        if let Some(ParsedItem::Task(task)) = result {
            assert_eq!(task.priority, Priority::Urgent);
        }
    }
}
