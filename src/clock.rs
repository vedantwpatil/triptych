//! Wall-clock source for the app. `TRIPTYCH_NOW` (RFC 3339) freezes it, so the end-to-end suite
//! in `tests/tui/` gets the same answer on every run. Unset, it is the system clock.

use chrono::{DateTime, Local, Utc};
use std::sync::OnceLock;

pub const NOW_ENV: &str = "TRIPTYCH_NOW";

fn pinned() -> Option<DateTime<Utc>> {
    static PINNED: OnceLock<Option<DateTime<Utc>>> = OnceLock::new();
    *PINNED.get_or_init(|| {
        let raw = std::env::var(NOW_ENV).ok()?;
        DateTime::parse_from_rfc3339(&raw)
            .ok()
            .map(|t| t.with_timezone(&Utc))
    })
}

#[must_use]
pub fn now() -> DateTime<Utc> {
    pinned().unwrap_or_else(Utc::now)
}

#[must_use]
pub fn local_now() -> DateTime<Local> {
    now().with_timezone(&Local)
}
