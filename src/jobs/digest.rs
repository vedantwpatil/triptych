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
    /// The posting's page, when an anchor in the HTML body has this posting's role as its text.
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
    let mut anchors = links;
    for line in text {
        if line.to_lowercase().starts_with(END) {
            return found;
        }
        let Some((company, role)) = line.split_once(':') else {
            continue;
        };
        let (company, role) = (company.trim(), role.trim());
        if company.is_empty() || role.is_empty() {
            continue;
        }
        // Every line takes its anchor, a collapsed repeat included, so the next one cannot pair with it.
        let url = next_url(&mut anchors, role);
        let posting = Posting {
            company: company.to_string(),
            role: role.to_string(),
            url,
        };
        if found.iter().all(|p| p.key() != posting.key()) {
            found.push(posting);
        }
    }
    // No end marker: the template changed, so trust nothing.
    Vec::new()
}

/// Letters and digits only, lowercase: `SWE Intern` and `swe-intern` compare equal.
fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The `href` of the first anchor in `rest` whose text is `role`, leaving `rest` just past it. The
/// digest's anchors carry the role only (the company is plain text beside it) and run in body order,
/// so a role several companies share pairs by position; a role with no anchor leaves `rest` alone.
fn next_url(rest: &mut &[Link], role: &str) -> Option<String> {
    let want = norm(role);
    let at = rest.iter().position(|l| norm(&l.text) == want)?;
    let (hit, tail) = rest[at..].split_first()?;
    *rest = tail;
    Some(hit.href.clone())
}
