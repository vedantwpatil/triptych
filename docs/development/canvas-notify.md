# Canvas and alerts history

Canvas assignment sync (iCal feed, links, titles, delete guard) and deadline alerts (TUI worker, `notify`
subcommands, LaunchAgent). Newest first.
Hub, workflow and open issues: [`../DEVELOPMENT.md`](../DEVELOPMENT.md). Code:
[`../../src/sync/CLAUDE.md`](../../src/sync/CLAUDE.md). Related: [`todo.md`](./todo.md), [`cli-daemon.md`](./cli-daemon.md).

## Changelog

- 2026-09-30 (canvas links): Canvas tasks carry links. New `tasks.links` (JSON array, `add_links`
  migration): the assignment page built from the feed `URL` (course and assignment ids), then the
  `href`s in `X-ALT-DESC`. `canvas::upsert` refreshes them on every known task, finished or not. Todo
  `o` opens the first link, `O` lists them (`App.links_open`, `app/links.rs`, popup in `ui/todo.rs`,
  `1`-`9` pick). `⇗N` marks tasks with N links past the first. Opener is `open`/`xdg-open`, or
  `TRIPTYCH_OPEN_CMD`; the suite sets it to `true`, scenario `canvas_links_open_pick` swaps in a logger.
  Open: popup rows are bare URLs; anchor text would read better.
- 2026-09-30 (alerts with the TUI closed): `triptych notify check|install|uninstall`. `check` is one
  pass of `notify::check_once` (no `App::build()`, so no Ollama probe each minute: `app::open_pool`);
  `install` (`src/notify/agent.rs`, macOS) writes `~/Library/LaunchAgents/com.triptych.notify.plist`
  (`StartInterval` 60, `RunAtLoad`, `WorkingDirectory` = cwd for `.env`, `DATABASE_URL` pinned absolute
  because launchd's cwd is `/`) and loads it with `launchctl bootstrap`; it refuses when no database file
  exists, since an agent started elsewhere would create an empty one. `due_alerts` now claims a task with
  an atomic `UPDATE ... WHERE notified_tier < ?`, so the TUI worker and the agent never double-alert
  (`concurrent_checks_alert_each_task_once`). Tests: `tests/it/notify.rs`, `tests/it/cli.rs`
  (`notify_*`); the installer itself is not run by tests (it needs the real launchd). Open: Linux has no
  installer (cron/systemd timer by hand); the agent does not sync Canvas, so Canvas deadlines stay as
  fresh as the last TUI or `canvas sync` run. Suite: `todo_add_cold_model` fails at `-j 4` here with
  or without this change (2s budget; passes alone with `--only todo_add_cold_model -j 1`): load, not a regression.
- 2026-09-29 (canvas delete guard): tasks with `external_id` (Canvas sync) cannot be deleted by `x`/`d`/`D`, visual range delete, `rm` or `clear`; the feed would re-add them. TUI shows "Canvas task: cannot delete", range delete reports how many were kept, `rm` errors.
- 2026-09-29 (alerts, canvas titles): desktop deadline alerts (`src/notify.rs`, `sync/notify.rs`; new
  `tasks.notified_tier`, reset when a deadline moves). Canvas titles become `CS-472: Quiz 3`
  (`canvas::tidy_title`; old untouched titles are rewritten on re-poll). Todo ties sort by soonest
  deadline. New `docs/SETUP.md` and `docs/USAGE.md`. The suite sets `TRIPTYCH_NOTIFY_CMD=true` so no real
  alerts fire; scenario `notify_deadline_alert` overrides it with a logger.
- 2026-09-29 (canvas): Canvas assignment sync. `src/canvas.rs` parses the per-user iCal feed
  (`CANVAS_ICS_URL`) and upserts VEVENTs as tasks keyed on new `tasks.external_id` (unique partial
  index, added in `src/migrations.rs`). `sync/canvas.rs` polls every 15 min inside the TUI;
  `triptych canvas sync` runs it once. TUI list now refreshes on the 2s tick. Tests: `tests/it/canvas.rs`,
  two in `tests/it/cli.rs`, scenario `canvas_feed_syncs_into_todo`. Limits: assignments removed from
  the feed are not deleted; a re-poll updates only `deadline` of open tasks; imported tasks are not
  auto-allocated until `schedule reallocate`; all-day dates become 23:59 local.

## Resolved issues

Indexed in [`../DEVELOPMENT.md`](../DEVELOPMENT.md#known-issues).

- **KI-29** (2026-10-20) `canvas::fetch_feed` had no timeout, so an unresponsive server hung the Canvas
  worker and `triptych canvas sync`. Now 30s. Checked by hand (a local listener that
  accepts and never answers: the sync failed after 30s); no automated test, the suite has no such fake.
- **KI-26** (2026-10-20) A deadline alert whose notifier failed (bad `TRIPTYCH_NOTIFY_CMD`, no
  `osascript`) was lost: `notified_tier` was set before `send`, which only logged. A failed send now
  restores the tier, so the next pass retries. `triptych notify check` now reports the failure
  on stderr instead of "No alerts due". A permanently broken notifier retries (and logs) every pass.
  `notify_check_retries_an_alert_whose_notifier_failed`.

