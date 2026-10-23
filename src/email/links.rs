//! Hyperlinks in an HTML mail body: the text and target of each `<a href>`.
//!
//! Stored per message in `email_messages.links` so a later pass (`jobs::scan`) can pair a line of the
//! plain-text body with its URL, which `body_text` alone drops.

use serde::{Deserialize, Serialize};

use crate::app::is_web_url;

/// Most anchors kept per message, and the longest anchor text kept.
const MAX_LINKS: usize = 100;
const MAX_TEXT: usize = 160;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub text: String,
    pub href: String,
}

/// The `http(s)` anchors of `html` in document order, without exact repeats.
#[must_use]
pub fn anchors(html: &str) -> Vec<Link> {
    // ASCII lowering keeps byte offsets, so indices found in `lower` slice `html` too.
    let lower = html.to_ascii_lowercase();
    let mut links: Vec<Link> = Vec::new();
    let mut at = 0;
    while links.len() < MAX_LINKS {
        let Some(open) = lower[at..].find("<a ").map(|i| at + i) else {
            break;
        };
        let Some(tag_end) = lower[open..].find('>').map(|i| open + i) else {
            break;
        };
        let close = lower[tag_end..]
            .find("</a")
            .map_or(html.len(), |i| tag_end + i);
        at = close.max(tag_end + 1);
        let Some(href) = attr(&html[open..tag_end], "href").map(|h| unescape(&h)) else {
            continue;
        };
        let href = href.trim().to_string();
        let link = Link {
            text: plain(&html[tag_end + 1..close]),
            href,
        };
        if is_web_url(&link.href) && !links.contains(&link) {
            links.push(link);
        }
    }
    links
}

/// The stored form; `None` when there are no links.
#[must_use]
pub fn to_json(links: &[Link]) -> Option<String> {
    (!links.is_empty())
        .then(|| serde_json::to_string(links).ok())
        .flatten()
}

/// Links from the stored form; empty for `None` or text that is not a JSON list.
#[must_use]
pub fn from_json(json: Option<&str>) -> Vec<Link> {
    json.and_then(|j| serde_json::from_str(j).ok())
        .unwrap_or_default()
}

/// The value of attribute `name` in an opening tag, quoted or bare.
fn attr(tag: &str, name: &str) -> Option<String> {
    let needle = format!(" {name}=");
    let start = tag.to_ascii_lowercase().find(&needle)? + needle.len();
    let rest = &tag[start..];
    let value = match rest.chars().next()? {
        quote @ ('"' | '\'') => rest[1..].split(quote).next()?,
        _ => rest.split(char::is_whitespace).next()?,
    };
    Some(value.to_string())
}

fn unescape(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// Tag-free text with whitespace collapsed, cut to [`MAX_TEXT`] characters.
fn plain(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    unescape(&out)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_TEXT)
        .collect()
}
