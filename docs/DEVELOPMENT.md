# Development

Dev workflow, recent changes and open issues for Triptych. Module map and build/test/lint:
[`../CLAUDE.md`](../CLAUDE.md). Plans: [`roadmap.md`](./roadmap.md), [`roadmap-email.md`](./roadmap-email.md).
Everything older (changelog back to the first slice, per-slice email entries, the Rust idioms
audits, resolved KI-1..KI-32): [`DEVELOPMENT-ARCHIVE.md`](./DEVELOPMENT-ARCHIVE.md).

## Workflow

```
cargo build
cargo test      # tests/it/ (in-process, in-memory sqlite) + tests/it/cli.rs (spawns the binary)
cargo clippy --all-targets
cargo fmt --check
```

Run all four before considering a change done. Known Issues are resolved in place, never deleted.

End-to-end check (drives the real binary in sandboxes, ~1 min; see [`TUI_DRIVER.md`](./TUI_DRIVER.md)):

```
python3 tests/tui/tui_suite.py -j 4      # FAIL = regression; XFAIL = open Known Issue; XPASS = fixed, drop its marker
```

What the tests do not cover (merged line coverage, mutation survivors, pty-suite determinism): [`TEST_GAPS.md`](./TEST_GAPS.md).

When this file passes about 100 lines, move the oldest changelog entries to the archive.

## Changelog

- 2026-10-21 (`list --all`): `triptych list` now hides completed tasks; `--all`/`-a` shows them
  (`Commands::List { all }`, filtered in `cli/commands.rs`; `load_tasks` and the TUI are unchanged).
  Tests: `list_hides_completed_tasks_unless_all`, `list_with_only_completed_tasks_points_at_all`.
- 2026-10-21 (audit fixes): from a repo-wide read; each item has a test.
  - Rules: a `delete` rule acts only on mail that arrives after it (also the first rule; it used to
    sweep every stored message, a permanent server delete). `rule_applied` is 0 new, 1 checked,
    2 backlog; `create_rule` moves checked mail to 2 (unchecked too for `delete`), `run_email_rules`
    does new mail then the backlog, and `rule_fires` keeps `delete` out of the backlog. The Rules
    popup says so. This supersedes the "each new rule re-runs all rules" line in the entry below.
  - IMAP: `sync_folder` waits `CHUNK_PAUSE` (1s) between chunks, since each chunk logs in again; a
    message `parse_raw` rejects is logged with its UID instead of vanishing. A shared session is the
    real fix if a server still throttles.
  - Triage: the drain rests between passes as long as the pass took (at most 10s).
  - Daemon: `handle_client` drops a client that sends nothing for 5s (`READ_TIMEOUT`).
  - `import_schedule` resolves its database through `db_url()`, so it no longer writes to `triptych.db`.
  - `src/app/mail_auto.rs` now holds triage and rules, moved out of `mail.rs` (2,026 to 1,739 lines).
  - Tests: 363 `cargo test` tests; new scenario `email_rule_delete_new_only`, and
    `email_rule_archive_delete` adds its to-be-deleted mail after the rules (suite is 191, 188 gating).
- 2026-10-21 (email follow-ups): closes what the two entries below left open.
  - List: the cursor row is `Modifier::REVERSED`, so unread bold and per-span colors stay visible.
    Only the visible rows are built (`scroll_offset` in `ui/email.rs`).
  - Compose: accepting a suggestion appends `, `; `address_list` in `email/smtp.rs` trims a trailing
    `,`/`;` from `To:` and `Cc:`.
  - Rules: the popup says new rules also cover synced mail (see the audit entry above for `delete`).
  - Triage: `run_email_triage` loops passes until nothing is pending. `TriageDone.finished` ends the
    drain (empty backlog or a failure; the next sync retries) and only that report clears `triage_running`.
  - Daemon: the cache preload runs in a spawned task, so the socket accepts before it finishes.
  - IMAP: `fetch_new` returns at most `SYNC_CHUNK` (100) messages, the oldest after the cursor;
    `sync_folder` stores each chunk and moves the cursor before the next (up to 20 per call), so a
    timeout keeps the earlier chunks.
- 2026-10-21 (UX fixes): found by walking every key of the three views in a sandbox. Key-help bars
  fit 80x24 (`?: help`); `?` opens a per-view key reference (`src/tui/ui/help.rs`); todo `u` undoes the
  last delete (`App.deleted_tasks`); email `d` needs a second `d` (`App.armed_key`); the calendar opens
  on today's hour (`calendar_cursor_at`). Details in the archive.
- 2026-10-21 (perf): `DbWatch` (`src/app/watch.rs`) reads `PRAGMA data_version`, so the 2s tick
  reloads the task or email list only on a change or after a 60s floor; `thread_counts` builds every
  thread count in one pass.

## Known Issues

### Open

- **KI-33** (2026-10-20) `src/app/mail.rs` is still about 1,740 lines with a dozen jobs: triage and
  rules moved to `mail_auto.rs`, but sync and list filters, compose, drafts, delete/archive,
  attachments, snooze, folders and invites remain. Groups are listed in
  [`src/app/CLAUDE.md`](../src/app/CLAUDE.md). No behaviour is wrong; it is slow to read. Split the
  next group (compose and drafts is the largest) when touching it.

### Resolved

KI-1..KI-32 are fixed and covered by a passing test, except KI-29 (checked by hand only); their
entries are in the [archive](./DEVELOPMENT-ARCHIVE.md#resolved).

## Open Questions

- None blocking right now.
- Not covered by the suite: `H`/`L` edge behaviour in the calendar, and IMAP against a real server
  (`fakeimap.py` speaks only what `client.rs` sends: no OAuth, no partial-body fetch). Also
  unconfirmed: whether editing the deadline of an already-scheduled task should drop its existing
  allocation (observed, not judged a bug).
