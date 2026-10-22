# TUI UX and performance history

Cross-view polish: key help, confirmations, undo, layout at 80x24, and the 2s reload tick. Newest first.
Hub, workflow and open issues: [`../DEVELOPMENT.md`](../DEVELOPMENT.md). Code:
[`../../src/tui/CLAUDE.md`](../../src/tui/CLAUDE.md). Related: [`todo.md`](./todo.md), [`calendar.md`](./calendar.md),
[`email-messages.md`](./email-messages.md).

## Changelog

- 2026-10-21 (UX fixes): found by walking every key of the three views in a sandbox.
  - Titles: the key-help bars clipped or collided at 80x24. Each is now the main keys plus `?: help`;
    the email `sorted by` label moved to `title_bottom`; the deadline and calendar-input popups are
    70% wide. Scenarios that matched the old titles (`New deadline`, `Cell (j/k`, `Ctrl-D: save draft`)
    were updated.
  - `?` opens a per-view key reference (`src/tui/ui/help.rs`, `App.help_open`); any key closes it.
  - Todo `x`/`d`/`D` now say what they deleted and `u` undoes the last delete (`App.deleted_tasks`,
    `App::undo_delete`, original ids). Undo keeps one step, resets the alert tier, and its
    `Restored` message can be replaced by an allocation warning.
  - Email `d` is a permanent server delete, so it needs a second `d` (`App.armed_key`,
    `confirm_delete_selected_email`); another key disarms. `x` says `Snooze cleared` or `Not snoozed`.
  - Block form `Enter` on an empty title says `Title required`. Status messages are cleared on a view
    change. The calendar opens on today's hour (`calendar_cursor_at`); cell detail shows tags as `#work`
    instead of raw JSON. `Ctx.cal()` in the suite resets to Mon 07am so older scenarios keep counting.
  - Tests: 355 `cargo test` tests; 6 new scenarios (suite is 188, 185 gating). Left open then, fixed in
    the follow-up entry in [`email-messages.md`](./email-messages.md): the cursor row hid read/unread contrast, contact autofill had no
    separator, the Rules popup did not say rules are retroactive.
- 2026-10-21 (perf): the 2s tick reloaded the task or email list even when nothing had changed.
  - `DbWatch` (`src/app/watch.rs`) reads `PRAGMA data_version` on a held connection (~10µs against a
    10-30ms reload); `run_app` reloads only on a change or after a 60s floor (snooze expiry and urgency
    follow the clock). `tests/it/db_watch.rs` covers both.
  - `thread_counts` builds every thread count in one pass instead of a scan per row;
    `load_tasks` reads the clock once per sort; `has_any` in `email/priority.rs` reuses one buffer.
  - Left open then, fixed in the follow-up entry in [`email-messages.md`](./email-messages.md): the first IMAP sync was not chunked, the triage
    loop was not drained, the daemon preload ran before the accept loop, and the email list rendered
    every row.
