# Todo list history

Task list: ordering, urgency badges and colours, recurrence, reword, text caret, visual delete. Newest first.
Hub, workflow and open issues: [`../DEVELOPMENT.md`](../DEVELOPMENT.md). Code:
[`../../src/app/CLAUDE.md`](../../src/app/CLAUDE.md), [`../../src/tui/CLAUDE.md`](../../src/tui/CLAUDE.md).
Related: [`nlp.md`](./nlp.md), [`calendar.md`](./calendar.md), [`tui-ux.md`](./tui-ux.md).

## Changelog

- 2026-10-25 (repeat intervals, day lists): `every 2 days|weeks|months`, `every other day|week|month`,
  `every other friday`, trailing `biweekly`, and weekday lists (`every mon, wed and fri`, `every tue/thu`,
  `every 2 weeks on fri`). `Recurrence` in `src/recurrence.rs` is now `{ every, unit }` (1..=999) with
  `Unit::{Day, Week, Month, On(WeekdaySet)}`; `extract(input, today)` leaves the long name of the
  soonest listed day after today for the date parser, which never resolves a weekday name to today
  (`every weekday` leaves nothing). A joining `and`
  counts only before a weekday ("every fri and buy milk" repeats on fri). The stored key reuses the
  phrase grammar (`fri`, `mon,wed,fri`, `every 2 weeks on mon,wed`; interval 1 keeps the old
  `daily`/`weekly`/`monthly`/`weekdays`), so `from_key` is the same parser, serde writes the key string
  and existing rows read unchanged (no migration). An interval day list skips the extra weeks only when
  the next day wraps into a new Monday-start week: `every 2 weeks on mon,wed` goes Mon, Wed (+2 days),
  Mon two weeks on (+12). Tests: `tests/it/recurrence.rs` (`intervals_and_day_lists_parse`,
  `extract_leaves_the_soonest_day_from_today`, `interval_day_lists_skip_whole_weeks_only_when_wrapping`,
  `serde_is_the_key_string`), scenarios `todo_repeat_interval`, `todo_repeat_day_list`. Open: no end
  date (delete the open copy to end a series); monthly drift after a short-month clamp (31 Jan, 28 Feb,
  28 Mar); reopening a task leaves its spawned copy; an undated first task stays undated.
- 2026-09-30 (recurring tasks): `every day|weekday|week|month|<weekday>` in a task. Rules are pure in
  `src/recurrence.rs` (`Recurrence`, `extract`, `next_after`, `next_dates`); `NLPParser::parse` strips
  the phrase first (`every fri` becomes `friday`, since the date parser only knows full names) and
  returns it as `ParseResult.recurrence`, so the regex, LLM and cache layers are untouched. Stored in
  `tasks.recurrence` (`add_recurrence` migration, key like `fri`, `weekly`). All three insert paths bind
  it: `insert_parsed_task` (TUI), `App::add_task` (CLI), `add_task_to_db` (daemon).
  `App::toggle_completed` and `complete_task_by_id` call `add_next_occurrence` (`src/app/repeat.rs`),
  which copies the row with shifted dates and skips when an identical open copy exists (done, reopen,
  done). `↻ rule` in the todo row and `[↻ rule]` in `triptych list`. Tests: `tests/it/recurrence.rs`,
  `tests/it/app.rs`, `tests/it/cli.rs`, scenario `todo_repeat_complete`. Open: no `every 2 weeks`, no
  multi-weekday, no end date; reopening a task leaves its spawned copy; an undated first task stays
  undated but its copies get an end-of-day deadline; monthly drift after a clamp.
- 2026-09-30 (reword key): todo reword is `r`/`R` (it moved from `e` in `a25f249`); scenario
  `todo_reword` and the docs now match, and the scenario uses `R` for the second reword.
- 2026-09-29 (text caret + word motions): every typed input (task, search, snooze, rule, calendar
  block/task/deadline, email compose) now has a caret, not append-only. `src/app/textedit.rs` is the pure
  edit engine (`Edit`, `apply_edit`; caret = `App.edit_cursor`, `None` = end); `tui/keys.rs::edit_for` maps
  keys and `type_into` applies them before mode dispatch, and any non-edit key resets the caret to the end.
  Renders place the terminal cursor with `cursor_col`. Scenario `text_word_nav`, driver tokens `M-x`,
  `M-LEFT`, `M-RIGHT`, `M-BS`.
- 2026-09-29 (todo colours): MED has no badge (default level, `priority_badge` returns `None`). Row
  colours: course-code prefix per course, green fitness (red is reserved for priority), magenta tags,
  dim struck-through done rows, cyan far dates. All named ANSI colours so the user's theme applies.
  New scenario `todo_row_colors`.
- 2026-09-29 (todo order): completed tasks sort below open ones, and toggling completion keeps the
  cursor on the same task (`toggle_completed` uses `reload_tasks_keep_selection`).
- 2026-09-29 (todo reword): `e` in the todo list reopens the input prompt pre-filled with the selected
  task; Enter saves it as the new description (no NLP re-parse, so date/priority/tags stay). Tests:
  `reword_task_updates_description_and_keeps_selection`, TUI scenario `todo_reword`.
- 2026-09-29 (todo order): `App::load_tasks` now stable-sorts the list by `urgency::effective_priority`
  (highest first, ties keep `item_order`), so date-raised tasks rise too. The DB `item_order` is
  unchanged. `load_tasks_orders_by_priority_then_item_order`.
- 2026-09-19 (delete fix, visual select): Deleting a task that an email was converted into failed
  with `FOREIGN KEY constraint failed` (KI-18). New: `v`/`V` in the todo list start a visual
  selection (`j`/`k` extend, `Esc` or any other key cancels) and `x`/`d`/`D` delete the whole range in
  one transaction; with no selection they delete the cursor row. Tests: 83 in-process, suite 92/92.
- 2026-09-19 (latest): Fixed KI-14..KI-17, found by a sandboxed todo-list test with college-student
  tasks. Bare weekday parsing (`src/nlp/rules.rs`), `[LOW]` and `[DUE ...]` badges, and automatic
  priority escalation near a date, all in the new `src/urgency.rs` and shared by `ui.rs` and the
  CLI `list`; the TUI badges are colour-coded by urgency (brighter red as it rises). Suite: 89
  scenarios, 0 XFAIL; 60 unit + 14 CLI tests. The CLI test
  `nlp_parses_tags_priority_and_relative_date` moved to a far-future date, because "tomorrow !!" now
  correctly shows `[URGENT↑]`; `priority_rises_when_the_date_is_near` covers the raise.

## Resolved issues

Indexed in [`../DEVELOPMENT.md`](../DEVELOPMENT.md#known-issues).

- **KI-18** Deleting a todo that came from an email (`x`, `triptych rm`, `clear`) failed with
  `FOREIGN KEY constraint failed`: `email_messages.task_id` references `tasks(id)` with no `ON DELETE`
  rule, and SQLite cannot alter one in place. `run_email_migration` now creates the
  `email_messages_unlink_task` trigger (`BEFORE DELETE ON tasks` sets the link to NULL), which covers
  every delete path and existing databases on next start. It must stay after the table rebuild, because
  `RENAME TO` would repoint it at the dropped table. `tests/it/app.rs` (three `*_unlinks_*` tests),
  `todo_delete_email_linked`.
- **KI-15** Priority 0 (Low) showed no badge in the TUI or `triptych list`. Both now show `[LOW]`
  through `urgency::priority_badge`. `cli_low_badge`, `todo_escalation_badges`.
- **KI-16** Deadlines ("by friday") showed no badge. Both views now show `[DUE Fri]`, `[DUE TMR]`,
  `[DUE 09/30]` (plus a time when it is not end of day) or `[OVERDUE]`; finished tasks show none.
  `cli_deadline_badge`, `todo_escalation_badges`.
- **KI-17** (feature) Priority now rises as the date nears. `urgency::effective_priority` takes the
  earlier of `scheduled_at` and `deadline` and sets a floor: <=24h or overdue is Urgent, <=3 days
  High, <=7 days Medium. The stored priority never changes and never goes down, so moving the date
  away undoes the raise; a raised badge carries an arrow (`[URGENT↑]`). Finished tasks keep their own
  priority. Display only: the auto-scheduler still orders by deadline and calendar colours are
  unchanged. Also: the TUI hides the time on date-only tasks (`[09/20]`, not `[09/20 12:00am]`).
  The TUI colours the priority, date and deadline badges by the effective level (`urgency_style` in
  `src/ui.rs`): LOW dark gray, MED gray, HIGH red, URGENT bold bright red. Only named ANSI colours
  are used, never RGB or 256-colour indexes, so the user's terminal theme picks the shades.
  `cli_priority_escalates`, `todo_escalation_badges`, `todo_urgency_colors`.

