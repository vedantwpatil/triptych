# Development Log

Dev workflow, changelog, known issues for Triptych. See [`../CLAUDE.md`](../CLAUDE.md) (module
map, build/test/lint), [`roadmap.md`](./roadmap.md), [`roadmap-email.md`](./roadmap-email.md).

## Workflow

```
cargo build
cargo test      # in-memory sqlite; tests in src/app.rs + pure-fn tests in src/ui.rs, src/email/, src/nlp/
cargo clippy
```

Run all three before considering a change done. Known Issues resolved in place, never deleted.

End-to-end check (drives the real binary in sandboxes, ~1 min; see [`TUI_DRIVER.md`](./TUI_DRIVER.md)):

```
python3 tools/tui_suite.py -j 4      # FAIL = regression; XFAIL = open Known Issue; XPASS = fixed, drop its marker
```

## Changelog

- 2026-09-19 (later): Fixed KI-1..KI-13 using the driver: each scenario reproduced its bug before the
  fix and passes after (suite: 82 scenarios, 0 XFAIL). Details and file references are under
  Resolved. Largest changes: `src/nlp/rules.rs` time/date parsing (KI-2, 8 new unit tests), the
  async deadline parse (KI-13), and removal of the fuzzy cache (KI-1). No known bugs remain open.
- 2026-09-19: Feature audit plus a headless TUI driver. Audited every shipped feature by running the
  real binary (CLI, daemon, TUI) in sandboxes and found 13 bugs, all filed under Known Issues
  (KI-1..KI-13) and none fixed yet. Built `tools/tuidrive.py` (detached pty session an agent can
  `send` keys to and read the screen from, plus `db`/`cli` against the same sandbox) and
  `tools/tui_suite.py` (81 scenarios: 57 pass, 24 are expected failures that reproduce the open
  issues). Docs: [`TUI_DRIVER.md`](./TUI_DRIVER.md), [`../tools/CLAUDE.md`](../tools/CLAUDE.md),
  and the `triptych-tui` skill. No Rust code changed. Two findings worth remembering: the NLP regex
  fast path returns 0.95 confidence on partial parses, so Ollama never gets a chance to fix them
  (KI-2); and a stale `triptych daemon` from an earlier test run was found holding the real
  `$TMPDIR/triptych.sock` (KI-3 lets that happen silently).
- 2026-09-18: Doc-drift + repo cleanup pass. `docs/roadmap-email.md`: dropped shipped items still
  listed as deferred/limitations (UIDVALIDITY tracking, "`.env` not auto-loaded" - `main.rs` calls
  `dotenvy::dotenv()`), replaced the removed `max_uid` with the real `store.rs` helper list
  (`SyncCursor` get/set, `delete_older_than`), added `run_email_migration` and the `v` body popup,
  and rewrote the `triptych.db` note (file deleted, was 0 bytes) into the CWD-relative `todo.db`
  gotcha. `docs/roadmap.md`: Email Slice 1 In progress -> done. Cleanup: `cargo clean` (12.2G),
  removed `.DS_Store`s, empty root `triptych.db`, stale `logs/` file, and stray `todo.db*` sets
  from `src/` and `src/sync/` (launched from wrong CWD; root `todo.db` untouched). Mistake worth
  noting: both stray sets shared basenames and were moved into one backup dir, so `src/sync/`'s
  overwrote `src/`'s 56K `todo.db` - that one is unrecoverable (untracked, not in Trash). Use
  distinct destination names or `mv -n` when batching same-named files.
- 2026-09-18: Clippy warn-level remediation, following up the deny-level fix in `624e9ae`
  (`nursery`/`pedantic` enabled at `warn` in `Cargo.toml`). Started at 67 tractable findings
  across 15 files after `cargo clippy --fix` mechanically resolved ~340; fixed each by hand.
  Pattern used per finding: real cast sites (`as i64`/`as u32`/`as usize` narrowing DB-derived
  or user-input-derived values) got `TryFrom::try_from(...).unwrap_or(fallback)` with a comment
  justifying why the fallback branch is unreachable in practice, not a silent truncation;
  `match`/`if let` over `Option`/`Result` that only branched two ways became
  `.map_or_else(...)`; `manual_let_else` sites became `let Some(x) = ... else { continue };`.
  Two cast patterns repeated 5-6x in one file each got a small shared helper instead of
  per-site fixes: `day_of_week_i32` (`src/app.rs`) and `elapsed_ms` (`src/nlp/parser.rs`) - both
  doc-commented with why the truncation branch can't hit. The UIDVALIDITY round-trip cast
  (`c.uid_validity as u32`) recurs at 4 call sites across `app.rs`/`sync/mail.rs`/
  `email/client.rs`/`main.rs`; fixed individually rather than a cross-module helper, since the
  sites are unrelated modules - not a good abstraction target. Structural pedantic/nursery lints
  with no real fix (`struct_field_names` on `Task.task_category`, which mirrors a DB column name;
  `struct_excessive_bools` on `SyncConfig`'s 4 independent per-worker flags, see
  [`src/sync/CLAUDE.md`](../src/sync/CLAUDE.md); `too_many_lines` on 5 functions that are each one
  linear sequence - an IMAP protocol round-trip, a render function, an NLP pipeline, a migration's
  idempotent-check list, a CLI subcommand dispatch) got targeted `#[allow]`s with a one-line
  rationale instead of forced splits. Also: `SyncDaemon::start` (`src/sync/daemon.rs`) was
  `async fn` returning `Result<Self>` despite never `.await`ing or failing in its own body (only
  `tokio::spawn`s workers) - now a plain `fn` returning `Self`, taking `config: &SyncConfig`
  instead of by value; `main.rs`'s one call site updated (dropped `.await` and the trailing `?`).
  `OllamaClient::build_prompt`/`parse_response` (`src/nlp/ollama_client.rs`) didn't touch `self` -
  now associated functions (`Self::build_prompt(input)`), called from `parse`. `{file:?}`/
  `{socket:?}` in user-facing `println!`/`eprintln!` (`main.rs`'s `schedule import`/`export`,
  `daemon.rs`'s startup/bind-failure messages) switched to `{}`/`.display()` - `Debug` on a
  `PathBuf` adds quotes and escapes that `Display` doesn't, and these are plain status lines, not
  diagnostic dumps. Separately, `arithmetic_side_effects`/`indexing_slicing` (both `nursery`) were
  dropped from `Cargo.toml`'s lint table entirely rather than fixed per-site - too broad a rewrite
  for the value, by user decision. Verified clean throughout: `cargo build --all-targets` (0
  errors, only the 2 pre-existing baseline warnings - `EmailMessage`'s unread fields, the `toml`
  crate's semver-metadata notice), `cargo clippy --all-targets` (same), `cargo test` (57 passed).
- 2026-09-18: Fetch/storage efficiency pass, triggered by a live sync that fetched the entire
  30,232-message mailbox (several multi-MB attachments included) instead of the intended
  25-message capped catch-up, then lost all 505 already-fetched messages when it hit the
  timeout (nothing persists until `fetch_new` returns `Ok` - a `tokio::time::timeout` on the
  in-flight future drops everything it had accumulated). Root cause: `INITIAL_SYNC_LIMIT`'s cap
  guard only checked `since_uid.is_none()`, not `since_uid == Some(0)` - a `last_uid = 0` cursor
  row (the default value backfilled by `src/migrations.rs`'s `ALTER TABLE ... ADD COLUMN last_uid
  ... DEFAULT 0` for any account that existed before that column did) looked identical to a
  legitimate incremental resume, so it searched `UID 1:*` uncapped. Fixed: cap condition is now
  `since_uid.is_none_or(|uid| uid == 0)`. Also reconciled a stale doc: `src/email/CLAUDE.md` said
  `FETCH_TIMEOUT` was 120s; the source has read 300s since this session started (doc not updated
  when the constant was last bumped) - corrected to match source. Four further changes, same
  session:
  1. **Selective fetch for oversized messages.** `client.rs::fetch_new_inner` now does a cheap
     `(RFC822.SIZE)` pre-fetch (sizes only, no bodies) before the real fetch, splitting the UID
     set into "small" (fetched as full `RFC822`, as before) and "large" (over
     `LARGE_MESSAGE_BYTES` = 1 MiB, fetched as `RFC822.HEADER` only - no body, no attachments).
     `RawMessage` is now `(uid, raw_bytes, header_only)`; `message::parse_raw` takes a
     `header_only` param and synthesizes a placeholder snippet/`None` body for those instead of
     parsing a body that was never fetched. All 3 duplicated callers (`src/sync/mail.rs`,
     `main.rs`'s `EmailCommands::Sync`, `App::sync_email_accounts`) updated to destructure and
     thread the extra tuple field through.
  2. **Batched inserts.** `store::insert_new` wrapped its per-row `INSERT OR IGNORE` loop in a
     single `sqlx::Transaction` (`pool.begin()`/`tx.commit()`) instead of one implicit
     transaction per row.
  3. **Lazy body load.** `store::get_recent` (used for the Email view's list, up to 100 rows) now
     selects `NULL AS body_text` instead of the real column - the detail popup is the only place
     a body is ever shown, and only one email at a time, so loading every row's full body just to
     render a subject-line list was pure waste. New `store::get_body(pool, id)` fetches one
     email's body on demand; `App::open_selected_email` calls it and patches the body into
     `self.emails` by id after `mark_selected_email_read`'s own refresh (which would otherwise
     re-null it).
  4. **Background sync logging moved off stderr.** `client.rs`/`src/sync/mail.rs`/
     `App::sync_email_accounts`'s connection/fetch/error messages were plain `eprintln!`, which
     punched text directly into the terminal mid-TUI-navigation since they fire from
     `tokio::spawn`ed background tasks with no relation to ratatui's raw-mode screen (user
     report: sync output "keeps showing up while I'm going through the UI"). Switched to
     `tracing::debug!/info!/warn!`; new `init_tracing()` in `main.rs` (using the `tracing`/
     `tracing-subscriber`/`tracing-appender` deps that were already in `Cargo.toml` but never
     wired up) routes them to a file instead - `$TMPDIR/triptych.log` by default, overridable via
     `TRIPTYCH_LOG_PATH`, filtered by `RUST_LOG` (defaults to `info`, so the per-connection
     `debug!` chatter is off unless asked for). `email sync`/`email list`'s own
     `println!`/`eprintln!` summary lines in `handle_cli_command` are untouched - that's a
     one-shot CLI command's direct, expected terminal output, not background noise.
  Rebuilt/clippy/tested clean after each change (57 tests, no new warnings beyond the
  pre-existing `EmailMessage` dead-field one).
- 2026-09-18: Fixed mail sync silently going stale after a Gmail-side UIDVALIDITY change (user
  report: "I've gotten more emails since 7/24, why is it not updating"). Root cause #1: `.env`
  wasn't being loaded at all in one code path (fixed first, unblocked diagnosis). Root cause #2,
  the deeper bug: IMAP only guarantees UIDs are stable within one UIDVALIDITY epoch (RFC 3501) -
  Gmail changed it server-side with no user action, silently invalidating every previously-stored
  UID, so the old `max_uid`-derived resume cursor kept searching from a UID that no longer meant
  anything in the new epoch. Fix, in 3 layers:
  1. `client.rs`'s `MailSource::fetch_new` now returns the mailbox's current `uid_validity`
     alongside messages, and compares it against the caller's stored cursor before trusting
     `last_uid` - a mismatch is treated as a first sync (`INITIAL_SYNC_LIMIT`-capped) instead of
     resuming from a stale UID.
  2. Discovered via a live second sync that comparing-before-trusting wasn't enough: `store::
     max_uid` derived the cursor from `MAX(uid)` over `email_messages`, but that table can hold
     rows from more than one UIDVALIDITY epoch at once (`insert_new`'s `INSERT OR IGNORE` dedups
     by `(account, message_id)`, not `uid`, so old-epoch rows are never purged on an epoch
     change) - its max kept silently returning the stale epoch's UID forever, reproducing the
     original bug on every sync after the first. Replaced with a self-contained cursor: new
     `email_sync_state(account, folder, uid_validity, last_uid)` table (`src/migrations.rs`,
     same idempotent-`ALTER TABLE` pattern as the rest of the post-initial-schema columns),
     written once per sync by `store::set_sync_cursor`/read by `store::get_sync_cursor`. `store::
     max_uid` removed entirely (verified no other callers via `grep -rn max_uid`).
  3. `uid_validity`/`last_uid` moved from same-typed `(u32, u32)`/`(i64, i64)` tuples to a named
     `SyncCursor { uid_validity, last_uid }` struct (`src/email/store.rs`) - same bug class
     (UID/epoch confusion) as root cause #2, so tuple-of-same-type felt worth eliminating rather
     than risk swapping the two fields at a call site later. Also fixed an edge case found in
     self-review: if the epoch changed but zero messages came back that round, persisting a
     synthetic `last_uid = 0` would produce `since_uid = Some(0)` next time - bypassing
     `INITIAL_SYNC_LIMIT`'s cap, which only applies when `since_uid` is `None`. Now skips the
     cursor write in that specific case, leaving the stale-but-safe cursor in place so the next
     sync retries the same capped catch-up.
  All 3 duplicated call sites updated in lockstep (`src/sync/mail.rs::sync_mail`,
  `main.rs`'s `EmailCommands::Sync`, `App::sync_email_accounts`) - see `src/email/CLAUDE.md`'s
  "three independent callers" gotcha. Verified against the real mailbox: first post-fix sync
  correctly did a capped 25-message catch-up and revealed the true new-epoch UID range
  (30752-30776) coexisting with old-epoch rows up to 42978, directly confirming the UIDVALIDITY-
  change hypothesis. A later verification run then hung for 10+ minutes with an ESTABLISHED-but-
  silent TCP connection (`lsof` confirmed), reproduced on retry - migrations completed instantly
  both times, so the stall was inside the IMAP session itself, not the new cursor logic. A raw
  `openssl s_client` TLS handshake to `imap.gmail.com:993` completed fast with a valid cert
  chain, ruling out a network/DNS/firewall problem; most likely Gmail-side throttling from the
  repeated rapid automated logins this debugging session generated. Exposed a real gap either
  way: `client.rs::fetch_new` had no timeout, and since `mail_sync_worker` awaits it sequentially
  inside one `tokio::select!` branch, an unbounded hang there blocks every future tick for every
  account and starves the `shutdown_rx` poll too - not just a failed sync. Fixed by wrapping the
  connect-through-logout sequence in `tokio::time::timeout(FETCH_TIMEOUT, ...)`
  (`FETCH_TIMEOUT = 120s`; body moved to a private `fetch_new_inner`, `fetch_new` is now a thin
  timeout wrapper). Confirmed working: rerunning `email sync` against the same stalled account
  now fails cleanly after 120s (`"IMAP sync for 'default' timed out after 120s"`) instead of
  hanging indefinitely.
- 2026-09-18: Fixed the Email view freezing on `m`/Tab/Esc - reported as "doesn't seem to be
  functional/accepting action key presses". Root cause was the tradeoff called out in the entry
  below: `App::toggle_to_email` awaited `sync_email_accounts` synchronously in the key handler,
  so a slow/unreachable IMAP server blocked the TUI's redraw loop (no draw, no key input) for
  the entire per-account TCP+TLS round-trip. `sync_email_accounts` (`src/app.rs`) now
  `tokio::spawn`s the fetch→parse→store work instead of awaiting it; `toggle_to_email` returns
  immediately after kicking it off and showing whatever's already in the DB via `refresh_emails`.
  Per-account failures move from `status_message` to `eprintln!` (see `src/email/CLAUDE.md`),
  since there's no `&mut App` left once the task is spawned. Also added: email body viewing.
  `email_messages` gained a `body_text` column (`src/migrations.rs`, idempotent `ALTER TABLE`
  like the rest of that table's post-initial-schema columns); `message::parse_raw` now extracts
  it via `mail-parser`'s `body_text(0)` (full text/plain part, or HTML-to-text if that's all the
  message has - same conversion `body_preview` already used for the snippet, just without
  collapsing line breaks). New `v` key in the Email view (`src/keys.rs::handle_email_key`) opens
  a detail popup (`render_email_detail_popup`, `src/ui.rs`, same `centered_rect` pattern as the
  BlockForm/TaskPicker popups) showing from/date/body, scrollable with `j`/`k`, closed with
  `Esc`/`v`; opening it also marks the email read (`App::open_selected_email`).
- 2026-09-18: Email view now syncs IMAP before showing cached mail, instead of only refreshing
  from the local DB. `App::toggle_to_email` (`src/app.rs`) calls new `sync_email_accounts`
  (max_uid → fetch_new → parse_raw → insert_new per configured account, same shape as
  `EmailCommands::Sync`) before `refresh_emails`, so `m`/Tab/Esc into the Email view always
  pulls current mail rather than whatever the last 60s background poll or manual `email sync`
  left cached - mail previously only synced while the TUI was the open process (see Known
  Issues), which had left a real inbox stuck 8 weeks stale. No-ops silently if email isn't
  configured; per-account failures land in
  `status_message` rather than aborting the view. Tradeoff: this `.await`s synchronously in the
  key handler, so it blocks the TUI's redraw loop until the IMAP round-trip finishes - see
  `src/email/CLAUDE.md`'s new gotcha if that needs to become non-blocking later.
- 2026-09-18: Fixed calendar view swallowing `m`/`u`/`e`/`d` feedback. `render_calendar_view`
  (`src/ui.rs`) never rendered `app.status_message` - unlike `render_todo_view`/
  `render_email_view`, which both do - so error paths like "No scheduled task here" (pressing
  `u`/`m`/`e` on an empty cell) or "Can't move a deadline allocation directly" set the message
  correctly in `App` but nothing ever showed it. Looked identical to the key doing nothing.
  Added a status-line chunk (`Constraint::Length(3)`, same 3s-fade pattern as the other two
  views), shown only in `CalendarInputMode::Navigate` since popups (`n`/`s`/`a`) cover the area
  anyway. Root-caused by driving the compiled binary through a real pty (`expect`, isolated
  sandbox via `DATABASE_URL`/`TRIPTYCH_SOCKET_PATH`) rather than just reading `keys.rs` -
  dispatch logic for every calendar key was already correct, confirming `n`/`s`/`a` open their
  popups and `u` sets its status text; the gap was purely the missing render call.
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
- 2026-09-17: `install_panic_hook()` in `main.rs` — a panic mid-TUI used to skip the raw-mode/
  alt-screen teardown, breaking the terminal. Hook restores it before the panic prints.
- 2026-09-17: Fixed calendar same-hour task collision. `src/ui.rs`: `find_task_display`/
  `build_cell_content`/`get_cell_text` → `cell_task_displays` (collects every task sharing an
  hour, not just the first) + `build_cell_view` → `CellView{headline, overflow, style}`; 2-line
  cell, dim `+N more`. Added `ORDER BY ..., id` tiebreakers (`get_scheduled_tasks_internal`,
  `get_week_allocations_internal`). First `ui.rs` `mod tests` (3 cases).
- 2026-09-17: Current-hour row accent. `today_accent()` (`src/ui.rs`) shared by header + now-row:
  `▸` prefix + accent on the time label, `UNDERLINED` patched onto today's column at that hour
  (before the selection override). No accent when the displayed week doesn't contain today.
- 2026-09-17: Reallocation conflicts now say why. `ConflictReason::{BeyondWindow, OutOfCapacity}`,
  `TaskConflict.reason`, `AllocationResult::conflict_summary()` (`src/app.rs`) — one wording
  shared by the TUI status message and CLI `schedule reallocate`. `ALLOCATION_WINDOW_DAYS` is now
  a named const. +7 tests.
- 2026-09-17: Fixed stale multi-account docs (`src/email/CLAUDE.md`, `src/sync/CLAUDE.md`) —
  described old `EmailConfig::from_env() -> Option`; code is `all_from_env() -> Vec`. Reworded
  `mail.rs`'s misleading "TRIPTYCH_EMAIL_ENABLED not set" log line.
- 2026-09-17: Fixed 2 panics in `nlp/rules.rs` found by the idioms audit below: (1) 6 sites did
  `.and_local_timezone(Local).unwrap()`, panicking on a DST transition — now route through
  `app::resolve_local_datetime`. (2) `"eom"` called `.with_month()` before `.with_day(1)`,
  panicking on e.g. Jan 31 → "Feb 31" — reordered. +2 tests.
- 2026-09-17: Fixed both remaining calendar Known Issues. `allocate_task_to_blocks` (`src/app.rs`)
  now writes each allocation's own start/end (`block.start_time + Duration::minutes(used)`), not
  the block's — de-stacks tasks sharing one block. New `pub(crate) allocation_covers_hour` (range
  check, shared by `app.rs::cell_tasks` and `ui.rs::cell_task_displays`) so allocations render
  across every hour they span, not just their start. Deleted the now-unused `find_cell_task`/
  `App::task_at_cell`. Added `App::stack_index` + `cycle_stack_next`/`_prev` on `[`/`]`, reset on
  every cursor/week/view-entry move, so `m`/`u`/`e` reach any task in a stacked cell. Overflow
  text `"+N more"` → `"position/total"`. +2 tests, 2 updated (44 total).

## Rust Idioms Audit (2026-09-17)

Ran the `rust-idioms` skill checklist against `src/`. 2 bugs found — see Resolved below. Reviewed
clean, no action taken:

- `.clone()` (34 sites): 10 `Arc` (cheap), 24 owned at real ownership boundaries.
- Errors: `nlp/` has 2 real enumerate-shape errors (`OllamaError`, `ParseError`); everywhere else
  correctly erases to `sqlx::Error`/`anyhow::Error` since callers never branch on cause.
- `&Vec`/`&String` params, nested smart pointers, `unsafe`: 0 hits.
- Global state: 1 `std::sync::Once` (`email/client.rs`), narrowly scoped.
- Lifetimes: only `ui.rs`'s `CalendarGrid<'a>`/`cell_task_displays<'a>`, justified (avoids
  cloning the cached week data every frame).
- Blocking-in-async: `email/client.rs`, `nlp/ollama_client.rs` both genuinely async. Not
  exhaustively verified.
- Derives: `App` has none; every field is `Debug`+`Clone`-able, so `#[derive(Debug)]` would be
  free. Low priority, not filed.

## Rust Idioms Audit (2026-09-18)

Ran the `rust-idioms` skill against this session's UIDVALIDITY cursor fix (`client.rs`,
`store.rs`, `migrations.rs`, `sync/mail.rs`, `app.rs`, `main.rs`) plus the earlier-session
email-body-popup code (`message.rs`, `ui.rs`). 2 issues found and fixed during the fix itself
(see Changelog: the `SyncCursor` struct, the `last_uid = 0` edge case) - no further issues in
those files on re-review. `message.rs`/`ui.rs` reviewed clean: `Context`/let-else throughout
(no panics on malformed mail), iterator-based body rendering (`body.lines().map(Line::from)`,
zero-copy), `sqlx::Row::try_get` chosen over tuple `FromRow` to avoid relying on an unverified
sqlx API surface. `SyncCursor` derives `Debug, Clone, Copy, PartialEq, Eq` but not `Default` -
deliberate, not an oversight: a "default cursor" (validity 0, uid 0) has no sensible meaning
since epoch 0 never occurs on real IMAP servers.

## Known Issues

### Open

None. Every 2026-09-19 audit finding (KI-1..KI-13) is fixed and covered by a passing suite scenario.

### Resolved

Found in the 2026-09-19 audit; fixed the same day, each verified with its `tools/tui_suite.py` scenario
(named at the end of each entry).

- **KI-1** Fuzzy NLP cache returned the wrong item ("Call dad tomorrow" saved as "Call mom" once
  similarity passed 0.85). Removed the Jaro-Winkler layer and the `strsim` dependency; only exact
  matches hit the cache. `daemon_distinct_tasks`, `todo_fuzzy_cache`.
- **KI-2** NLP date/time/duration gaps. The regex parser treated each phrase as a full timestamp, so
  "tomorrow at 3pm" lost the time and "3pm-5pm" lost its length. `src/nlp/rules.rs` now has separate
  `Date`, `TimeOfDay` and `TimeRange` segments that `assemble` merges. Added `on`/`for` prefixes,
  `12/25`-style dates (a bare `1/2` is read as a fraction unless prefixed with `on`), and am/pm
  inheritance for ranges (`2-4pm`). Bare numbers ("pages 5-7", "look at 5 things") stay in the
  title. `extract_task_fields` (`src/app.rs`) now stores an Event's end - start as its duration.
  The ten `nlp_*` scenarios.
- **KI-3** A second `triptych daemon` silently took over the socket. `start_daemon` now exits if a
  health check on the existing socket succeeds; a stale socket is still replaced.
  `daemon_second_instance`.
- **KI-4** `triptych add ""` inserted an empty task. Empty input now exits 1 in the CLI, and
  `App::add_task` and the daemon's `add_task_to_db` reject it too. `cli_add_empty`.
- **KI-5** A block ending before it started was accepted. `validate_time_range` (`src/app.rs`) now
  guards `create_schedule_block` and `schedule import`. `sched_end_before_start`, `cal_block_backwards`.
- **KI-6** Block-form errors were invisible. The form popup (`src/ui.rs`) now has a fifth row that
  shows `status_message` in red. `cal_block_error_visible`.
- **KI-7** CLI output noise. Migration and startup banners (`src/migrations.rs`, `App::build`) now go
  through `tracing`, so stdout and stderr carry only command output. `cli_quiet_output`.
- **KI-8** `triptych stop` with no daemon printed two errors. `stop_daemon` now returns one error
  when no daemon answers, and `status` exits 1 in that case too. `daemon_stop_no_daemon`.
- **KI-9** `schedule show` omitted the week's task allocations. `print_schedule_summary` now ends
  with an "Allocated tasks:" section. `sched_show_allocations`.
- **KI-10** Email detail popup scroll was unclamped. `render_email_detail_popup` (`src/ui.rs`) clamps
  the offset using `Paragraph::line_count`, which needs ratatui's `unstable-rendered-line-info`
  feature (enabled in `Cargo.toml`). Its count includes block borders. `email_detail_scroll`,
  `email_detail_long_scroll`.
- **KI-11** NLP `eprintln!`s wrote into the TUI alt-screen. `src/nlp/parser.rs` now logs through
  `tracing`. `email_stray_output`.
- **KI-12** Pressing Enter twice on a converted email created a duplicate task.
  `convert_selected_email_to_task` now returns early when the email already has a `task_id`.
  `email_convert_twice`.
- **KI-13** The TUI froze for up to 15s while NLP waited on Ollama during a deadline edit.
  `submit_deadline_edit` is now synchronous: it closes the popup, shows "Parsing deadline...", and
  spawns the parse, which reports back over `App::deadline_rx`. `run_app` (`src/main.rs`) selects on
  that channel and calls `apply_deadline_parse`. `cal_deadline_no_freeze`.

- DST-transition panic in `nlp/rules.rs` (6 `.and_local_timezone().unwrap()` sites). Fixed
  2026-09-17.
- `"eom"` month-rollover panic in `nlp/rules.rs`. Fixed 2026-09-17.
- Terminal left broken after a panic mid-TUI. Fixed 2026-09-17.
- Calendar same-hour task collision hid tasks silently. Fixed 2026-09-17.
- Calendar had no current-time indicator. Fixed 2026-09-17.
- Deadline reallocation conflicts didn't say why. Fixed 2026-09-17.
- Stale multi-account docs (`email`/`sync` `CLAUDE.md`). Fixed 2026-09-17.
- Calendar grid actions only reached the first task of a stacked cell. Fixed 2026-09-17.
- Deadline allocations only rendered in their block's start hour. Fixed 2026-09-17.
- `App::build()` ignored `DATABASE_URL`, hardcoded to `sqlite:todo.db`, inconsistent with
  `import_schedule.rs`. Fixed 2026-09-18 (see Changelog).
- `triptych add` printed no task ID outside the daemon fast-path. Fixed 2026-09-18.
- `triptych stop` left a stale socket file (`std::process::exit` skipped cleanup). Fixed
  2026-09-18.
- README.md's `schedule.toml` example used field names that don't match the code
  (`day_of_week`/`start_time`/`end_time`/`block_type` vs. actual `day`/`start`/`end`/`type`),
  so copy-pasting it failed to import. Fixed 2026-09-18.
- Calendar view (`render_calendar_view`, `src/ui.rs`) never rendered `app.status_message`,
  so `m`/`u`/`e`/`d` error feedback (e.g. "No scheduled task here") was silently dropped -
  looked like the key did nothing. Fixed 2026-09-18 (see Changelog).
- Mail only ever synced while the interactive TUI was the running process (`SyncDaemon`'s
  `mail_sync_worker` is spawned in `main.rs`'s no-subcommand branch only - the `triptych
  daemon` socket process never starts it). Entering the Email view showed whatever the last
  TUI session or manual `email sync` had cached, which could be arbitrarily stale. Fixed
  2026-09-18 by syncing on view entry (see Changelog).
- Entering the Email view froze the whole TUI (no redraw, no key input accepted) until the
  synchronous IMAP sync added by the fix above finished or timed out - the sync-on-entry fix
  traded staleness for a blocking-in-async bug. Fixed 2026-09-18 by making the sync
  fire-and-forget (see Changelog).
- No way to read an email's body in the TUI - only a 200-char snippet was stored, and nothing
  rendered it beyond the list. Fixed 2026-09-18: `body_text` column + `v` detail popup (see
  Changelog).
- Mail sync silently stopped picking up new mail after a Gmail-side UIDVALIDITY change (backlog
  since 7/24 never synced). Fixed 2026-09-18: UIDVALIDITY-aware `SyncCursor` in new
  `email_sync_state` table, replacing the old `MAX(uid)`-over-`email_messages` cursor that broke
  permanently across an epoch change (see Changelog for the full 3-layer fix).
- `client.rs::fetch_new` had no timeout - a stalled/throttled IMAP server could hang the awaiting
  task forever, and since `mail_sync_worker` awaits each account sequentially inside one
  `tokio::select!` branch, that also blocked every future tick for every account and starved
  shutdown. Fixed 2026-09-18 with a `tokio::time::timeout` (now 300s) around the whole connect-
  through-logout sequence (see Changelog).
- First-sync cap (`INITIAL_SYNC_LIMIT`, 25 messages) only checked `since_uid.is_none()`, so a
  migration-backfilled `last_uid = 0` cursor row silently bypassed it and fetched the entire
  mailbox (live-observed: 30,232 messages, several MB of attachments, lost entirely on the
  resulting timeout). Fixed 2026-09-18: cap condition is now `since_uid.is_none_or(|uid| uid ==
  0)` (see Changelog).
- Fetch/storage inefficiency: every message got a full `RFC822` fetch regardless of size
  (attachments included, though never stored), `insert_new` ran one implicit transaction per row,
  and every row in the Email view's list carried its full body even though only one is ever shown
  at a time. Fixed 2026-09-18: size-gated selective fetch (`LARGE_MESSAGE_BYTES`), batched insert
  in one transaction, list/detail `body_text` split (see Changelog).
- Background sync (`tokio::spawn`ed tasks: `mail_sync_worker`, `App::sync_email_accounts`,
  `client.rs`) wrote straight to stderr via `eprintln!`, corrupting the TUI's display mid-
  navigation since it bypasses ratatui's alternate-screen buffer. Fixed 2026-09-18: converted to
  `tracing::debug!/info!/warn!`, routed to a file (`$TMPDIR/triptych.log` by default) via new
  `init_tracing()` in `main.rs` (see Changelog).

## Open Questions

- None blocking right now.
- Not covered by the suite: real IMAP sync (email scenarios seed rows directly), and `H`/`L` edge
  behaviour in the calendar. Also unconfirmed: whether editing the deadline of an already-scheduled
  task should drop its existing allocation (observed, not judged a bug).
