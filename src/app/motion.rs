//! Vim-style motions: counts (`5j`), `gg`/`G`, `0`/`$` and `Ctrl-d`/`Ctrl-u`.
//!
//! Terminal-free on purpose: `tui/keys.rs` turns a `KeyEvent` into a [`MotionKey`], [`KeyPrefix::feed`]
//! decides what it means, and the pure `*_target` functions compute where the cursor goes. That keeps the
//! logic testable from `tests/it`, which cannot reach the private `tui` module.

/// A cursor movement, before a view decides what it means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Down,
    Up,
    Left,
    Right,
    /// `gg`; with a count, that row (`3gg`).
    Top,
    /// `G`; with a count, that row (`3G`).
    Bottom,
    /// `0`
    LineStart,
    /// `$`
    LineEnd,
    /// `Ctrl-d`
    HalfDown,
    /// `Ctrl-u`
    HalfUp,
}

/// The keys a motion cares about, independent of crossterm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionKey {
    Char(char),
    Ctrl(char),
    Up,
    Down,
    Left,
    Right,
    Esc,
    Other,
}

/// What [`KeyPrefix::feed`] did with a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Feed {
    /// A complete motion; `count` is `None` when the user typed none.
    Motion { motion: Motion, count: Option<usize> },
    /// The key was part of a prefix (a digit, the first `g`) or cancelled one: nothing else should see it.
    Consumed,
    /// Not a motion key and no prefix was pending: the view handles it.
    Other,
}

/// Counts above this are clamped, so typing digits forever cannot overflow.
const MAX_COUNT: usize = 100_000;

/// A count and a first `g` typed so far.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyPrefix {
    count: Option<usize>,
    g: bool,
}

impl KeyPrefix {
    #[must_use]
    pub const fn is_pending(&self) -> bool {
        self.count.is_some() || self.g
    }

    pub const fn clear(&mut self) {
        self.count = None;
        self.g = false;
    }

    /// Feeds one key. A key that is not part of a motion cancels a pending prefix and is swallowed, so
    /// `3d` does not delete a task the user meant to skip past, and Esc only clears the prefix.
    pub const fn feed(&mut self, key: MotionKey) -> Feed {
        let pending = self.is_pending();
        let motion = match key {
            MotionKey::Char('g') if self.g => Motion::Top,
            MotionKey::Char('g') => {
                self.g = true;
                return Feed::Consumed;
            }
            _ if self.g => {
                self.clear();
                return Feed::Consumed;
            }
            MotionKey::Char(d @ '1'..='9') => return self.push_digit(d),
            MotionKey::Char('0') if self.count.is_some() => return self.push_digit('0'),
            MotionKey::Char('j') | MotionKey::Down => Motion::Down,
            MotionKey::Char('k') | MotionKey::Up => Motion::Up,
            MotionKey::Char('h') | MotionKey::Left => Motion::Left,
            MotionKey::Char('l') | MotionKey::Right => Motion::Right,
            MotionKey::Char('G') => Motion::Bottom,
            MotionKey::Char('0') => Motion::LineStart,
            MotionKey::Char('$') => Motion::LineEnd,
            MotionKey::Ctrl('d') => Motion::HalfDown,
            MotionKey::Ctrl('u') => Motion::HalfUp,
            _ if pending => {
                self.clear();
                return Feed::Consumed;
            }
            _ => return Feed::Other,
        };
        let count = self.count;
        self.clear();
        Feed::Motion { motion, count }
    }

    const fn push_digit(&mut self, digit: char) -> Feed {
        let value = digit as usize - '0' as usize;
        let next = match self.count {
            Some(c) => c.saturating_mul(10).saturating_add(value),
            None => value,
        };
        self.count = Some(if next > MAX_COUNT { MAX_COUNT } else { next });
        Feed::Consumed
    }
}

/// Where a list cursor at `cur` in `len` rows goes, or `None` if the motion does not apply to a list.
/// `page` is the visible row count, for the half-page motions.
#[must_use]
pub fn list_target(
    cur: usize,
    len: usize,
    page: usize,
    motion: Motion,
    count: Option<usize>,
) -> Option<usize> {
    let last = len.checked_sub(1)?;
    let step = count.unwrap_or(1);
    let half = (page / 2).max(1);
    let row = |n: usize| n.saturating_sub(1).min(last);
    match motion {
        Motion::Down => Some(cur.saturating_add(step).min(last)),
        Motion::Up => Some(cur.saturating_sub(step)),
        Motion::Top => Some(count.map_or(0, row)),
        Motion::Bottom => Some(count.map_or(last, row)),
        Motion::HalfDown => Some(cur.saturating_add(half).min(last)),
        Motion::HalfUp => Some(cur.saturating_sub(half)),
        Motion::Left | Motion::Right | Motion::LineStart | Motion::LineEnd => None,
    }
}

/// Where a `(row, col)` cursor in a `rows` x `cols` grid goes. `0`/`$` jump to the first/last column and
/// half pages move `rows / 2` rows.
#[must_use]
pub fn grid_target(
    pos: (usize, usize),
    size: (usize, usize),
    motion: Motion,
    count: Option<usize>,
) -> (usize, usize) {
    let (row, col) = pos;
    let (last_row, last_col) = (size.0.saturating_sub(1), size.1.saturating_sub(1));
    let step = count.unwrap_or(1);
    let half = (size.0 / 2).max(1);
    let line = |n: usize| n.saturating_sub(1).min(last_row);
    match motion {
        Motion::Down => (row.saturating_add(step).min(last_row), col),
        Motion::Up => (row.saturating_sub(step), col),
        Motion::Left => (row, col.saturating_sub(step)),
        Motion::Right => (row, col.saturating_add(step).min(last_col)),
        Motion::Top => (count.map_or(0, line), col),
        Motion::Bottom => (count.map_or(last_row, line), col),
        Motion::LineStart => (row, 0),
        Motion::LineEnd => (row, last_col),
        Motion::HalfDown => (row.saturating_add(half).min(last_row), col),
        Motion::HalfUp => (row.saturating_sub(half), col),
    }
}

/// The next index after `cur` (before it, if `!forward`) in a list of `len` where `is_match` holds,
/// wrapping around; the flag says it wrapped. `cur` itself is tried last, so a lone match is found.
#[must_use]
pub fn find_match(
    len: usize,
    cur: usize,
    forward: bool,
    is_match: impl Fn(usize) -> bool,
) -> Option<(usize, bool)> {
    (1..=len).find_map(|off| {
        let (idx, wrapped) = if forward {
            ((cur + off) % len, cur + off >= len)
        } else {
            ((cur + len - off % len) % len, off > cur)
        };
        is_match(idx).then_some((idx, wrapped))
    })
}
