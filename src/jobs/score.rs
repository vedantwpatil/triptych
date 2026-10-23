//! Rule scoring of a posting against the resume-derived [`Profile`]. Roles are matched as whole
//! words, so `ml` does not hit `html`.

use std::ops::RangeInclusive;

use serde::Deserialize;

use super::digest::Posting;
use crate::nlp::ollama_client::JobFit;

/// What counts as a fit. The defaults come from the resume; `TRIPTYCH_JOBS_PROFILE` points at a TOML
/// file that overrides any field (omitted fields keep the default).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Profile {
    /// Digest senders (bare, lowercase addresses).
    pub senders: Vec<String>,
    /// Role words that raise the score by 2 each.
    pub up: Vec<String>,
    /// Role words that lower it by 3 each.
    pub down: Vec<String>,
    /// Role words that rule a posting out whatever its score.
    pub exclude: Vec<String>,
    /// Lowest score that becomes a task.
    pub shortlist_at: i32,
    /// Digests older than this are not scanned.
    pub max_age_days: i64,
    /// Most tasks one scan may create per digest; the rest are skipped.
    pub max_per_digest: usize,
}

pub const PROFILE_ENV: &str = "TRIPTYCH_JOBS_PROFILE";

fn words(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_string()).collect()
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            senders: words(&["noreply@swelist.com"]),
            up: words(&[
                "software",
                "machine learning",
                "ml",
                "ai",
                "data",
                "backend",
                "back end",
                "platform",
                "infrastructure",
                "cloud",
                "distributed",
                "systems",
                "devops",
                "sre",
                "developer",
            ]),
            down: words(&[
                "hardware",
                "silicon",
                "rf",
                "electrical",
                "gis",
                "product manager",
                "program manager",
                "analyst",
                "validation",
                "test",
                "safety",
                "mechanical",
                "business",
                "governance",
                "sales",
                "marketing",
            ]),
            exclude: words(&["phd", "masters", "master s", "sophomore only", "freshman"]),
            shortlist_at: 2,
            max_age_days: 14,
            max_per_digest: 10,
        }
    }
}

impl Profile {
    /// The default profile, or the TOML file named by [`PROFILE_ENV`]. An unreadable or invalid
    /// file logs a warning and falls back to the default.
    #[must_use]
    pub fn load() -> Self {
        let Ok(path) = std::env::var(PROFILE_ENV) else {
            return Self::default();
        };
        std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|text| toml::from_str(&text).map_err(|e| e.to_string()))
            .unwrap_or_else(|e| {
                tracing::warn!("[Jobs] {PROFILE_ENV} ({path}): {e}; using the defaults");
                Self::default()
            })
    }

    #[must_use]
    pub fn is_sender(&self, addr: &str) -> bool {
        self.senders.iter().any(|s| s.eq_ignore_ascii_case(addr))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub score: i32,
    pub hard_no: bool,
    /// The words that decided it, e.g. `+software -test`.
    pub reason: String,
}

/// Rule scores in this range are too close to call (no word hit, or up and down cancelling), so
/// the model gets a say.
pub const LLM_BAND: RangeInclusive<i32> = -1..=1;

impl Verdict {
    #[must_use]
    pub const fn shortlisted(&self, profile: &Profile) -> bool {
        !self.hard_no && self.score >= profile.shortlist_at
    }

    /// True when the rules cannot decide: not excluded, and scored inside [`LLM_BAND`].
    #[must_use]
    pub fn needs_llm(&self) -> bool {
        !self.hard_no && LLM_BAND.contains(&self.score)
    }

    /// Folds in a model rating. `fit` (0-10) moves the score by `fit - 5`: 5 changes nothing and
    /// the rule evidence still counts, so one wrong rating cannot override a clear rule hit.
    pub fn apply_fit(&mut self, fit: &JobFit) {
        self.score += fit.fit - 5;
        self.reason = format!("{} llm:{} {}", self.reason, fit.fit, fit.reason)
            .trim()
            .to_string();
    }
}

/// Role text as space-padded lowercase words, so `contains(" ml ")` is a whole-word test.
fn padded(role: &str) -> String {
    let words: Vec<String> = role
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    format!(" {} ", words.join(" "))
}

#[must_use]
pub fn score(posting: &Posting, profile: &Profile) -> Verdict {
    let role = padded(&posting.role);
    let hits = |list: &'_ [String]| -> Vec<String> {
        list.iter()
            .filter(|w| role.contains(&format!(" {w} ")))
            .cloned()
            .collect()
    };
    let (up, down, excluded) = (
        hits(&profile.up),
        hits(&profile.down),
        hits(&profile.exclude),
    );
    let reason = excluded
        .iter()
        .map(|w| format!("!{w}"))
        .chain(up.iter().map(|w| format!("+{w}")))
        .chain(down.iter().map(|w| format!("-{w}")))
        .collect::<Vec<_>>()
        .join(" ");
    Verdict {
        score: 2 * i32::try_from(up.len()).unwrap_or(0)
            - 3 * i32::try_from(down.len()).unwrap_or(0),
        hard_no: !excluded.is_empty(),
        reason,
    }
}
