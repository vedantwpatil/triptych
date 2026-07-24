# Triptych Roadmap

Formal, living roadmap across all planned features. Supersedes `future.md` (kept in place as
historical implementation notes for the scheduling work below, now largely shipped) as the
place to check current status before starting new work.

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

- **Slice 1 (in progress)**: single IMAP account via app-password, polling fetch, SQLite
  storage, read-only TUI list view, email→task conversion via the existing NLP pipeline.
- **Slice 2 (planned)**: true IMAP IDLE (push instead of poll).
- **Slice 3 (planned)**: OAuth2 for Gmail and other providers (needs external OAuth client
  setup — a user action, not something buildable unilaterally).
- **Slice 4 (planned)**: multi-account (`accounts` table, per-account config instead of env
  vars).
- **Slice 5 (planned)**: SMTP send/reply, keyboard-driven triage (archive, snooze), enhanced
  NLP parsing and auto-scheduling integration for email-derived tasks.

Goal: Superhuman-like email productivity in the terminal, integrated with task/calendar
workflows — not a standalone mail client bolted on.

## Other Planned Features

Carried over from the README roadmap; no implementation work started on any of these yet.

- **CalDAV calendar sync** — two-way sync with external calendars (Google Calendar, iCloud,
  etc.) via CalDAV. Note: `icalendar` crate is already an optional dependency
  (`calendar-sync` feature) but unwired — check `src/sync/calendar.rs` before starting, it has
  a `calendar_sync_worker` stub marked "Not yet finished" in `src/sync/daemon.rs`.
- **Recurring tasks** — repeat rules for tasks (daily/weekly/custom), distinct from the
  existing recurring *schedule blocks* (TOML-defined).
- **Full-text search** — search across task descriptions (and, once built, email bodies).
  Likely SQLite FTS5 virtual table given the existing SQLite-only storage layer.
- **Desktop notifications** — OS-level notifications for deadlines/reminders; needs a
  platform-notification crate, decision on daemon vs TUI as the trigger point.
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
