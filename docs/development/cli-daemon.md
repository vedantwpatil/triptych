# CLI and daemon history

One-shot commands (`add`, `list`, `done`, `rm`, `clear`, `schedule`), the Unix-socket daemon, and the
database path. Newest first.
Hub, workflow and open issues: [`../DEVELOPMENT.md`](../DEVELOPMENT.md). Code: `src/cli.rs`,
`src/cli/commands.rs`, `src/cli/daemon.rs` (map in [`../../CLAUDE.md`](../../CLAUDE.md)).
Related: [`testing.md`](./testing.md), [`canvas-notify.md`](./canvas-notify.md).

## Changelog

- 2026-10-27 (`.env` outside the repo): `triptych canvas sync` run from `$HOME` said `Canvas not
  configured` although `DATABASE_URL` (exported in the shell) found the data: `dotenvy::dotenv()` only
  looks in the cwd and its parents. With none found, `run()` now loads the `.env` beside the database
  file. Test sandboxes point `DATABASE_URL` at their own dir, so they never reach the real one.

- 2026-10-21 (`list --all`): `triptych list` now hides completed tasks; `--all`/`-a` shows them
  (`Commands::List { all }`, filtered in `cli/commands.rs`; `load_tasks` and the TUI are unchanged).
  Tests: `list_hides_completed_tasks_unless_all`, `list_with_only_completed_tasks_points_at_all`.
- 2026-10-21 (audit fixes, daemon and database): `handle_client` drops a client that sends nothing for
  5s (`READ_TIMEOUT`). `import_schedule` resolves its database through `db_url()`, so it no longer
  writes to `triptych.db`.

## Resolved issues

Indexed in [`../DEVELOPMENT.md`](../DEVELOPMENT.md#known-issues).

- **KI-31** (2026-10-20) `insert_parsed_task` and the daemon's `add_task_to_db` each held a copy of the
  `tasks` INSERT (tags JSON, category, default duration, 11 columns); a new column had to be added in
  both. Both now call `insert_task_row`. Covered by the existing add tests and
  `the_daemon_takes_a_request_larger_than_one_read`.
- **KI-30** (2026-10-20) With `DATABASE_URL` unset the app opened `sqlite:todo.db` relative to the
  working directory, so running it from another folder silently made a second, empty database. It now
  prints the absolute path when it creates one. `a_new_default_database_says_where_it_was_created`.
- **KI-28** (2026-10-20) The CLI daemon and its client each did a single `read` into an 8 KB buffer, so
  a request or response over 8 KB failed to parse, and the client had no timeout. Both now read to EOF
  (capped at 1 MiB) and the client gives up after 120s. The daemon now waits for the client's EOF
  before parsing; the only in-tree client half-closes. A client that never closes pins one daemon
  task until restart (a silent client already did before).
  `the_daemon_takes_a_request_larger_than_one_read`.
- **KI-3** A second `triptych daemon` silently took over the socket. `start_daemon` now exits if a
  health check on the existing socket succeeds; a stale socket is still replaced.
  `daemon_second_instance`.
- **KI-4** `triptych add ""` inserted an empty task. Empty input now exits 1 in the CLI, and
  `App::add_task` and the daemon's `add_task_to_db` reject it too. `cli_add_empty`.
- **KI-7** CLI output noise. Migration and startup banners (`src/migrations.rs`, `App::build`) now go
  through `tracing`, so stdout and stderr carry only command output. `cli_quiet_output`.
- **KI-8** `triptych stop` with no daemon printed two errors. `stop_daemon` now returns one error
  when no daemon answers, and `status` exits 1 in that case too. `daemon_stop_no_daemon`.

### Earlier fixes (not numbered)

- `App::build()` ignored `DATABASE_URL`, hardcoded to `sqlite:todo.db`, inconsistent with
  `import_schedule.rs`. Fixed 2026-09-18 (see Changelog).
- `triptych add` printed no task ID outside the daemon fast-path. Fixed 2026-09-18.
- `triptych stop` left a stale socket file (`std::process::exit` skipped cleanup). Fixed
  2026-09-18.
