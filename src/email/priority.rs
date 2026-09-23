//! Display-time email priority: a keyword heuristic over the subject, snippet and sender.
//!
//! Nothing is stored, so the rules can change without a migration. Read state is deliberately not an
//! input, so opening a message never moves it in the list.

use super::EmailMessage;

/// How the Email view orders its rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmailSort {
    /// Highest score first, newest first within a score.
    #[default]
    Priority,
    /// Newest first.
    Date,
}

impl EmailSort {
    #[must_use]
    pub const fn toggled(self) -> Self {
        match self {
            Self::Priority => Self::Date,
            Self::Date => Self::Priority,
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Priority => "priority",
            Self::Date => "date",
        }
    }
}

/// Badge tier derived from a score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Normal,
    Medium,
    High,
}

impl Level {
    /// Marker shown before the subject, `None` for normal mail.
    #[must_use]
    pub const fn badge(self) -> Option<&'static str> {
        match self {
            Self::High => Some("▲"),
            Self::Medium => Some("△"),
            Self::Normal => None,
        }
    }
}

const HIGH: i32 = 4;
const MEDIUM: i32 = 2;

const HIGH_WORDS: &[&str] = &[
    "urgent", "asap", "action required", "final notice", "immediately", "deadline", "overdue",
    "expires", "expiring", "past due", "important",
];
const MEDIUM_WORDS: &[&str] = &[
    "due", "reminder", "assignment", "exam", "interview", "invoice", "payment", "meeting",
    "schedule", "rsvp", "reply", "confirm",
];
const BULK_WORDS: &[&str] = &[
    "newsletter", "sale", "unsubscribe", "digest", "deals", "discount", "webinar", "promo",
    "promotion",
];
const BULK_SENDERS: &[&str] = &["noreply", "no-reply", "donotreply", "do-not-reply"];

/// Lowercases `text` and collapses every run of non-alphanumerics into one space, padded both ends
/// so a whole-word match is a plain substring search for `" word "`.
fn normalize(text: &str) -> String {
    let mut out = String::from(" ");
    for word in text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
    {
        out.push_str(&word.to_lowercase());
        out.push(' ');
    }
    out
}

fn has_any(text: &str, words: &[&str]) -> bool {
    words.iter().any(|w| text.contains(&format!(" {w} ")))
}

/// Higher means more worth reading first. Each keyword group counts once, however many of its words appear.
#[must_use]
pub fn score(email: &EmailMessage) -> i32 {
    let text = normalize(&format!(
        "{} {}",
        email.subject,
        email.snippet.as_deref().unwrap_or_default()
    ));
    let mut score = 0;
    if has_any(&text, HIGH_WORDS) {
        score += HIGH;
    }
    if has_any(&text, MEDIUM_WORDS) {
        score += MEDIUM;
    }
    if has_any(&text, BULK_WORDS) {
        score -= 3;
    }
    let sender = email.from_addr.to_lowercase();
    if BULK_SENDERS.iter().any(|s| sender.contains(s)) {
        score -= 2;
    }
    if email.task_id.is_some() {
        score -= 3; // already handled
    }
    score
}

#[must_use]
pub const fn level(score: i32) -> Level {
    if score >= HIGH {
        Level::High
    } else if score >= MEDIUM {
        Level::Medium
    } else {
        Level::Normal
    }
}
