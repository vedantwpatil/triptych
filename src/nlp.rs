use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, Timelike, Utc};
use regex::Regex;

pub struct TaskParser {
    time_regex: Regex,
    tag_regex: Regex,
    priority_regex: Regex,
}

impl TaskParser {
    pub fn new() -> Self {
        Self {
            time_regex: Regex::new(r"(?i)\b(tomorrow|today|monday|tuesday|wednesday|thursday|friday|saturday|sunday|\d{1,2}/\d{1,2}|\d{1,2}:\d{2}(?:\s*[ap]m)?)\b").unwrap(),
            tag_regex: Regex::new(r"#(\w+)").unwrap(),
            priority_regex: Regex::new(r"(?i)\b(urgent|high|medium|low|priority:high|priority:medium|priority:low|\*{1,3}|!{1,3})\b").unwrap(),
        }
    }

    pub fn parse(&self, input: &str) -> ParsedTask {
        let mut task = ParsedTask {
            description: input.to_string(),
            scheduled_at: None,
            priority: 0,
            tags: Vec::new(),
        };

        // Custom date parsing - more reliable than chrono-english
        if let Some(time_match) = self.time_regex.find(input) {
            let time_str = time_match.as_str().to_lowercase();
            let now = Local::now();

            task.scheduled_at = match time_str.as_str() {
                "today" => Some(now.with_timezone(&Utc)),
                "tomorrow" => Some((now + Duration::days(1)).with_timezone(&Utc)),
                "monday" => Some(self.next_weekday(now, 0)),
                "tuesday" => Some(self.next_weekday(now, 1)),
                "wednesday" => Some(self.next_weekday(now, 2)),
                "thursday" => Some(self.next_weekday(now, 3)),
                "friday" => Some(self.next_weekday(now, 4)),
                "saturday" => Some(self.next_weekday(now, 5)),
                "sunday" => Some(self.next_weekday(now, 6)),
                _ => {
                    // Try to parse MM/DD format
                    if let Some(captures) = Regex::new(r"(\d{1,2})/(\d{1,2})")
                        .unwrap()
                        .captures(&time_str)
                    {
                        if let (Ok(month), Ok(day)) =
                            (captures[1].parse::<u32>(), captures[2].parse::<u32>())
                        {
                            let year = now.year() as i32;
                            if let Some(naive_date) = NaiveDate::from_ymd_opt(year, month, day) {
                                // Use unwrap_or to handle the Option instead of ?
                                if let Some(datetime) = naive_date.and_hms_opt(12, 0, 0) {
                                    Some(
                                        datetime
                                            .and_local_timezone(Local)
                                            .unwrap()
                                            .with_timezone(&Utc),
                                    )
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
            };

            if task.scheduled_at.is_some() {
                task.description = input.replace(time_match.as_str(), "").trim().to_string();
            }
        }

        // Extract priority
        if let Some(priority_match) = self.priority_regex.find(&task.description) {
            task.priority = match priority_match.as_str().to_lowercase().as_str() {
                "urgent" | "priority:high" | "***" | "!!!" => 3,
                "high" | "priority:medium" | "**" | "!!" => 2,
                "medium" | "priority:low" | "*" | "!" => 1,
                "low" => 0,
                _ => 0,
            };
            task.description = task
                .description
                .replace(priority_match.as_str(), "")
                .trim()
                .to_string();
        }

        // Extract tags
        for tag_match in self.tag_regex.find_iter(&task.description) {
            task.tags.push(tag_match.as_str()[1..].to_string());
        }
        task.description = self
            .tag_regex
            .replace_all(&task.description, "")
            .trim()
            .to_string();

        task
    }

    // Helper function to get the next occurrence of a weekday
    fn next_weekday(&self, from: chrono::DateTime<Local>, target_weekday: u32) -> DateTime<Utc> {
        let current_weekday = from.weekday().num_days_from_monday();
        let days_until_target = if target_weekday >= current_weekday {
            target_weekday - current_weekday
        } else {
            7 - current_weekday + target_weekday
        };

        // If it's today and we haven't passed the typical work hours, use today
        let days_to_add = if days_until_target == 0 && from.hour() < 18 {
            0
        } else if days_until_target == 0 {
            7 // Next week
        } else {
            days_until_target
        };

        (from + Duration::days(days_to_add as i64)).with_timezone(&Utc)
    }
}

#[derive(Debug)]
pub struct ParsedTask {
    pub description: String,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub priority: i32,
    pub tags: Vec<String>,
}
