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
- **Slice 2 (planned)**: true IMAP IDLE (push instead of poll).
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
- **Slice 5 (partially done)**: SMTP send/reply/forward, real delete and archive/folder-move are
  done (`roadmap-email.md`'s own Slice 2, Slice 5 and Slice 6 — that doc's slice numbers track a
  separate, finer-grained sequence than this file's); snooze triage, enhanced NLP parsing and
  auto-scheduling integration for email-derived tasks remain planned.
- **Slice 6 (done)**: unified inbox with AI-driven triage. Shipped as a binary Focused/Other
  split (Outlook's Focused Inbox) rather than a ranked score, using the existing Ollama
  integration (`src/nlp/ollama_client.rs`'s `triage`) — no new LLM dependency. Distinct from
  Slice 5's keyboard triage (archive/snooze are manual actions; this is automatic classification,
  unrelated to either). See `docs/roadmap-email.md`'s Slice 17.

Goal: Superhuman-like email productivity in the terminal, integrated with task/calendar
workflows — not a standalone mail client bolted on.

## Other Planned Features

Carried over from the README roadmap; no implementation work started on any of these yet.

- **CalDAV calendar sync** — two-way sync with external calendars (Google Calendar, iCloud,
  etc.) via CalDAV. Note: `icalendar` crate is already an optional dependency
  (`calendar-sync` feature) but unwired — check `src/sync/calendar.rs` before starting, it has
  a `calendar_sync_worker` stub marked "Not yet finished" in `src/sync/daemon.rs`.
- **Canvas assignment sync** — not built. Poll Canvas's per-user iCal feed and upsert assignments as
  todo tasks; design and risks in [`future-features.md`](./future-features.md) (first entry).
- **Todo list in the calendar** — plan to integrate the todo list into the calendar view. Not a
  priority right now; nothing started.
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
