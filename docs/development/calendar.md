# Calendar history

Weekly grid, cell popup, to-do panel, schedule blocks, allocation and reallocation. Newest first.
Hub, workflow and open issues: [`../DEVELOPMENT.md`](../DEVELOPMENT.md). Code:
[`../../src/app/CLAUDE.md`](../../src/app/CLAUDE.md), [`../../src/tui/CLAUDE.md`](../../src/tui/CLAUDE.md).
Related: [`todo.md`](./todo.md), [`tui-ux.md`](./tui-ux.md).

## Changelog

- 2026-10-12 (calendar on small terminals): the grid drew all 16 hour rows and 7 days whatever the
  terminal size, so the cursor row scrolled off screen and headers truncated (`Thu 0`).
  `render_calendar_view` now sizes both axes from the grid area (after the to-do panel split) and draws
  only the window returned by `ui::window(selected, total, visible)` (`src/tui/ui/grid.rs`): pure,
  centred on the cursor, clamped at the ends, so no scroll offset is stored in `App` and `render_*`
  stays `&App`. Rows are 2 high plus 4 of overhead; a day column needs 10 cells (`Thu 10/09` and
  spacing). A full-size terminal shows everything as before. Tests: `tests/it/ui.rs`
  (`window_keeps_the_cursor_in_view_and_inside_the_bounds`), scenario `cal_small_scroll` (60x16, scrolls
  to 10pm and Sunday, then opens the panel). Suite is 182 scenarios. Open: the calendar title and cell
  headlines still truncate when narrow.
- 2026-10-12 (calendar to-do panel): `T` in the calendar opens a right-hand panel of open tasks
  (`src/app/calendar_todo.rs`, `src/tui/ui/calendar_todo.rs`, `CalendarInputMode::TodoPanel`). `Enter`
  schedules the highlighted task at the grid cursor (which stays drawn), `x` completes it through
  `complete_task_by_id` (recurrence and reallocation included), `Esc` returns to the grid with the panel
  open, `T` hides it. The three `scheduled_at` writes (picker, held-task drop, panel) now share
  `schedule_task_at_selected_cell`. After scheduling, the panel cursor follows the task because
  `load_tasks` re-sorts by urgency, which a new `scheduled_at` can raise. Tests: `tests/it/app.rs`
  (`calendar_todo_*`), scenarios `cal_todo_panel`, `cal_todo_panel_empty`. Suite is 181 scenarios (the docs
  said 173, already stale). `todo_add_cold_model` can fail under `-j 4` from real-Ollama timing; it passes
  alone. Open: fixed 30% width, no unscheduled-only filter.
- 2026-09-29 (calendar cell popup): `Enter` opens `CalendarInputMode::CellDetail` (full titles, kind,
  priority, deadline, tags); `stack_index` is its cursor so `m`/`u`/`e` reuse the grid paths. Cause: the
  12-char headline truncation made stacked `CS-472: Q...` tasks look identical while cycling. Blocks
  imported from TOML get `schedule_blocks.source = 'toml'` (idempotent migration `add_block_source`) and
  `d` refuses them. New scenarios `cal_cell_detail`, `cal_toml_block_protected`. `todo_reword` failed at
  HEAD (scenario pressed `e`, key is `r`); fixed, see the reword key entry in [`todo.md`](./todo.md).
- 2026-09-18: Fixed calendar view swallowing `m`/`u`/`e`/`d` feedback. `render_calendar_view`
  (`src/ui.rs`) never rendered `app.status_message` - unlike `render_todo_view`/
  `render_email_view`, which both do - so error paths like "No scheduled task here" (pressing
  `u`/`m`/`e` on an empty cell) or "Can't move a deadline allocation directly" set the message
  correctly in `App` but nothing ever showed it. Looked identical to the key doing nothing.
  Added a status-line chunk (`Constraint::Length(3)`, same 3s-fade pattern as the other two
  views), shown only in `CalendarInputMode::Navigate` since popups (`n`/`s`/`a`) cover the area
  anyway. Root-caused by driving the compiled binary through a real pty (`expect`, isolated
  sandbox via `DATABASE_URL`/`TRIPTYCH_SOCKET_PATH`) rather than just reading `keys.rs` -
  dispatch logic for every calendar key was already correct, confirming `n`/`s`/`a` open their
  popups and `u` sets its status text; the gap was purely the missing render call.
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
- 2026-09-17: Fixed both remaining calendar Known Issues. `allocate_task_to_blocks` (`src/app.rs`)
  now writes each allocation's own start/end (`block.start_time + Duration::minutes(used)`), not
  the block's — de-stacks tasks sharing one block. New `pub(crate) allocation_covers_hour` (range
  check, shared by `app.rs::cell_tasks` and `ui.rs::cell_task_displays`) so allocations render
  across every hour they span, not just their start. Deleted the now-unused `find_cell_task`/
  `App::task_at_cell`. Added `App::stack_index` + `cycle_stack_next`/`_prev` on `[`/`]`, reset on
  every cursor/week/view-entry move, so `m`/`u`/`e` reach any task in a stacked cell. Overflow
  text `"+N more"` → `"position/total"`. +2 tests, 2 updated (44 total).

## Resolved issues

Indexed in [`../DEVELOPMENT.md`](../DEVELOPMENT.md#known-issues).

- **KI-22** (2026-09-23) A manually scheduled task showed, and could be picked up, only in its start hour:
  cells matched `scheduled_at.hour() == hour` and the cache carried no duration (allocations had one, but
  their check missed a start off the hour, such as 12:30). One rule, `span_covers_hour`, now serves
  `cell_tasks`, the grid render and the auto-scheduler's free-slot check, so a 180 min task from 07:00
  fills 07-09 and a 12:30 hour spans 12 and 13. Not covered: `find_next_available_slot` still does not
  check that the task being placed fits before the next occupied hour. `cal_long_task_spans`,
  `tests/it/app.rs` (two tests).
- **KI-5** A block ending before it started was accepted. `validate_time_range` (`src/app.rs`) now
  guards `create_schedule_block` and `schedule import`. `sched_end_before_start`, `cal_block_backwards`.
- **KI-6** Block-form errors were invisible. The form popup (`src/ui.rs`) now has a fifth row that
  shows `status_message` in red. `cal_block_error_visible`.
- **KI-9** `schedule show` omitted the week's task allocations. `print_schedule_summary` now ends
  with an "Allocated tasks:" section. `sched_show_allocations`.

### Earlier fixes (not numbered)

- Calendar same-hour task collision hid tasks silently. Fixed 2026-09-17.
- Calendar had no current-time indicator. Fixed 2026-09-17.
- Deadline reallocation conflicts didn't say why. Fixed 2026-09-17.
- Calendar grid actions only reached the first task of a stacked cell. Fixed 2026-09-17.
- Deadline allocations only rendered in their block's start hour. Fixed 2026-09-17.
- README.md's `schedule.toml` example used field names that don't match the code
  (`day_of_week`/`start_time`/`end_time`/`block_type` vs. actual `day`/`start`/`end`/`type`),
  so copy-pasting it failed to import. Fixed 2026-09-18.
- Calendar view (`render_calendar_view`, `src/ui.rs`) never rendered `app.status_message`,
  so `m`/`u`/`e`/`d` error feedback (e.g. "No scheduled task here") was silently dropped -
  looked like the key did nothing. Fixed 2026-09-18 (see Changelog).
