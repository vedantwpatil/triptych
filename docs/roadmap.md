# Triptych Roadmap

Formal, living roadmap across all planned features. Supersedes `future.md` (kept in place as
historical implementation notes for the scheduling work below, now largely shipped) as the
place to check current status before starting new work. See [`DEVELOPMENT.md`](./DEVELOPMENT.md)
for the dev changelog and known issues, and [`../CLAUDE.md`](../CLAUDE.md) for the module map.

Status legend: **Done** / **In Progress** / **Planned**

## Smart Task Scheduling — Done

Deadline-driven allocation of tasks into deepwork/admin blocks, with reallocation on change.
Implemented per `future.md`'s plan:

- `tasks.deadline`, `tasks.duration_minutes` columns; `task_block_allocations` table
  (`src/migrations.rs`).
- Reallocation algorithm and calendar drag-move / deadline editing (`src/app.rs`, `src/ui.rs`).
- Calendar grid shows deadline-allocated tasks (see recent commits: "show deadline-allocated
  tasks in the calendar grid", "drag-move tasks and edit deadlines from grid").

Remaining polish (not blocking, pick up opportunistically): edge cases in NLP deadline/duration
extraction, conflict-reporting UX when blocks are exhausted.

## Email Client — In Progress

Full detail and phase breakdown in [`docs/roadmap-email.md`](./roadmap-email.md).

- **Slice 1 (done)**: IMAP app-password fetch on a 60s poll, SQLite storage (UIDVALIDITY-aware
  `SyncCursor`, 180-day retention purge), TUI list view + `v` body popup, email→task
  conversion via the existing NLP pipeline. See `src/email/`.
- **Slice 2 (done)**: true IMAP IDLE (push instead of poll); `roadmap-email.md` Slice 15.
- **Slice 3 (planned)**: OAuth2 for Gmail and other providers (needs external OAuth client
  setup — a user action, not something buildable unilaterally).
- **Slice 4 (done)**: multi-account. Shipped as env-var config (`IMAP_ACCOUNTS=label1,label2`
  plus `IMAP_*_<LABEL>` suffixed vars per account, see `.env`) rather than the originally
  planned `accounts` table — no schema-editing UI exists yet, so a DB-backed accounts table
  would have needed one just to be usable; env vars fit the existing config convention (see
  `src/email/config.rs`). `email_messages` gained an `account` column (uniqueness now scoped
  to `(account, message_id)`, not just `message_id`); the TUI email view shows a merged inbox
  across all configured accounts, no account switcher. Revisit the `accounts` table if
  per-account state needs to live in the DB (e.g. sync cursors, enable/disable toggles from the
  UI).
- **Slice 5 (done)**: SMTP send/reply/forward, delete, archive/folder-move, snooze (`roadmap-email.md`
  numbers its own finer-grained slices). Email-derived tasks take a deadline from the body
  (`convert_selected_email_to_task`) and go through the same allocation as any task.
- **Slice 6 (done)**: unified inbox with AI-driven triage. Shipped as a binary Focused/Other
  split (Outlook's Focused Inbox) rather than a ranked score, using the existing Ollama
  integration (`src/nlp/ollama_client.rs`'s `triage`) — no new LLM dependency. Distinct from
  Slice 5's keyboard triage (archive/snooze are manual actions; this is automatic classification,
  unrelated to either). See `docs/roadmap-email.md`'s Slice 17.

Goal: email productivity in the terminal, integrated with task/calendar workflows, not a
standalone mail client bolted on. The email view is feature-complete enough for daily use; further
ideas (Outlook-style panes, quick steps, RSVP) sit in [`roadmap-outlook.md`](./roadmap-outlook.md) as
a backlog, not a goal. Core todo and calendar work comes first.

## Focus order

1. ~~Recurring tasks~~ — done (see below).
2. Email work only where it feeds tasks and the calendar.
3. Todo list inside the calendar view.
4. CalDAV sync, last (large; needs an external account to test).

## Other Planned Features

- **CalDAV calendar sync** — Planned, not started. `src/sync/calendar.rs` is a stub: `sync_calendar`
  only counts `events` rows and `calendar_sync_enabled` is hardcoded `false`. Canvas and email
  invites read iCalendar data but are not CalDAV (no server protocol, read-only).
- **Canvas assignment sync** — built (`CANVAS_ICS_URL`, `triptych canvas sync`). Open: reconcile
  assignments removed from the feed; details in [`future-features.md`](./future-features.md) (first entry).
- **Desktop notifications** — built: in-TUI worker plus `triptych notify install` (macOS
  LaunchAgent); see [`USAGE.md`](./USAGE.md).
- **Todo list in the calendar** — allocated tasks already show in the grid; a todo panel inside the
  calendar view is not started.
- **Recurring tasks** — built: `every day|weekday|week|month|<weekday>` in the task text; finishing one
  adds the next copy (`src/recurrence.rs`, `src/app/repeat.rs`, see [`USAGE.md`](./USAGE.md)). Open:
  `every 2 weeks`, several weekdays, an end date. Separate from the TOML recurring *schedule blocks*.
- **Full-text search** — search across task descriptions (and, once built, email bodies).
  Likely SQLite FTS5 virtual table given the existing SQLite-only storage layer.
- **Task dependencies** — blocking/blocked-by relationships between tasks, feeding into the
  scheduling algorithm's priority ordering.
- **Statistics dashboard** — a TUI view (new `ViewMode` variant, same pattern as `Calendar`/
  `Email`) surfacing completion rates, time-in-category, etc.

## How to use this doc

When starting a new feature, add a section here before writing code: what's done, what's
explicitly deferred, and which existing conventions (schema, sync daemon, `ViewMode`) it
should follow — see `docs/roadmap-email.md` for the level of detail expected. Keep this file's
top-level status current as slices land; move completed items into "Done" with a one-line
pointer to where the implementation lives instead of leaving them under "Planned".
