# Future features: feasibility

Assessed 2026-09-23 against the current tree. Each request is kept verbatim with its verdict below.
Sizes: S = a session, M = a few sessions, L = touches most of the app. Implemented 2026-09-23: vim
motions and search, email sync (`s`), summaries, priority. Suggested order for the rest: 8, 7, 1, 6. Related: [`roadmap.md`](./roadmap.md), [`roadmap-email.md`](./roadmap-email.md),
[`roadmap-llm.md`](./roadmap-llm.md), [`DEVELOPMENT.md`](./DEVELOPMENT.md), root [`CLAUDE.md`](../CLAUDE.md).

- Is there a possibility to sync with canvas calendars to be able to automatically have my assignments load in?

  **Feasible, M.** Canvas exposes a per-user iCal feed (Calendar, "Calendar Feed") as a plain HTTPS
  `.ics` URL, so no OAuth (field details from memory; verify against your feed).
  - Already in the tree: `reqwest`, an optional `icalendar` dep behind the `calendar-sync` feature, a
    stub worker (`src/sync/calendar.rs`) and `SyncConfig::calendar_sync_enabled`. The stub says CalDAV;
    a feed URL is simpler and enough.
  - Approach: `CANVAS_ICS_URL` in `.env`; poll on the same cadence as mail; map each VEVENT to a task
    with `deadline` = due time. Add `external_id` (feed UID) through an idempotent `ALTER` in
    `src/migrations.rs` and upsert on it, so re-polls never duplicate.
  - Risks: the URL is a secret token (never log it). The feed has no "submitted" state, so completion
    stays local. Removed assignments need a reconcile pass. All-day events carry a date, not a time;
    pick a default due hour.

- add support for all general vim motions in all views

  **Implemented (motions and search); the rest is out of scope.** Counts (`5j`), `gg`/`G`/`NG`, `0`/`$`
  (calendar day start/end), `Ctrl-d`/`Ctrl-u`, and `/` with `n`/`N` in the todo and email lists. They work
  in the todo list (and visual mode), the calendar, the email list and the message popup. Logic is in
  `src/app/motion.rs` and `src/app/search.rs`; key wiring and rules in [`src/tui/CLAUDE.md`](../src/tui/CLAUDE.md).
  - Decisions: `H`/`L` stay week-prev/next in the calendar; `u`, `m` and `d` keep their app meanings.
  - Still out of scope: registers, macros, undo/redo, text objects, popup cursor editing (each a subsystem).

- Email should live refresh, there should be a ability to press s and sync and be able to see the new emails populate or a message pop up that mentions all emails are gathered

  **Implemented.** The Email view reloads every 2s (KI-24). `s` spawns one sync (`App::start_email_sync`,
  never awaited in the handler) and reports "N new" or "All emails gathered" through `status_message`.
  The fetch, parse and store pipeline is now one function, `email::sync::sync_account`, shared by
  `s`, the background poll and `email sync`. Tests: `imap_*` scenarios against `fakeimap.py`.

- Email should have ai overview/summaries

  **Implemented.** Opening a message (`v`) asks Ollama for a summary in the background
  (`App::request_email_summary`); it is cached in `email_messages.summary` and shown above the body.
  A failure is not cached, so the next open retries; bodies of 200 characters or fewer get none. The
  prompt fences the email as untrusted data and the reply is capped. `TRIPTYCH_OLLAMA_URL` points the
  client at a stub in tests (`tests/tui/fakeollama.py`, [`TUI_FAKES.md`](./TUI_FAKES.md)).
  - Not done: a summary line in the list.

- Email should also prioritize and place higher priority emails higher up to be seen before the others

  **Implemented, heuristic only.** Display-time score in `src/email/priority.rs` (whole-word keywords
  in subject and snippet, bulk and no-reply senders lower, converted-to-task lower); `▲` high, `△`
  medium. The list sorts by score then newest, `o` toggles priority or date, and the cursor stays on its
  message by id. Nothing is stored, so tuning the keywords needs no migration.
  - Not done: LLM ranking (could share the summary call), sender history.

- Todo list should have the ability to create sub tasks as apart of one main major task

  **Feasible, L.** The flat `Vec<Task>` and the `selected` index are assumed everywhere (keys, visual
  range, scheduler, CLI `list`).
  - Schema: `parent_id INTEGER REFERENCES tasks(id)` via idempotent `ALTER` (NULL default is allowed).
    Delete children with the parent explicitly; do not rely on a pragma.
  - Keep the flat list: `load_tasks` returns parent, then its children, and the UI indents by depth.
    Then `selected`, `j`/`k` and visual mode keep working unchanged.
  - Decisions: one level only; a parent is a container, so the scheduler and
    `reallocate_all_tasks` skip it and schedule the children; parent completes when all children
    do; children inherit the parent deadline unless set. Add a key (`A`, or Tab to indent) and a CLI
    flag. `item_order` reordering and `x`/`d` on a parent are the fiddly parts.

- On smaller views the calendar doesn't scale

  **Feasible, S-M. Reproduced** at 60x16: after 20 `j` presses the screen still shows 07am-10am (the
  cursor row is off screen, nothing scrolls) and headers truncate (`Thu 0`).
  - Vertical: `render_calendar_view` builds every hour row in a `Table` with no scroll offset. Track
    a top-hour offset in `App` and keep the cursor row inside the visible window (`TableState` offset).
  - Horizontal: below about 80 columns show a window of 3-5 days around the cursor (so `h`/`l` pan)
    with a 3-letter header, or shorten headers to `Mo 09`.

- When adding/scheduling tasks it fails and says that there is no task scheduled for the already rendered schedule

  **Not reproduced as worded; two likely causes found, S.** Needs the exact text and keys from you.
  - Most likely: press `s` on a todo (auto-schedule), open Calendar, press `s` (picker). It shows
    "No unscheduled tasks available." while the task is already drawn on the grid. Reproduced with the
    driver. The picker only lists tasks with `scheduled_at IS NULL` (`unscheduled_tasks`), so tasks
    placed by `s` or by calendar `a` never appear, and the popup gives no reason. Fix: say "All
    tasks are scheduled" and let the picker also offer rescheduling.
  - Second: `m`/`u` on an empty cell says "No scheduled task here" (`src/app/calendar.rs`), including
    on `[deepwork]` blocks that look "rendered" but hold nothing manual.
  - Related, unfixed: todo `s` only uses blocks whose type equals the task category, else any free
    7am-11pm hour (`find_next_available_slot`); scheduling into the past is allowed.
