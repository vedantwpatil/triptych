//! Parses a SWE List digest body into postings.
//!
//! The plain-text body is an intro, a `Link to Simplify` line, one `Company:Role` line per posting,
//! then `See more details ...`. Only the lines between the two markers count; a body without both
//! yields nothing, so a changed template fails closed.

use crate::email::links::Link;

const START: &str = "link to simplify";
const END: &str = "see more details";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posting {
    pub company: String,
    pub role: String,
    /// The apply link, when an anchor in the HTML body has this posting's text.
    pub url: Option<String>,
}

impl Posting {
    /// Dedupe key and tombstone in `job_postings`.
    #[must_use]
    pub fn key(&self) -> String {
        format!(
            "{}|{}",
            self.company.to_lowercase(),
            self.role.to_lowercase()
        )
    }
}

/// The postings of one digest, repeats collapsed, in body order.
#[must_use]
pub fn parse_digest(body: &str, links: &[Link]) -> Vec<Posting> {
    let mut text = body.lines().map(str::trim);
    if !text.any(|l| l.to_lowercase().starts_with(START)) {
        return Vec::new();
    }
    let mut found: Vec<Posting> = Vec::new();
    for line in text {
        if line.to_lowercase().starts_with(END) {
            return found.into_iter().map(|p| with_url(p, links)).collect();
        }
        let Some((company, role)) = line.split_once(':') else {
            continue;
        };
        let posting = Posting {
            company: company.trim().to_string(),
            role: role.trim().to_string(),
            url: None,
        };
        if !posting.company.is_empty()
            && !posting.role.is_empty()
            && found.iter().all(|p| p.key() != posting.key())
        {
            found.push(posting);
        }
    }
    // No end marker: the template changed, so trust nothing.
    Vec::new()
}

/// Letters and digits only, lowercase: `Acme: SWE Intern` and `Acme:SWE Intern` compare equal.
fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn with_url(mut posting: Posting, links: &[Link]) -> Posting {
    let want = norm(&posting.company) + &norm(&posting.role);
    posting.url = links
        .iter()
        .find(|l| norm(&l.text) == want)
        .map(|l| l.href.clone());
    posting
}
