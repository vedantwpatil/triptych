# Development Log

Dev workflow, changelog, known issues for Triptych. See [`../CLAUDE.md`](../CLAUDE.md) (module
map, build/test/lint), [`roadmap.md`](./roadmap.md), [`roadmap-email.md`](./roadmap-email.md).

## Workflow

```
cargo build
cargo test      # in-memory sqlite; tests in src/app.rs + pure-fn tests in src/ui.rs, src/email/, src/nlp/
cargo clippy
```

Run all three before considering a change done. Known Issues resolved in place, never deleted.

## Changelog

- 2026-09-17: `install_panic_hook()` in `main.rs` — a panic mid-TUI used to skip the raw-mode/
  alt-screen teardown, breaking the terminal. Hook restores it before the panic prints.
- 2026-09-17: Fixed calendar same-hour task collision. `src/ui.rs`: `find_task_display`/
  `build_cell_content`/`get_cell_text` → `cell_task_displays` (collects every task sharing an
  hour, not just the first) + `build_cell_view` → `CellView{headline, overflow, style}`; 2-line
  cell, dim `+N more`. Added `ORDER BY ..., id` tiebreakers (`get_scheduled_tasks_internal`,
  `get_week_allocations_internal`). First `ui.rs` `mod tests` (3 cases).
- 2026-09-17: Current-hour row accent. `today_accent()` (`src/ui.rs`) shared by header + now-row:
  `▸` prefix + accent on the time label, `UNDERLINED` patched onto today's column at that hour
  (before the selection override). No accent when the displayed week doesn't contain today.
- 2026-09-17: Reallocation conflicts now say why. `ConflictReason::{BeyondWindow, OutOfCapacity}`,
  `TaskConflict.reason`, `AllocationResult::conflict_summary()` (`src/app.rs`) — one wording
  shared by the TUI status message and CLI `schedule reallocate`. `ALLOCATION_WINDOW_DAYS` is now
  a named const. +7 tests.
- 2026-09-17: Fixed stale multi-account docs (`src/email/CLAUDE.md`, `src/sync/CLAUDE.md`) —
  described old `EmailConfig::from_env() -> Option`; code is `all_from_env() -> Vec`. Reworded
  `mail.rs`'s misleading "TRIPTYCH_EMAIL_ENABLED not set" log line.
- 2026-09-17: Fixed 2 panics in `nlp/rules.rs` found by the idioms audit below: (1) 6 sites did
  `.and_local_timezone(Local).unwrap()`, panicking on a DST transition — now route through
  `app::resolve_local_datetime`. (2) `"eom"` called `.with_month()` before `.with_day(1)`,
  panicking on e.g. Jan 31 → "Feb 31" — reordered. +2 tests.
- 2026-09-17: Fixed both remaining calendar Known Issues. `allocate_task_to_blocks` (`src/app.rs`)
  now writes each allocation's own start/end (`block.start_time + Duration::minutes(used)`), not
  the block's — de-stacks tasks sharing one block. New `pub(crate) allocation_covers_hour` (range
  check, shared by `app.rs::cell_tasks` and `ui.rs::cell_task_displays`) so allocations render
  across every hour they span, not just their start. Deleted the now-unused `find_cell_task`/
  `App::task_at_cell`. Added `App::stack_index` + `cycle_stack_next`/`_prev` on `[`/`]`, reset on
  every cursor/week/view-entry move, so `m`/`u`/`e` reach any task in a stacked cell. Overflow
  text `"+N more"` → `"position/total"`. +2 tests, 2 updated (44 total).

## Rust Idioms Audit (2026-09-17)

Ran the `rust-idioms` skill checklist against `src/`. 2 bugs found — see Resolved below. Reviewed
clean, no action taken:

- `.clone()` (34 sites): 10 `Arc` (cheap), 24 owned at real ownership boundaries.
- Errors: `nlp/` has 2 real enumerate-shape errors (`OllamaError`, `ParseError`); everywhere else
  correctly erases to `sqlx::Error`/`anyhow::Error` since callers never branch on cause.
- `&Vec`/`&String` params, nested smart pointers, `unsafe`: 0 hits.
- Global state: 1 `std::sync::Once` (`email/client.rs`), narrowly scoped.
- Lifetimes: only `ui.rs`'s `CalendarGrid<'a>`/`cell_task_displays<'a>`, justified (avoids
  cloning the cached week data every frame).
- Blocking-in-async: `email/client.rs`, `nlp/ollama_client.rs` both genuinely async. Not
  exhaustively verified.
- Derives: `App` has none; every field is `Debug`+`Clone`-able, so `#[derive(Debug)]` would be
  free. Low priority, not filed.

## Known Issues

### Open

- None currently.

### Resolved

- DST-transition panic in `nlp/rules.rs` (6 `.and_local_timezone().unwrap()` sites). Fixed
  2026-09-17.
- `"eom"` month-rollover panic in `nlp/rules.rs`. Fixed 2026-09-17.
- Terminal left broken after a panic mid-TUI. Fixed 2026-09-17.
- Calendar same-hour task collision hid tasks silently. Fixed 2026-09-17.
- Calendar had no current-time indicator. Fixed 2026-09-17.
- Deadline reallocation conflicts didn't say why. Fixed 2026-09-17.
- Stale multi-account docs (`email`/`sync` `CLAUDE.md`). Fixed 2026-09-17.
- Calendar grid actions only reached the first task of a stacked cell. Fixed 2026-09-17.
- Deadline allocations only rendered in their block's start hour. Fixed 2026-09-17.

## Open Questions

- None blocking right now.
