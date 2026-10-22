# Testing history

`cargo test` suites, the pty driver and scenario suite, fake servers, mutation survivors, coverage. Newest first.
Hub, workflow and open issues: [`../DEVELOPMENT.md`](../DEVELOPMENT.md). Code: [`../../tests/CLAUDE.md`](../../tests/CLAUDE.md),
[`../../tests/tui/CLAUDE.md`](../../tests/tui/CLAUDE.md). Related: [`../TEST_GAPS.md`](../TEST_GAPS.md),
[`../TUI_DRIVER.md`](../TUI_DRIVER.md), [`../TUI_FAKES.md`](../TUI_FAKES.md), [`cli-daemon.md`](./cli-daemon.md).

## Changelog

- 2026-10-21 (audit fixes, tests): 363 `cargo test` tests; new scenario `email_rule_delete_new_only`, and
  `email_rule_archive_delete` adds its to-be-deleted mail after the rules (suite is 191, 188 gating).
- 2026-10-20 (mutation survivors): 181 of the 186 surviving mutants are now killed; the 5 left are
  equivalent or platform-only, see [`TEST_GAPS.md`](../TEST_GAPS.md).
  - New `tests/it/{tasks,allocation,calendar}_edges.rs`, plus cases in `cli` (daemon, `schedule show`),
    `notify`, `email_smtp` (fake STARTTLS server), `canvas`, `recurrence`, `urgency`, `nlp_rules`.
  - `src/email/smtp.rs`: `send_line`, `read_reply`, `expect` and `build_mime` are `pub` for those tests.
  - `src/app/allocation.rs`: dropped a redundant capacity filter and the unused `block_has_capacity`.
    `src/nlp/rules.rs`: the unreachable `_ => 0s` arm of `parse_relative_duration` folded into `days`.
  - A range like `9:30-11` must end the input in a test: `parse_loose_time` eats a trailing space, so a
    following word fails `word_boundary` before the colon rule runs.

- 2026-10-20 (test gaps): the pty suite is repeatable and measurable.
  - `src/clock.rs`: `now()` / `local_now()` replace the direct `Utc::now()` / `Local::now()` reads in
    the app, TUI, CLI, parser and notify worker; `TRIPTYCH_NOW` (RFC 3339) freezes them. Unset, they
    are the system clock. The `email/` modules keep the real clock on purpose. Tested by
    `a_pinned_clock_decides_what_tomorrow_is` (`tests/it/cli.rs`).
  - `tests/tui/`: the suite pins `TRIPTYCH_NOW` for the binary, `fakeimap.py` and its own checks, closes
    the Ollama URL unless a scenario starts the fake, and moves the 3 real-Ollama scenarios behind
    `--real-ollama`. `notify_deadline_alert` seeded deadlines with SQLite `'now'`, so it failed against the
    pinned clock; it now seeds from the pinned instant. Two fixed sleeps became waits on the condition.
  - `TUIDRIVE_COVERAGE=1` quits the TUI before the kill so an instrumented binary writes its profile:
    merged coverage 51% to 87% of lines. First mutation batches: survivors listed in `TEST_GAPS.md`.

- 2026-09-29 (verification): `todo_add_cold_model` fails under `-j 4` on the pre-change commit too (3 of 3 runs),
  so it is not a regression from the sort/reword work. `todo_add_llm_nonblocking` failed once in 3 runs on
  the new code (22.6s, real Ollama) and 0 of 3 on the old; unexplained, watch it.
- 2026-09-27 (KI-25, TUI cursor-label regex): the full suite failed 3 calendar scenarios
  (`cal_cursor_moves`, `vim_cal_motions`, `vim_cal_input_literal`) with e.g. `got 'Mon ', want
  'Mon 07am'` — but only when run at the wall-clock hour matching the row under test (confirmed via
  `date`: `Sun Sep 27 07:16:54 EDT 2026`, hour 07, the exact row all three scenarios target).
  Root-caused with a standalone debug script driving `tuidrive.Sandbox`/`Term` directly and dumping
  the raw screen buffer: `src/tui/ui/calendar.rs` prefixes the current-hour row with `▸` (the "now"
  accent, intentional, added 2026-09-17), and when that row also falls under an open popup's `│`
  border, `tuidrive.py`'s `Term.cursor()` time-label regex (`r"\s*.?(\d\d[ap]m)"`) — which only
  tolerated one leading non-digit decorator character — failed to match with two (`│` then `▸`)
  stacked ahead of the digits, leaving the parsed label empty. Not a regression from Slice 10 (none
  of that work touches calendar rendering or motion routing) and not flaky — reproduced
  deterministically on every rerun at this hour. Fixed by widening the regex to `r"\s*.{0,2}?
  (\d\d[ap]m)"` (lazy, tolerates 0-2 decorator characters before the digits); all 3 scenarios and
  the full 150-scenario suite pass regardless of wall-clock time now.
- 2026-09-19 (later): Fixed KI-1..KI-13 using the driver: each scenario reproduced its bug before the
  fix and passes after (suite: 82 scenarios, 0 XFAIL). Details and file references are under
  the `## Resolved issues` sections of the feature files (KI index in [`../DEVELOPMENT.md`](../DEVELOPMENT.md#known-issues)). Largest changes: `src/nlp/rules.rs` time/date parsing (KI-2, 8 new unit tests), the
  async deadline parse (KI-13), and removal of the fuzzy cache (KI-1). No known bugs remain open.
- 2026-09-19: Feature audit plus a headless TUI driver. Audited every shipped feature by running the
  real binary (CLI, daemon, TUI) in sandboxes and found 13 bugs, all filed under Known Issues
  (KI-1..KI-13) and none fixed yet. Built `tests/tui/tuidrive.py` (detached pty session an agent can
  `send` keys to and read the screen from, plus `db`/`cli` against the same sandbox) and
  `tests/tui/tui_suite.py` (81 scenarios: 57 pass, 24 are expected failures that reproduce the open
  issues). Docs: [`TUI_DRIVER.md`](../TUI_DRIVER.md), [`../tests/tui/CLAUDE.md`](../../tests/tui/CLAUDE.md),
  and the `triptych-tui` skill. No Rust code changed. Two findings worth remembering: the NLP regex
  fast path returns 0.95 confidence on partial parses, so Ollama never gets a chance to fix them
  (KI-2); and a stale `triptych daemon` from an earlier test run was found holding the real
  `$TMPDIR/triptych.sock` (KI-3 lets that happen silently).
- 2026-09-18: Extended `tests/cli.rs` with 7 more tests (13 total) covering calendar/
  scheduling and email paths not yet exercised: `schedule reallocate` success (task with
  a deadline+duration fits a matching deepwork block) and conflict (`"1 out of block
  capacity"`, `"needs 120m, got 0m"`, reason text - all sourced from `AllocationResult::
  conflict_summary`/`ConflictReason` in `src/app.rs`, not guessed); CLI-level compound/
  group day import (`"weekdays"` + `"monday_wednesday_friday"` -> 8 blocks); the
  overlapping-block-skip warning path (second block on the same day/time is dropped, not
  imported, with a stderr warning); `schedule import --clear`; `email list` formatting
  against seeded rows (inserted directly into `email_messages` via a throwaway `sqlx`
  pool, bypassing IMAP) - read/unread marker, `(account)` tag, `from_name` vs. `from_addr`
  fallback, most-recent-first ordering; and `email sync` against a closed local port, as a
  fast, network-free way to exercise the per-account failure path without needing a real
  IMAP server. All source-verified by reading the actual code first (`src/app.rs`,
  `src/main.rs`, `src/nlp/rules.rs`, `src/migrations.rs`, `src/email/store.rs`) rather than
  guessed - one wrong guess caught along the way: the overlap warning prints the day name
  lowercase (`"on monday"`), not capitalized like `schedule show`'s display - not a bug,
  just two independent naming conventions for the same day-of-week int. No product code
  changed this round.
- 2026-09-18: Built `tests/cli.rs` - black-box integration tests that spawn the compiled
  `triptych` binary per-test (not an in-process `App` call), each in a throwaway sandbox dir
  passed as `DATABASE_URL`/`TRIPTYCH_SOCKET_PATH` env vars, so `cargo test` never touches the
  real `todo.db` or a live `$TMPDIR/triptych.sock`. Required two small, non-breaking fixes to
  make that isolation possible: `App::build()` (`src/app.rs`) now reads `DATABASE_URL` (was
  hardcoded to `sqlite:todo.db`, ignoring the env var entirely - see the DB gotcha below,
  updated); `src/daemon.rs`'s `socket_path()` now reads `TRIPTYCH_SOCKET_PATH`, falling back to
  the old `$TMPDIR/triptych.sock` default in both cases. Running this harness surfaced 2 real
  bugs, both fixed same session:
  - `triptych add` printed no task ID in the (default, no-daemon-running) direct-execution
    path - only the daemon fast-path did. `App::add_task` now returns the inserted row id
    (via `execute().await?.last_insert_rowid()`, not a separate `SELECT last_insert_rowid()`
    query, which would be unreliable against a pool since that value is connection-scoped) and
    `main.rs` prints it, matching the daemon path's wording. Callers that only cared about
    `Err` (`src/keys.rs`, the email-to-task conversion in `src/app.rs`) needed no changes.
  - `triptych stop` called `std::process::exit(0)` directly inside `handle_client`'s
    `Shutdown` arm, bypassing `start_daemon`'s own socket cleanup at the end of its accept
    loop - left a stale socket file on disk after every clean shutdown. Now removes the
    socket in that arm too before exiting.
  - Also fixed a real doc bug the harness's schedule-import test caught by using the *wrong*
    field names on purpose first: README.md's example `schedule.toml` used
    `day_of_week`/`start_time`/`end_time`/`block_type`, none of which match
    `BlockDefinition`'s actual fields (`day`/`start`/`end`/`type` - `future.md`'s example had
    it right). Copy-pasting README's example would fail to import. Fixed.

## Resolved issues

Indexed in [`../DEVELOPMENT.md`](../DEVELOPMENT.md#known-issues).

- **KI-32** (2026-10-20) 13 clippy warnings in `tests/it` (`clippy::assert_is_empty`, new in 1.99), so
  "the tree is warning-free" in `CLAUDE.md` was false. The lint's suggestion needs a type annotation at
  every call site, so it is allowed once in `tests/it/main.rs`.
- **KI-25** (2026-09-27) `tests/tui/tuidrive.py`'s `Term.cursor()` misread the calendar time label
  whenever the current-hour `▸` accent (`src/tui/ui/calendar.rs`) landed on a row also covered by an
  open popup's `│` border — the regex only tolerated one leading decorator character before the
  digits, not two stacked. Test-harness bug only, wall-clock-time-dependent, not a product bug; full
  details and fix in the Changelog above. `cal_cursor_moves`, `vim_cal_motions`,
  `vim_cal_input_literal`.
