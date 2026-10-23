# Development

Dev workflow, recent changes and open issues for Triptych. Module map and build/test/lint:
[`../CLAUDE.md`](../CLAUDE.md). Plans: [`roadmap.md`](./roadmap.md), [`roadmap-email.md`](./roadmap-email.md).
Full history is split by feature in [`development/`](./development/); every file links back here.

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

Keep this file under about 100 lines. Write a change's full entry (and a fixed KI) in the matching
feature file below, then leave one line here under Recent changes.

## Feature history

| File | Covers |
| --- | --- |
| [`todo.md`](./development/todo.md) | Task list: order, urgency badges, colours, recurrence, reword, caret, visual delete |
| [`calendar.md`](./development/calendar.md) | Weekly grid, cell popup, to-do panel, blocks, allocation |
| [`nlp.md`](./development/nlp.md) | Regex and Ollama task parsing, cache, warmup, latency |
| [`email-sync.md`](./development/email-sync.md) | IMAP sync, IDLE, folders, cursors, fetch efficiency |
| [`email-messages.md`](./development/email-messages.md) | List and detail, compose, drafts, delete/archive, attachments, threads, snooze |
| [`email-triage.md`](./development/email-triage.md) | LLM triage and summaries, rules, categories, filters, search, email to task |
| [`canvas-notify.md`](./development/canvas-notify.md) | Canvas assignment sync, deadline alerts, LaunchAgent |
| [`cli-daemon.md`](./development/cli-daemon.md) | One-shot commands, Unix-socket daemon, database path |
| [`tui-ux.md`](./development/tui-ux.md) | Key help, confirmations, undo, 80x24 layout, reload tick |
| [`testing.md`](./development/testing.md) | Test suites, pty driver, fakes, mutation survivors, coverage |
| [`code-health.md`](./development/code-health.md) | Module splits, lint and idiom passes, audits, docs refreshes |

## Recent changes

- 2026-10-23 (new-mail alerts): grouped desktop alert per background mail sync (unread, non-bulk, under 6h).
  [`canvas-notify.md`](./development/canvas-notify.md).
- 2026-10-22 (docs): the changelog, Known Issue entries and the old archive moved into
  [`development/`](./development/) by feature; no entry was dropped.
- 2026-10-21 (`list --all`): `triptych list` hides completed tasks; `--all`/`-a` shows them.
  [`cli-daemon.md`](./development/cli-daemon.md).
- 2026-10-21 (audit fixes): rule/triage backlog, IMAP chunk pause, daemon read timeout, `import_schedule`
  database path, `mail_auto.rs` split. [`email-triage.md`](./development/email-triage.md),
  [`email-sync.md`](./development/email-sync.md), [`cli-daemon.md`](./development/cli-daemon.md),
  [`code-health.md`](./development/code-health.md).
- 2026-10-21 (email follow-ups): cursor row, compose separator, chunked IMAP sync, drained triage.
  [`email-messages.md`](./development/email-messages.md).
- 2026-10-21 (UX fixes, perf): `?` help, delete undo, confirmed email delete, `DbWatch` reload.
  [`tui-ux.md`](./development/tui-ux.md).

- 2026-10-22 (binary size): release build 21.0 to 9.3 MiB (`[profile.release]`, reqwest without `http2`).
  [`code-health.md`](./development/code-health.md).

## Known Issues

### Open

- **KI-33** (2026-10-20) `src/app/mail.rs` is still about 1,740 lines with a dozen jobs: triage and
  rules moved to `mail_auto.rs`, but sync and list filters, compose, drafts, delete/archive,
  attachments, snooze, folders and invites remain. Groups are listed in
  [`src/app/CLAUDE.md`](../src/app/CLAUDE.md). No behaviour is wrong; it is slow to read. Split the
  next group (compose and drafts is the largest) when touching it.

### Resolved

KI-1..KI-32 are fixed and covered by a passing test, except KI-29 (checked by hand only). Each entry
sits under "Resolved issues" in its feature file:

- [`nlp.md`](./development/nlp.md): KI-1, 2, 11, 13, 14, 19, 20, 21
- [`todo.md`](./development/todo.md): KI-15, 16, 17, 18
- [`calendar.md`](./development/calendar.md): KI-5, 6, 9, 22
- [`cli-daemon.md`](./development/cli-daemon.md): KI-3, 4, 7, 8, 28, 30, 31
- [`email-sync.md`](./development/email-sync.md): KI-23, 24
- [`email-messages.md`](./development/email-messages.md): KI-10, 27
- [`email-triage.md`](./development/email-triage.md): KI-12
- [`canvas-notify.md`](./development/canvas-notify.md): KI-26, 29
- [`testing.md`](./development/testing.md): KI-25, 32

## Open Questions

- None blocking right now.
- Not covered by the suite: `H`/`L` edge behaviour in the calendar, and IMAP against a real server
  (`fakeimap.py` speaks only what `client.rs` sends: no OAuth, no partial-body fetch). Also
  unconfirmed: whether editing the deadline of an already-scheduled task should drop its existing
  allocation (observed, not judged a bug).
