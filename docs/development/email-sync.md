# Email sync history

IMAP sync, IDLE push, folder discovery and browsing, sync cursors, fetch efficiency. Newest first.
Hub, workflow and open issues: [`../DEVELOPMENT.md`](../DEVELOPMENT.md). Code:
[`../../src/email/CLAUDE.md`](../../src/email/CLAUDE.md), [`../../src/sync/CLAUDE.md`](../../src/sync/CLAUDE.md).
Related: [`email-messages.md`](./email-messages.md), [`email-triage.md`](./email-triage.md), [`../TUI_FAKES.md`](../TUI_FAKES.md).

## Changelog

- 2026-10-21 (audit fixes, IMAP): `sync_folder` waits `CHUNK_PAUSE` (1s) between chunks, since each chunk
  logs in again; a message `parse_raw` rejects is logged with its UID instead of vanishing. A shared
  session is the real fix if a server still throttles.
- 2026-09-29 (6-month mail load): first sync now searches `SINCE` 180 days (was `ALL`, last 25),
  capped at the newest 1000 (`INITIAL_SYNC_DAYS`/`INITIAL_SYNC_LIMIT`, `src/email/client.rs`); Email list
  loads up to 1000 rows (`EMAIL_LIST_LIMIT`). Existing accounts keep their cursor, so run
  `triptych email sync --backfill` once (clears `email_sync_state`, dupes skipped by `insert_new`).
  Fake IMAP accepts `SEARCH SINCE`. Also: `v` in a calendar cell opens the popup for blocks too.
- 2026-09-27 (folder discovery via IMAP LIST, Slice 16): Slice 13's folder browsing only ever
  reached the two folders `EmailConfig` names (`imap_folder`, `archive_folder`) — a real mailbox's
  "Sent", "Drafts", "Junk" or any custom folder was invisible no matter how much mail was in it.
  New `MailSource::list_folders` (`src/email/client.rs`) sends RFC 3501 `LIST "" *` on a one-shot
  connection (same shape as every other `MailSource` method) and drops any name flagged
  `\Noselect` (a hierarchy-only node) — confirmed against the vendored `async-imap`/`imap-proto`
  source that `\Noselect` is the server's literal wire token before writing the filter. New
  `sync::sync_one_folder(pool, config, folder)` reuses the same fetch/parse/store/cursor pipeline
  as `sync_account`'s per-folder pass, just for a folder neither `imap_folder` nor `archive_folder`
  names. `App::open_folder_browser` (`B` in the email list) spawns a background LIST pass across
  every configured account into a new popup (`folder_browser_open`, `discovered_folders`,
  `render_folder_browser_popup`); `j`/`k`/Enter/Esc navigate, pick, sync (via
  `App::browse_to_selected_folder`, spawned) and set the folder filter, or close without picking.
  New scenario `imap_tui_folder_browse` (156 TUI scenarios, was 155) seeds an empty "Sent" folder
  and a `\Noselect`-flagged "Hidden" one via a new `Mailbox.add_folder(name, noselect=False)` on
  `tests/tui/fakeimap.py` (which also gained a real `LIST` dispatch arm — inserted before the
  `not self.selected` gate, since real LIST needs no prior SELECT, unlike every other command that
  fake server had implemented so far), and asserts Sent and INBOX both appear, Hidden does not, and
  picking Sent both syncs it and narrows the view. No new Rust unit tests — `list_folders` only
  ever talks to a real IMAP server, exercised at the TUI-scenario level like `sync_account` and
  `idle_wait` before it. Full gate green throughout (`cargo build/clippy/test` unchanged at 180
  tests; TUI suite 156/156). See [`roadmap-email.md`](../roadmap-email.md)'s Slice 16,
  `src/email/CLAUDE.md`'s `client.rs`/`sync.rs` rows, `src/app/CLAUDE.md`'s `mail.rs` row,
  `src/tui/CLAUDE.md`'s `keys.rs`/`ui.rs` rows, `tests/tui/CLAUDE.md`'s `fakeimap.py` row.
- 2026-09-27 (IMAP IDLE / push-based mail sync, Slice 15): replaced `src/sync/mail.rs`'s shared
  fixed-60s-interval poll loop with RFC 2177 `IDLE` per account — `mail_sync_worker` now spawns one
  concurrent `account_sync_loop` per configured account (was one sequential loop for all accounts;
  a single blocked IDLE call now occupies its own connection for up to `IDLE_TIMEOUT`, 300s, so
  accounts can no longer share a loop without one delaying every other account's sync). New
  `MailSource::idle_wait(folder, timeout) -> Result<IdleOutcome>` (`src/email/client.rs`, `IdleOutcome`
  = `NewData`/`Timeout`) opens IDLE on a fresh connection, same one-shot-per-call shape as every
  other `MailSource` method; either outcome (or an `idle_wait` error) triggers an ordinary
  `sync_account` pass, since IDLE only proves *something* changed, never what.
  Caught a real regression before shipping, via the TUI scenario suite (not `cargo test`, which
  stayed green throughout): an open IDLE connection on `imap_folder` wakes the instant an archive
  action removes a message from that folder — archiving *is* exactly the kind of INBOX change IDLE
  watches for. Since `sync_account` had always swept `archive_folder` too (Slice 13), that wake's
  resulting sync pass rediscovered the just-archived message as new-to-that-folder and silently
  reinserted the row `archive_selected_email` had just deleted — `imap_tui_archive`,
  `imap_tui_archive_popup`, `imap_tui_archive_no_move`, and `imap_tui_folder_filter` all failed with
  a leftover `email_messages` row. Root-caused by reading the fake IMAP server's `commands.log` from
  a captured failing run (confirmed the background loop's post-wake sync visiting `Archive` and
  re-fetching the moved UID). Fixed by giving `sync_account` an `include_archive: bool` parameter:
  `false` for the background worker (the only *ambient* caller — an IDLE wake or the backstop
  timeout is never something the user asked for), `true` for every *explicit* caller (`s`, view
  entry, `triptych email sync`) — so Slice 13's folder-browsing-a-just-archived-message-back-in stays
  a deliberate act, not an unavoidable side effect of push-based sync. (An earlier fix attempt made
  `archive_selected_email` advance the archive folder's own sync cursor at archive time instead —
  correct for stopping ambient resurrection, but it also permanently blocked the *legitimate*
  explicit resync `imap_tui_folder_filter` depends on, since cursor state is shared between ambient
  and explicit callers with no way to tell them apart after the fact; reverted in favor of the
  `include_archive` split, which distinguishes them at the call site instead.)
  New scenario `imap_idle_push` (155 TUI scenarios, was 154) drives mail arriving while the TUI sits
  untouched on the To-Do view and asserts it's picked up via a real blocked IDLE connection (checks
  `commands.log` for `IDLE` then `DONE`), not a poll tick. `tests/tui/fakeimap.py` gained IDLE/DONE
  protocol support (a side thread polls the mailbox file and pushes one unsolicited `* n EXISTS` on a
  count change, while the main thread blocks on a plain `readline()` for `DONE` — a timed socket read
  doesn't mix cleanly with repeated timeout-then-retry on a buffered socket file object in CPython,
  worked around by never setting a read timeout at all). No new Rust unit tests — `idle_wait` is
  exercised only at the TUI-scenario level, same as the rest of the background sync workers. See
  [`roadmap-email.md`](../roadmap-email.md)'s Slice 15, `src/email/CLAUDE.md`'s `client.rs`/`sync.rs`
  rows, `src/sync/CLAUDE.md`'s `mail.rs` row and Pattern section, `tests/tui/CLAUDE.md`'s
  `fakeimap.py` row.
- 2026-09-27 (folder browsing, Slice 13): mail archived out of INBOX (Slice 6) no longer vanishes
  from Triptych. `sync_account` (`src/email/sync.rs`) now runs a second pass over
  `config.archive_folder` after `imap_folder`, each with its own `email_sync_state` cursor (already
  keyed `(account, folder)`, no migration needed) — the archive pass's error is caught and
  `tracing::warn!`ed rather than propagated, so an account whose server has no such folder (or
  nothing archived into it yet) still syncs INBOX exactly as before. `MailSource`'s four methods
  (`fetch_new`/`delete`/`archive`/`fetch_attachments`) all take an explicit `folder: &str` now
  instead of assuming `config.imap_folder`, since a loaded message's actual folder decides which
  mailbox gets `SELECT`ed before acting on its UID — also a latent correctness fix, since any
  non-INBOX message's delete/archive/attachment-fetch would previously have silently targeted the
  wrong mailbox once such messages could be loaded at all. `App.folder_filter` +
  `App::cycle_folder_filter` (`F` in the email list) mirror Slice 11's account filter exactly, over
  new `email::store::distinct_folders`; `refresh_emails` retains on it right after the account
  filter, before snooze visibility and the sort. The list tags any row whose folder isn't `"INBOX"`
  with `[folder]`, and the title shows `· folder: {name}` while a filter is active. New
  `tests/it/app.rs` cases cover the folder retain and the cycle-through-and-back-to-merged round
  trip (179 in-process tests, was 177); new scenario `imap_tui_folder_filter` archives a message,
  resyncs it back in from a real second `SELECT`ed mailbox, and drives `F` end-to-end (153 TUI
  scenarios, was 152) — needed `tests/tui/fakeimap.py` to grow genuine (if minimal) multi-mailbox
  `SELECT` support: a non-INBOX name now succeeds once a prior `MOVE`/`COPY` has tagged something
  for it in `mailbox.json["archived"]`, and still fails `NO [NONEXISTENT]` otherwise, so every
  account that never archives anything keeps its old single-cursor behavior (confirmed by rerunning
  `imap_first_sync_cap`/`imap_incremental`/`imap_uidvalidity_reset`, which assert exactly one
  `email_sync_state` row, and the 3 pre-existing archive scenarios). See
  [`roadmap-email.md`](../roadmap-email.md)'s Slice 13, `src/app/CLAUDE.md`'s `mail.rs` row,
  `src/email/CLAUDE.md`'s `client.rs`/`sync.rs`/`store.rs` rows, `src/tui/CLAUDE.md`'s key-binding
  table, `docs/TUI_FAKES.md`.
- 2026-09-23 (KI-22..24, IMAP harness): a manual task now shows in every hour it spans (KI-22); an idle
  IMAP sync no longer refetches the last message (KI-23); the Email view reloads while open (KI-24).
  New `tests/tui/fakeimap.py` (local TLS IMAP server) lets the suite run the real `email sync` and the TUI's
  background sync: 14 `imap_*` scenarios. The 2s reload briefly blanked the open body popup (`get_recent`
  rows carry no body); the existing `email_detail_long_scroll` caught it, so the tick skips the popup.
  Tests: 93 in-process, suite 110.
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

## Resolved issues

Indexed in [`../DEVELOPMENT.md`](../DEVELOPMENT.md#known-issues).

- **KI-24** (2026-09-23) New mail never appeared while the Email view stayed open: `toggle_to_email`
  spawns the sync and reloads the list at once, and nothing reloaded it when the sync (or the 60s poller)
  finished, so it showed only after leaving and re-entering. `run_app` now has a 2s tick that calls
  `refresh_emails` while the view is open and no body popup is up; `refresh_emails` keeps the cursor on
  the same message by id when newer mail lands above it. `imap_tui_live_refresh`, `imap_tui_view_sync`,
  `tests/it/app.rs` (cursor test).
- **KI-23** (2026-09-23) An idle sync reported "Synced 1 new email(s)" and refetched the newest message
  every time. RFC 3501: `UID n:*` always includes the highest UID even when it is below `n`, so a mailbox
  with nothing new answered `UID 6:*` with UID 5. `fetch_new_inner` now drops UIDs at or below the
  cursor. `imap_idle_sync`.

### Earlier fixes (not numbered)

- Mail only ever synced while the interactive TUI was the running process (`SyncDaemon`'s
  `mail_sync_worker` is spawned in `main.rs`'s no-subcommand branch only - the `triptych
  daemon` socket process never starts it). Entering the Email view showed whatever the last
  TUI session or manual `email sync` had cached, which could be arbitrarily stale. Fixed
  2026-09-18 by syncing on view entry (see Changelog).
- Entering the Email view froze the whole TUI (no redraw, no key input accepted) until the
  synchronous IMAP sync added by the fix above finished or timed out - the sync-on-entry fix
  traded staleness for a blocking-in-async bug. Fixed 2026-09-18 by making the sync
  fire-and-forget (see Changelog).
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
