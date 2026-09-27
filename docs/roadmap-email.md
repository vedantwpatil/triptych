# Email Client — Slice 1 (IMAP fetch → store → view → convert-to-task)

See [`DEVELOPMENT.md`](./DEVELOPMENT.md) for known issues and the changelog, and
[`roadmap.md`](./roadmap.md) for the overall feature roadmap.

## Context

README's roadmap lists a full email client (IMAP IDLE, OAuth2, multi-account, triage) as a
planned major feature. `.env` already scaffolds `IMAP_SERVER/PORT/USERNAME/PASSWORD/FOLDER`,
`SMTP_*`, and `TRIPTYCH_EMAIL_ENABLED` — confirming the intended auth path is a single
app-password IMAP account, not OAuth2 (OAuth2 needs an external Google Cloud OAuth client
this session can't provision).

This slice builds the first working vertical: fetch mail over IMAP, store it, view it in the
TUI, and convert an email into a task using the existing NLP/task pipeline — the reason this
lives in Triptych rather than as a standalone client. OAuth2, true IMAP IDLE, and snooze were all
originally explicitly deferred so later sessions wouldn't reinvent this scoping conversation;
snooze landed as Slice 12 and IMAP IDLE as Slice 15, leaving OAuth2 the only one still deferred
(see below). Multi-account landed later as Slice 4, send/reply/forward as Slice 2, and archive as
Slice 6 (see below) — this doc's title still says "Slice 1" for historical reasons but now covers
all of them.

## Conventions this slice follows

- **Schema evolution**: guarded ALTER/CREATE in `src/migrations.rs`'s `run_calendar_migration`/
  `run_email_migration` (checked via `column_exists`), both called from `main.rs` after
  `App::build()`. The
  `migrations/` dir has a single initial-schema `.sql` file untouched since — new schema goes
  into `migrations.rs`, not a new sqlx migration file.
- **Background work**: `src/sync/` — one file per service (`cache.rs`, `calendar.rs`,
  `ollama.rs`), each a `worker(pool, shutdown_rx: broadcast::Receiver<()>)` fn spawned
  conditionally in `SyncDaemon::start` based on a bool in `SyncConfig::from_env()`. Mail sync
  follows this: `src/sync/mail.rs` + `mail_sync_enabled` from `TRIPTYCH_EMAIL_ENABLED`.
- **Views**: `ViewMode` enum in `app.rs`, matched in `tui/ui.rs::ui()` for rendering and in
  `src/tui/keys.rs::handle_key_event` (declared via `mod keys;` in `tui.rs`, single dispatch
  entry point) for input.
- **Task creation**: `App::add_task(&mut self, description: &str)` (`app.rs`) runs the full NLP
  pipeline and inserts a `Task`. Email→task conversion calls this directly with the subject
  line rather than duplicating insert logic.
- **Crates**: `async-imap` (`default-features = false, features = ["runtime-tokio"]` — default
  is `runtime-async-std`), paired with `tokio-rustls` (project already pulls in rustls via
  sqlx's `runtime-tokio-rustls`) + a root-cert crate, since async-imap ships no TLS itself.
  MIME parsing via `mail-parser`.
- Active DB file is `sqlite:todo.db` (`DB_URL` const in `app.rs`, overridable via `DATABASE_URL`
  through `db_url()`). The relative path resolves against CWD, so run from the repo root - a
  stray `todo.db` gets created wherever the binary is launched otherwise.

## What's in this slice

1. **Deps** (`Cargo.toml`): `async-imap`, `tokio-rustls`, `rustls-native-certs`, `mail-parser`.
2. **Schema**: `email_messages` table (`uid`, `message_id`, `account`, `folder`, `from_addr`,
   `from_name`, `subject`, `date_utc`, `snippet`, `is_read`, `task_id` FK → `tasks(id)`),
   uniqueness on `(account, message_id)` — not `message_id` alone, since the same Message-ID
   can land in more than one account. No dedicated `accounts` table — see the multi-account
   note below.
3. **`src/email/` module**: `config.rs` (`EmailConfig::all_from_env`), `message.rs`
   (`EmailMessage` + `parse_raw`), `client.rs` (`MailSource` trait + `ImapMailSource`),
   `store.rs` (batched insert, list/body queries, mark-read, link-task, `SyncCursor`
   get/set, `delete_older_than` for the 180-day retention purge - `EMAIL_RETENTION_DAYS` in
   `app.rs`).
4. **Daemon wiring**: `src/sync/mail.rs::mail_sync_worker`, spawned from `SyncDaemon::start` when
   `mail_sync_enabled` — originally a 60s poll, now push-based IMAP IDLE per account (Slice 15).
5. **TUI**: `ViewMode::Email`, list view, `'m'` to toggle from the todo list, `Enter` to convert
   the selected email into a task, `r` to mark read, `v` to open a scrollable body popup.
6. **CLI**: `triptych email sync` / `triptych email list` for testing without the daemon/TUI.

## Multi-account (Slice 4, done)

Config: `IMAP_ACCOUNTS="label1,label2"` (comma-separated), then per-account vars suffixed
`_<LABEL>` (label upper-cased, non-alphanumeric → `_`), e.g. `IMAP_SERVER_WORK`,
`IMAP_USERNAME_WORK`, `IMAP_PASSWORD_WORK`, `IMAP_PORT_WORK`, `IMAP_FOLDER_WORK`. Unset
`IMAP_ACCOUNTS` and the legacy flat `IMAP_*` vars are read as a single account labeled
`"default"` — existing single-account `.env` files need no changes. See `.env` for a
commented example block.

Chose env vars over the originally-planned `accounts` table (`EmailConfig::all_from_env` in
`src/email/config.rs`) because there's no schema-editing UI to manage DB-backed accounts with
yet, and env vars match every other config value in this project. `mail_sync_worker`
(`src/sync/mail.rs`) spawns one concurrent sync loop per configured account (originally a shared
60s-tick loop; push-based per account since Slice 15), one `ImapMailSource` per account; a failure
syncing one account is logged and doesn't stop the others. The TUI email view
(`ui.rs::render_email_view`) is a merged inbox tagged with each message's account — no per-account
switcher/filter.

## Per-email AI summary (done, 2026-09-23, predates slice numbering)

Opening a message in the detail popup asks Ollama for a summary of `body_text`
(`App::request_email_summary`/`apply_summary`, `src/app/mail.rs`), cached in
`email_messages.summary` (idempotent `ALTER TABLE`, `src/migrations.rs`) so it's only generated
once per message; a failed summary is not cached and retries next open. Shipped before this doc
started numbering slices — listed here because it was previously (incorrectly) still marked
deferred below. Distinct from the still-deferred unified-inbox AI triage: this summarizes one
already-open message, not the whole inbox.

## Slice 2: send/reply/forward (done, 2026-09-27)

Backend: hand-rolled RFC 5321 SMTP client (`src/email/smtp.rs`), sharing `src/email/tls.rs` with
the IMAP client. `SmtpConfig::all_from_env`/`for_account` mirror `EmailConfig`'s multi-account
scheme, keyed by the same account label — so `SMTP_*_<LABEL>` vars pair with `IMAP_*_<LABEL>`.
`.env`'s previously-unused `SMTP_*` vars are now live.

UI: `App::email_compose: Option<ComposeState>` (`src/app/model.rs`) drives a new
`InputMode::EmailCompose`. `c` in the email list opens a blank compose; `R`/`A`/`F` in the detail
popup open a reply/reply-all/forward pre-filled with subject prefix, quoted original, and
RFC 5322 `References` threading. Ctrl-S sends in the background (`App::send_compose`, reported
over `send_rx`, same spawn-and-report pattern as mail sync); Esc cancels. See
`src/app/CLAUDE.md`'s `mail.rs` row and `src/tui/CLAUDE.md`'s `keys.rs`/`ui/email.rs` rows.

## Slice 3: starring and mark-unread (done, 2026-09-27)

`email_messages.is_starred` (idempotent `ALTER TABLE`, `src/migrations.rs`), `store::set_starred`/
`mark_unread` mirroring `mark_read`. `f` toggles the selected message's star
(`App::toggle_selected_star`, `src/app/mail.rs`); `u` marks it unread again
(`App::mark_selected_email_unread`) — useful since opening the detail popup auto-marks read, same
as most mail clients. Both bound in the email list and the detail popup, both re-run
`refresh_emails` so the list stays current. Starred is rendered as a yellow `●` before the subject
in the list and a "Starred" line in the detail popup — display-only, doesn't affect `priority.rs`'s
sort score.

## Slice 4: subject-based thread grouping (done, 2026-09-27)

`App::thread_count`/`normalize_subject` (`src/app/mail.rs`, pure, tested in
`tests/it/email_thread.rs`) group messages in the already-loaded merged inbox by subject with
`Re:`/`Fwd:`/`Fw:` prefixes stripped and case ignored. Rendered as a `[N]` badge before the subject
in the list and an "N messages in this thread" line in the detail popup, only when `N > 1`. At the
time this shipped it was local, subject-only grouping over `App::emails`, not a real
`References`/`In-Reply-To` conversation reconstruction — Slice 7, below, added that on top without
changing this function's signature.

## Slice 5: real IMAP delete (done, 2026-09-27)

`d` in the email list or detail popup permanently deletes the selected message
(`App::delete_selected_email`, `src/app/mail.rs`): `MailSource::delete(uid)` (`src/email/client.rs`)
does `UID STORE +FLAGS.SILENT (\Deleted)` then `EXPUNGE` against that message's own account
(`EmailConfig::for_account`, new), spawned and reported over `delete_rx` like send/sync — an
unreachable server must not freeze the TUI. The local row (`store::delete_email`) is only dropped
after the server confirms, so a network error never silently loses mail the server still has;
`apply_delete_result` closes the detail popup on success and leaves it open with a "Delete failed:
..." message otherwise. No confirmation prompt — this codebase has no confirm-dialog pattern
anywhere (todo delete is immediate too), so delete follows that same convention rather than
introducing a new one. Delete needed no multi-folder support, since `\Deleted`+`EXPUNGE` acts on the
already-configured folder — archive (which does need a destination folder) followed as Slice 6,
below; snooze remains deferred. Tested via `tests/tui/fakeimap.py`'s
new `UID STORE`/`EXPUNGE` support (`imap_tui_delete`, `imap_tui_delete_popup`).

## Slice 6: archive / folder-move (done, 2026-09-27)

`a` in the email list or detail popup archives the selected message (`App::archive_selected_email`,
`src/app/mail.rs`): `MailSource::archive(uid)` (`src/email/client.rs`) tries RFC 6851 `UID MOVE` to
`EmailConfig::archive_folder` (new field, `IMAP_ARCHIVE_FOLDER[_<LABEL>]`, defaults `"Archive"`)
first; a server that answers `BAD`/`NO` (no `MOVE` capability) falls back to the classical
`UID COPY` + `UID STORE +FLAGS.SILENT (\Deleted)` + `EXPUNGE` sequence MOVE is defined to be
equivalent to — same drain-carefully pattern as Slice 5's delete. Spawned and reported over
`archive_rx`, same shape as delete/send; the local row is only dropped (`store::delete_email`,
reused) after the server confirms. Does not auto-create the destination folder — a missing folder
surfaces as "Archive failed: ...", consistent with delete/send's error convention. No confirmation
prompt, same reasoning as delete. This is a move *out of* the tracked folder, not a real
multi-folder mailbox — there's still no folder browser/switcher, so an archived message simply
stops appearing in Triptych once moved; the archive folder's contents aren't synced or viewable
from the TUI. Tested via `fakeimap.py`'s new `UID MOVE`/`UID COPY` support and `disable_move()`
(`imap_tui_archive`, `imap_tui_archive_popup`, `imap_tui_archive_no_move`).

## Slice 7: `References`-header threading (done, 2026-09-27)

`App::thread_count` (`src/app/mail.rs`) now reconstructs real conversations first: a private
`MsgIdForest` union-finds every loaded message's own `Message-ID` together with each id in its
`References` header (already stored per-message as `references_header` since before this slice,
previously unused for grouping). Ancestors never fetched into `App::emails` still link their
children into one component, since the forest's keys are bare `Message-ID` strings, not indices
into `emails` — two replies to the same unfetched parent land in one thread even though that
parent itself has no row. Falls back to Slice 4's `normalize_subject` grouping only when the
target has no header link to anything currently loaded (no `References` header at all, or every
referenced id is likewise absent) — most first-in-thread messages, and mail from clients that
drop the header. `thread_count`'s signature and both UI call sites (the list's `[N]` badge, the
detail popup's "N messages in this thread" line) are unchanged; only the grouping logic underneath
changed. Not a full IMAP `THREAD`/`SORT` extension query or `In-Reply-To`-specific handling — RFC
5322 §3.6.4 already requires `References` to carry the full ancestor chain (including the
immediate parent `In-Reply-To` would separately name), so a compliant sender's `References` header
alone is enough. New `tests/it/email_thread.rs` cases: header chain wins over an unrelated subject,
two siblings link through a shared unfetched ancestor, and a header that links to nothing loaded
still falls back to subject grouping (158 in-process tests, was 155). No TUI-scenario or doc
changes needed beyond this entry — the fake IMAP server never sets `references_header` on synced
mail, so this is exercised at the unit level, not through `tui_suite.py`.

## Slice 8: attachments (done, 2026-09-27)

Metadata-only: attachment bytes are never persisted, only their filename/content-type/size, in a
new `email_attachments` table (`ON DELETE CASCADE` on `email_messages.id` — safe because
sqlx-sqlite's `SqliteConnectOptions` default is `PRAGMA foreign_keys = ON`, which this project never
overrides). `message.rs::parse_raw` extracts them at parse time via `mail-parser`'s
`Message::attachments()` (skipped when `header_only`, since there's no body to walk);
`store::insert_new` writes the rows in the same transaction as the message, gated on
`rows_affected() == 1` before trusting `last_insert_rowid()` (a dup-key `INSERT OR IGNORE` no-op
must not attach rows to someone else's message). `store::get_recent` gained an `EXISTS(...)`
subquery as `EmailMessage.has_attachments`, cheap and indexed, so the list view never loads
attachment rows it isn't showing.

Saving is on-demand and re-fetches over IMAP rather than reading a local copy: `s` in the detail
popup calls `App::save_selected_attachments` (spawned, same shape as delete/archive), which opens
one fresh connection via the new `MailSource::fetch_attachments(uid)` and writes every part to
`TRIPTYCH_ATTACHMENT_DIR/<email_id>/` (falls back to `$TMPDIR/triptych-attachments`), sanitizing
each `Content-Disposition: filename` first (`app::sanitize_filename` keeps only the final path
component — that header is attacker-controlled wire data, so a raw join risks `../../etc/passwd`-
style traversal). Metadata for the popup's "Attachments: ..." line and the list's `[attach]` tag is
cached per email id in `App.email_attachments`, populated on open the same way `email_summaries`
already is. New `tests/it/email_attachments.rs`: `parse_raw` against a handcrafted multipart
message extracts filename/content-type/size, `header_only` skips extraction, a plain message has no
attachments; `sanitize_filename` passes a plain name through, strips traversal to the last
component, and rejects an empty or traversal-only name (164 in-process tests, was 158).
`fakeimap.py`'s `make_message(..., attachment=True)` now builds a multipart PDF message; new
scenario `imap_tui_attachment_save` drives save-to-disk end-to-end (148 TUI scenarios, was 147).

## Slice 9: full-text search over email body (done, 2026-09-27)

`/` in the Email view previously only matched the in-memory list rows' subject and sender, because
`store::get_recent` deliberately never loads `body_text` into them (only `get_body` fetches it, on
demand, when a message is opened). New `email::store::search_body_matches(pool, needle)` runs one
`SELECT id FROM email_messages WHERE body_text LIKE ? ESCAPE '\'` (`%`/`_` in the query escaped so a
literal percent or underscore in a search term can't widen the match) and returns the matching ids
as a `HashSet`;
`App::search_step` unions that against the existing subject/sender check before walking the list
with `motion::find_match`, same wrap/not-found messaging as before. Only hits the DB for the email
view — the todo list's search is unchanged and stays purely in-memory.

Both `App::commit_search` and `App::search_step` are now `async` (the one DB round-trip), so
`keys.rs`'s three call sites (`Enter` in the search prompt, `n`/`N` in the todo and email views) all
gained an `.await`; `tests/it/app.rs`'s eight existing search tests needed the same, plus a new
`email_search_matches_body_text_via_db_query` test asserting a hit that exists only in `body_text`,
not the subject or sender (165 in-process tests, was 164). New scenario `vim_email_search_body`
seeds `seed_emails`' default per-row bodies (`Body line one <n>`) and searches a phrase present only
in one row's body, not its subject or sender (149 TUI scenarios, was 148).

## Slice 10: drafts (done, 2026-09-27)

`Ctrl-D` in the compose/reply/forward popup saves the current form to a new `email_drafts` table
(`CREATE TABLE IF NOT EXISTS`, `src/migrations.rs`) instead of sending, and closes the popup;
`D` in the email list opens a drafts popup (`render_drafts_popup`, `src/tui/ui/email.rs`) listing
every saved draft, most-recently-updated first (`src/email/drafts.rs::list_drafts`). Inside that
popup, `j`/`k`/arrows move, `Enter` resumes the selected draft back into the compose form
(`App::resume_selected_draft`), `d` deletes it (`App::delete_selected_draft`), `Esc` closes it —
the same "popup owns simple list navigation, vim motions disabled" pattern as the existing
`CalendarInputMode::TaskPicker`.

`ComposeState` gained `draft_id: Option<i64>` so the round trip stays coherent: resuming a draft
sets it, saving again with it set overwrites that row (`save_draft`'s upsert) instead of inserting
a duplicate, and successfully sending a resumed draft (`App::apply_send_result`) deletes the
now-stale row — a draft never survives past becoming a real sent message. `SendResult` carries
`draft_id` through the existing send-spawn-and-report channel to make that cleanup possible
without re-opening the compose state after it's already been consumed.

New `tests/it/app.rs` cases cover save/overwrite/list/resume/delete (170 in-process tests, was
165); new scenario `email_drafts_lifecycle` drives save → list → resume → send → auto-delete →
delete-from-popup end-to-end against the fake SMTP server (150 TUI scenarios, was 149). See
`src/app/CLAUDE.md`'s `mail.rs` row, `src/email/CLAUDE.md`'s `drafts.rs` row, and
`src/tui/CLAUDE.md`'s key-binding table.

## Slice 11: per-account filter (done, 2026-09-27)

`A` in the email list cycles `App.account_filter` through `None` (merged, the default) and every
account with at least one stored message, alphabetical (`src/email/store.rs::distinct_accounts`,
`SELECT DISTINCT account ... ORDER BY account`), then back to `None`. `App::cycle_account_filter`
(`src/app/mail.rs`) picks the next label and calls `refresh_emails`, which — filter-at-source,
matching how `email_sort` already reorders `App.emails` in place — retains only that account's
rows on `App.emails` itself right after the `get_recent` load, before the priority/date sort, so
every existing action indexing into `App.emails`/`selected_email` (open, delete, archive, star,
convert-to-task, search) stays correct with no parallel filtered view to keep in sync. The list
title (`src/tui/ui/email.rs::render_email_view`) shows `sorted by {sort} · account: {label}` when
a filter is active, or just `sorted by {sort}` otherwise.

New `tests/it/app.rs` cases cover `refresh_emails`'s filter-retain and a full cycle through two
accounts and back to merged (172 in-process tests, was 170); new scenario `email_account_filter`
drives the `A` key across three seeded emails spanning two accounts (151 TUI scenarios, was 150).
See `src/app/CLAUDE.md`'s `mail.rs` row, `src/email/CLAUDE.md`'s `store.rs` row, and
`src/tui/CLAUDE.md`'s key-binding table.

## Slice 12: snooze (done, 2026-09-27)

`z` in the email list opens a spec prompt (`InputMode::EmailSnooze`, `render_snooze_box`); `10m`/
`2h`/`3d` snooze relative to now, `tomorrow`/`nextweek` land at 8am local (DST-safe, via
`src/app/time.rs::resolve_local_datetime`) — parsed by the pure `App::parse_snooze_spec(spec, now)
-> Option<DateTime<Utc>>`. Enter (`App::commit_snooze`) writes the result to the new
`email_messages.snoozed_until` column (`src/email/store.rs::set_snooze`) and refreshes; an
unparseable spec reports a status message and changes nothing. `refresh_emails` hides a message
with a future `snoozed_until` from the normal list — same filter-at-source shape as Slice 11's
account filter, applied right after it and before the priority/date sort — and shows only those
when `Z` (`App::toggle_show_snoozed`) flips `App.show_snoozed`. `x` clears an in-progress snooze
early (`App::unsnooze_selected_email` / `store::clear_snooze`). A lapsed snooze needs no separate
clear: once `snoozed_until` is in the past, `refresh_emails`'s filter treats it as not-snoozed on
its own, so the message just reappears next refresh. The list title shows `· snoozed view` while
active, and a currently-snoozed row is tagged `[snoozed until Sat 08:00]`.

Folder browsing (any folder beyond the one-way archive destination) was considered for this slice
and deferred instead — `EmailConfig` has one fixed `imap_folder` per account and `ImapMailSource`'s
`.select()` calls all use it directly, so adding a folder switcher is a larger, separate unit of
work than snooze turned out to be. Done in [Slice 13](#slice-13-folder-browsing-done-2026-09-27).

New `tests/it/app.rs` cases cover `parse_snooze_spec` (minutes/hours/days, the `tomorrow`/
`nextweek` keywords, and rejecting zero/negative/garbage specs) and the snooze/unsnooze/toggle
round trip through `refresh_emails` (177 in-process tests, was 172); new scenario
`email_snooze_hide_show` drives `z`/`Z`/`x` end-to-end (152 TUI scenarios, was 151). See
`src/app/CLAUDE.md`'s `mail.rs`/`model.rs` rows, `src/email/CLAUDE.md`'s `store.rs` row, and
`src/tui/CLAUDE.md`'s key-binding table.

## Slice 13: folder browsing (done, 2026-09-27)

Mail that leaves INBOX via Slice 6's archive no longer vanishes from Triptych. `sync_account`
(`src/email/sync.rs`) now runs a second pass over `config.archive_folder` after its `imap_folder`
pass, each with its own `email_sync_state` cursor (already keyed `(account, folder)`, no schema
change needed) — the archive pass's errors are caught and `tracing::warn!`ed rather than
propagated, so an account whose server has no such folder (or nothing archived into it yet) still
syncs INBOX exactly as before. `MailSource`'s four methods (`fetch_new`/`delete`/`archive`/
`fetch_attachments`) all take an explicit `folder: &str` now instead of assuming
`config.imap_folder`, since a loaded message's *actual* folder decides which mailbox
`ImapMailSource` selects before acting on its UID — this was also a latent correctness fix: any
non-INBOX message's delete/archive/attachment-fetch would previously have silently targeted the
wrong mailbox once such messages could be loaded at all.

`App.folder_filter` + `App::cycle_folder_filter` (`F` in the email list) mirror Slice 11's
account filter exactly, over `email::store::distinct_folders` instead of `distinct_accounts`;
`refresh_emails` applies its `.retain()` right after the account filter's, before snooze
visibility and the priority/date sort. The email list tags any row whose folder isn't `"INBOX"`
with `[folder]` (INBOX itself stays untagged, so the common case has no extra clutter), and the
title shows `· folder: {name}` while a filter is active.

New `tests/it/app.rs` cases cover the folder retain and the cycle-through-and-back-to-merged
round trip (179 in-process tests, was 177); new scenario `imap_tui_folder_filter` archives a
message, resyncs it back in from a real second `SELECT`ed mailbox on the fake IMAP server, and
drives `F` end-to-end (153 TUI scenarios, was 152) — this needed `tests/tui/fakeimap.py` to grow
genuine (if minimal) multi-mailbox `SELECT` support: a non-INBOX name now succeeds once a prior
`MOVE`/`COPY` has tagged something for it in `mailbox.json["archived"]`, and stays rejected
otherwise, so every account that never archives anything keeps its old single-cursor behavior
unchanged (see `docs/TUI_FAKES.md`). See `src/email/CLAUDE.md`'s `client.rs`/`sync.rs`/`store.rs`
rows, `src/app/CLAUDE.md`'s `mail.rs` row, and `src/tui/CLAUDE.md`'s key-binding table.

## Slice 14: smart NLP extraction from email body (done, 2026-09-27)

Converting an email to a task (`c`/Enter in the email list) used to parse the subject only —
`convert_selected_email_to_task` (`src/app/mail.rs`) always passed `email.subject` alone into
`App::submit_task`, so a due date or priority phrase sitting only in the body was invisible to the
NLP parser. It now folds `email.snippet` (the 200-char body preview, always populated in list rows,
no extra DB fetch) in alongside the subject when the snippet is non-empty, so the same regex/LLM
pipeline that already understands "by tomorrow" or "!!" can pick such a phrase out of the body too.

The risk this creates — per `src/nlp/CLAUDE.md`, unresolved input becomes the task's title
verbatim — is that a body with no recognizable phrase would otherwise leave garbled prose as the
title. `App::submit_task` gained a `title_override: Option<String>` parameter (threaded through
`TaskParse`, `apply_task_parse`, `insert_parsed_task`): when set, it replaces whatever title the
parse produced right before insert (and before `classify_task` runs, so category classification
sees the real title too), so `convert_selected_email_to_task` passes the bare subject through as
the override whenever it folded a snippet in — an email with no snippet at all skips the override
and keeps the parser's own cleaned-up title (e.g. a trailing "tomorrow" stripped into the deadline
field), unchanged from before Slice 14. `submit_task`'s other two call sites (`src/tui/keys.rs`'s
general non-email background add, and an existing test) pass `None`, untouched.

New `tests/it/app.rs` case `converting_an_email_extracts_a_deadline_from_the_body_but_keeps_the_subject_as_title`
covers a subject with no date phrase and a snippet that has one, asserting both that the deadline
lands and the title stays the subject (180 in-process tests, was 179); new scenario
`email_convert_body_due` covers the same end-to-end through the TUI (154 TUI scenarios, was 153).
See `src/app/CLAUDE.md`'s `tasks.rs`/`mail.rs` rows.

## Slice 15: IMAP IDLE (push-based sync) (done, 2026-09-27)

Replaces the fixed 60s poll with RFC 2177 `IDLE` per account: each configured account gets its own
concurrently-spawned `account_sync_loop` (`src/sync/mail.rs`, was one shared sequential
`interval.tick()` loop for all accounts) that blocks on `MailSource::idle_wait(imap_folder, timeout)`
between sync passes instead of sleeping a fixed duration — a per-account loop is required because
one account's IDLE call occupies its own connection for up to `IDLE_TIMEOUT` (300s, a backstop in
case a server pushes nothing), and accounts can no longer share one sequential loop without a
slow/idle one delaying every other account's sync.

`MailSource::idle_wait` (`src/email/client.rs`) opens a fresh connection (same one-shot-per-call
shape as every other `MailSource` method), sends `IDLE`, and returns `IdleOutcome::NewData` on any
unsolicited push, `IdleOutcome::Timeout` on the backstop elapsing — both trigger an ordinary
`sync_account` pass, since IDLE only proves *something* changed, never what. An `idle_wait` error
(network hiccup, a server without IDLE) degrades to the same thing rather than killing the loop.

**Regression caught and fixed before this shipped**: an open IDLE connection on `imap_folder`
necessarily wakes the instant an archive action removes a message from that folder — an archive is
exactly the kind of change IDLE watches for. If the resulting sync pass also swept `archive_folder`
(as `sync_account` always had, since Slice 13), it would rediscover the just-archived message as
new-to-that-folder and silently reinsert the row `archive_selected_email` had just deleted, breaking
`imap_tui_archive`/`imap_tui_archive_popup`/`imap_tui_archive_no_move`/`imap_tui_folder_filter`
(caught by the TUI scenario suite, not `cargo test`). `sync_account` gained an `include_archive:
bool` parameter: `false` for the background worker (the only *ambient* caller — an IDLE wake or the
backstop timeout, never a signal the user asked for), `true` for every *explicit* caller (`s`, view
entry, `triptych email sync`) — so folder-browsing a just-archived message back in stays a
deliberate, user-triggered act (the whole point of Slice 13) instead of an unavoidable side effect
of push-based sync. See `src/email/sync.rs`'s doc comment on `sync_account` for the full reasoning.

New scenario `imap_idle_push` (`tests/tui/tui_suite.py`, 155 TUI scenarios, was 154) drives mail
arriving while the TUI sits untouched on the To-Do view — no `m`, no manual sync — and asserts it's
picked up via a real blocked IDLE connection (checks the fake server's `commands.log` for `IDLE`
then `DONE` around the push), not a poll tick. `tests/tui/fakeimap.py` gained IDLE/DONE protocol
support: a `+ idling` continuation, then a side thread polls the mailbox file and pushes one
unsolicited `* n EXISTS` on a message-count change while the main thread blocks on a plain
`readline()` waiting only for `DONE` (a timed socket read doesn't mix cleanly with repeated
timeout-then-retry on a buffered socket file object in CPython — worked around by never setting a
read timeout at all). No new Rust unit tests — `idle_wait` is exercised only at the TUI-scenario
level so far, same as the rest of the background sync workers.

## Slice 16: folder discovery via IMAP LIST (done, 2026-09-27)

Slice 13's folder browsing only ever reached two folders per account (`imap_folder`,
`archive_folder`) because that's all `EmailConfig` names — a real mailbox's "Sent", "Drafts",
"Junk", or any custom folder was invisible to Triptych no matter how much mail was in it. Slice 16
adds real RFC 3501 `LIST` support so the app discovers whatever the server actually has.

`MailSource::list_folders` (`src/email/client.rs`) opens a one-shot connection (same shape as
every other `MailSource` method), sends `LIST "" *`, and returns every mailbox name except ones
flagged `\Noselect` (hierarchy-only nodes with no messages of their own — confirmed against
`imap-proto`'s parser that this is the literal wire token). `src/email/sync.rs` gained
`sync_one_folder(pool, config, folder)`, a thin wrapper over the same `sync_folder` helper
`sync_account` already used per-folder, so a freshly discovered folder gets the identical fetch/
parse/store/cursor pipeline instead of a second one.

`App::open_folder_browser` (`B` in the email list, `src/app/mail.rs`) spawns a background LIST pass
across every configured account and reports into a new `folder_browser_open` popup
(`src/tui/ui/email.rs`'s `render_folder_browser_popup`) listing every `(account, folder)` pair
found; `j`/`k`/arrows move the selection (motions disabled while the popup owns them, same
`folder_browser_open` gate as `drafts_open`), `Enter` (`App::browse_to_selected_folder`) sets the
folder filter to the picked name and kicks off `sync_one_folder` in the background so it actually
has mail to show instead of just narrowing to an empty view, `Esc` closes without picking anything.

New scenario `imap_tui_folder_browse` (`tests/tui/tui_suite.py`, 156 TUI scenarios, was 155) seeds
one INBOX message plus a registered-but-empty "Sent" folder and a `\Noselect`-flagged "Hidden" one,
opens the browser, asserts Sent and INBOX both appear and Hidden does not, then picks Sent and
asserts the one-folder sync and folder-filter both apply. `tests/tui/fakeimap.py` gained real LIST
support: `Mailbox.add_folder(name, noselect=False)` registers a folder that "exists" (is
SELECTable) without anything ever having been archived into it, and a new `list_` dispatch method
enumerates INBOX plus every `add_folder`-registered or `archived`-tagged name, emitting the
`\Noselect` attribute for ones registered that way. No new Rust unit tests — like `sync_account`
and `idle_wait` before it, `list_folders` only ever talks to a real IMAP server, so it's exercised
at the TUI-scenario level, not `cargo test`.

## Slice 17: unified inbox AI triage — Focused Inbox split (done, 2026-09-27)

Outlook's Focused/Other split: every synced email gets a one-time binary classification —
Focused (personal/work mail worth attention) or Other (bulk/automated: newsletter, receipt,
notification, marketing) — computed by the same local Ollama model already used for parsing and
summaries, not a new dependency. `email_messages.triage_focused` (`INTEGER`, tri-state via
`Option<bool>`: `NULL` until classified) is the new column (`src/migrations.rs`).

`NLPParser::triage`/`OllamaClient::triage` (`src/nlp/{parser,ollama_client}.rs`) mirror
`summarize`'s shape exactly (ignores the sticky `ollama_available` flag, so a late-started Ollama
still works): `build_triage_prompt` fences the subject and a 400-char-capped snippet as untrusted
data — same reasoning as `build_summary_prompt` — and asks for `{"focused": true|false}` JSON
back. `App::run_email_triage` (`src/app/mail.rs`) runs a background pass over
`email::store::pending_triage` (every row with `triage_focused IS NULL`, capped at
`TRIAGE_BATCH_LIMIT` = 20 per pass so a large first-sync backlog doesn't serialize dozens of model
round-trips at once — the rest catch up on the next sync's pass) after every mail/folder sync
(`apply_mail_sync`, `apply_folder_sync`), guarded by a `triage_running` flag so passes never
overlap. Results come back over a new `triage_rx` channel, applied in place (`apply_triage`, wired
into `run_app`'s `tokio::select!` in `src/tui.rs`) without a full `refresh_emails` reload, so the
cursor never moves mid-pass; a per-email failure is logged and skipped rather than failing the
whole batch.

`I` in the email list cycles `App.focus_filter` through `None` (merged) → `Some(true)` (Focused
only) → `Some(false)` (Other only) → `None` — a fixed 3-state cycle, unlike the dynamic
account/folder filters, since classification is always binary. `refresh_emails`'s `.retain()`
chain gained a `focus_filter` step right after `folder_filter`; a not-yet-classified message
(`triage_focused` still `NULL`) matches neither `Some` state, so it drops out of both filtered
views until triage catches up, same as any other filter's edge case. The list title shows
`· focus: Focused`/`· focus: Other` while active; an Other-classified row is tagged `[other]` in
the merged view (Focused stays untagged, same "don't clutter the common case" convention as the
folder tag).

New `tests/it/nlp_llm.rs` case asserts the triage prompt fences subject/snippet as untrusted data
(181 in-process tests, was 180); new scenarios `email_focus_filter` (seeded rows, drives `I`
end-to-end) and `email_triage_after_sync` (real fake-IMAP sync + fake-Ollama classification,
asserting the DB actually gets populated by the background pass, not just the filter UI) (158 TUI
scenarios, was 156). `tests/tui/fakeollama.py` gained a `"triage"` prompt kind: it answers
`{"focused": false}` when the fenced `<email>` body contains a bulk-mail keyword
("newsletter"/"receipt"/"notification"/"unsubscribe"), `{"focused": true}` otherwise — matched
against only the fenced section, not the whole prompt, since the prompt's own instructions name
those same keywords as examples when explaining what "Other" means (see `docs/TUI_FAKES.md`).
See `src/app/CLAUDE.md`'s `mail.rs` row, `src/nlp/CLAUDE.md`, and `src/tui/CLAUDE.md`'s
key-binding table.

## Slice 18: rules / auto-actions (done, 2026-09-27)

`R` in the email list opens the rules popup (`App::open_rules_list`, loads every saved rule fresh
from the new `email_rules` table); `j`/`k`/arrows move the selection, `n` opens a one-line spec
prompt (`InputMode::EmailRuleInput`), `d` deletes the selected rule, `Esc` closes. A spec is
`<field> <pattern...> <action>`, e.g. `subject newsletter star` or `from noreply read` — `field` is
`subject` or `from`/`sender` (both map to the stored `from_addr`), `pattern` may contain spaces (the
last whitespace-separated token is always `action`), `action` is `star` or `read`. Parsed by the
pure `App::parse_rule_spec`/matched by the pure `App::match_rule` (case-insensitive substring, not a
regex — keeps specs typeable in one line); an unparseable spec reports a status message and saves
nothing.

`App::run_email_rules` runs inline (awaited, not spawned) right after every sync
(`apply_mail_sync`, `apply_folder_sync`), same trigger point as Slice 17's triage — but unlike
triage there's no Ollama/IMAP round-trip, just local DB reads/writes, so there's no long-running I/O
to keep off the render loop. It loads every saved rule and every email with `rule_applied = 0`
(`email::store::pending_rule_check`, capped at `RULE_CHECK_LIMIT` = 100 per pass), applies every
matching rule's action, then marks the email checked regardless of whether anything matched — a
non-match is still a completed check, so a later pass never re-evaluates it (also means a rule
added after the fact only applies to *new* mail, and a manual un-star survives future passes). Only
`star`/`read` are supported: both are pure local writes; `archive`/`delete` need a live IMAP
round-trip per match and are deferred past v1.

New `tests/it/email_rules.rs` covers the pure `parse_rule_spec`/`match_rule` functions (field/action
validation, multi-word patterns, case-insensitivity); new `tests/it/app.rs` cases cover the popup's
save/reject path, an end-to-end sync-then-match-then-star round trip, and that `rule_applied`
actually sticks (194 in-process tests, was 181). See `src/app/CLAUDE.md`'s `mail.rs` row,
`src/email/CLAUDE.md`'s `store.rs` row, and `src/tui/CLAUDE.md`'s key-binding table.

## Slice 19: meeting invites (done, 2026-09-27)

`email::message::parse_raw` now extracts a meeting invite from a `text/calendar` `VEVENT` MIME
part (private `extract_meeting_invite`/`date_perhaps_time_to_utc`): `mail-parser` classifies any
`text/calendar` part as an attachment regardless of `Content-Disposition`, so it's found the same
way `extract_attachments` finds a PDF. The ICS body is parsed with the `icalendar` crate
(`Calendar::from_str`); `title`/`start`/`end`/`location` land on four new nullable
`EmailMessage`/`NewEmail` fields (`meeting_title`, `meeting_start`, `meeting_end`,
`meeting_location`), added via the usual idempotent `ALTER TABLE` pattern in `src/migrations.rs`
(not a new `.sql` file — matches every other post-initial-schema column). A message with no
calendar part, or one that fails to parse, just leaves all four `None` — never errors the sync.

`App::accept_meeting_invite` (`M` in the email list or the detail popup) turns an invite into a
scheduled task: unlike `convert_selected_email_to_task`, it never goes through `submit_task`'s NLP
parse, since the exact time is already on hand from the ICS `DTSTART`/`DTEND` — a direct `INSERT`
into `tasks`, the same shape as `placement::add_task_at_selected_cell`. It reuses
`email_messages.task_id` as the same "already converted" marker `convert_selected_email_to_task`
uses, so accepting twice is a no-op the second time, and a converted invite still gets `[task]` in
the list like an NLP-converted email would. The list tags an unconverted invite `[invite]`
(dropped once `task_id` is set, since `[task]` already covers that); the detail popup shows a
"Meeting: `<title>` — `<time>`" line, a "Location: `<location>`" line when present, and a
"Press M to accept as a task" hint until accepted.

New `tests/it/email_message.rs` cases cover `parse_raw`'s invite extraction (fields populated, the
`header_only` skip, and the no-calendar-part case); new `tests/it/app.rs` cases cover
`accept_meeting_invite`'s create/no-invite/idempotent-second-call paths (200 in-process tests, was
194). `tests/tui/fakeimap.py`'s `Mailbox.add(..., invite=True)` builds a fixed "Team Sync" VEVENT
for the new `imap_tui_meeting_invite` scenario (159 TUI scenarios, was 158), which drives the tag,
the popup lines, `M`, and the resulting `[task]` tag end-to-end. See `src/app/CLAUDE.md`'s
`mail.rs` row, `src/email/CLAUDE.md`'s `message.rs` row and its `chrono-tz` gotcha, and
`src/tui/CLAUDE.md`'s key-binding table.

## Slice 20: signatures (done, 2026-09-27)

`SmtpConfig` gained a `signature: Option<String>` field, read from `EMAIL_SIGNATURE[_<LABEL>]`
(`config::normalize_signature`: literal `\n` two-character escapes become real newlines, since a
`.env` value is one line; blank/whitespace-only counts as unset) — signature is a per-send-account
setting like `from_addr`, not per-receive-account, so it lives on `SmtpConfig` rather than
`EmailConfig`. `start_compose_new`/`start_reply`/`start_forward` (`src/app/mail.rs`) copy it onto
the new `ComposeState.signature` field from the relevant `SmtpConfig`; `resume_selected_draft`
looks it up fresh by the draft's account, same as the other three, since a draft's saved row never
stores it.

`signature` is read-only in the compose popup, appended below the editable `body` and above any
quoted original/forward — the same relationship `quoted` already had to `body`, extended one step.
Both the popup's live preview and the actual sent body now go through one new pure helper,
`app::mail::compose_full_body(body, signature, quoted)` (replacing `send_compose`'s and
`render_compose_popup`'s two independently-hand-rolled concatenations, which had already started to
drift in blank-line handling), so the sent message always matches what was previewed.

New `tests/it/email_config.rs` cases cover `normalize_signature`'s escape/blank handling and
`SmtpConfig`'s `Debug` redaction; new `tests/it/email_compose.rs` cases cover `compose_full_body`'s
ordering and separator handling (205 in-process tests, was 200). New `tests/tui/tui_suite.py`
scenario `email_compose_signature` sets `EMAIL_SIGNATURE`, confirms it renders in the popup, and
confirms a sent message's body has it after the typed text (160 TUI scenarios, was 159). See
`src/app/CLAUDE.md`'s `mail.rs` row and `src/email/CLAUDE.md`'s `config.rs`/gotchas.

## Slice 21: rule actions `archive`/`delete` (done, 2026-09-27)

Extends [Slice 18](#slice-18-rules--auto-actions-done-2026-09-27)'s rules beyond the two pure-local
actions: `parse_rule_spec` now also accepts `archive`/`delete`, e.g. `from spam@example.com delete`.
Unlike `star`/`read`, these need a live IMAP round-trip per match, which `run_email_rules` cannot
await inline without stalling every other rule and email in the same sync pass. `apply_rule_action`
(`src/app/mail.rs`) now takes the full `&EmailMessage` (was just `email_id`, since `archive`/`delete`
need the message's `account`/`folder`/`uid`, not only its id) and, for those two actions, hands off to
new `spawn_archive`/`spawn_delete` helpers — the same `tokio::spawn` + `mpsc`-report shape the manual
`a`/`d` keypresses already used, now factored out so a rule match and a manual keypress share one
code path and one cleanup method (`apply_archive_result`/`apply_delete_result`). A rule action against
an account with no IMAP config, or an email with an out-of-range UID, silently no-ops (still marks the
email checked) — same guard-clause behavior the manual paths already had, minus the user-facing
`notify`.

New `tests/it/email_rules.rs` case covers `parse_rule_spec` accepting both new actions; new
`tests/it/app.rs` case covers the no-IMAP-config no-op path (207 in-process tests, was 205). New
scenario `email_rule_archive_delete` (161 TUI scenarios, was 160) drives both actions end-to-end
against the fake IMAP server. Exercising two rule-triggered actions firing at once as genuinely
concurrent IMAP connections exposed a pre-existing race in the *test fixture* itself — see
[`tests/tui/CLAUDE.md`](../tests/tui/CLAUDE.md)'s `Mailbox` concurrency gotcha — not in
`triptych`'s own code. See `src/app/CLAUDE.md`'s `mail.rs` row and `src/email/CLAUDE.md`'s
`message.rs` row.

## Slice 22: categories (color tags) (done, 2026-09-27)

Outlook's colored-category tagging, simplified to one tag per message (Outlook allows several —
one is enough to sort/scan by at a glance). `t` in the email list or the detail popup cycles the
selected message's `category` column through a fixed six-color palette (`app::mail::CATEGORY_ORDER`:
red, orange, yellow, green, blue, purple) and wraps back to untagged after purple, via the pure
`app::mail::next_category` and `App::cycle_selected_category` (same shape as `toggle_selected_star`).
Persisted through a new nullable `email_messages.category` column (`src/migrations.rs`, idempotent
`ALTER TABLE`) and `email::store::set_category`; `EmailMessage.category` was added to the struct and
all three of `store.rs`'s `EmailMessage`-selecting queries, but deliberately not to `NewEmail` — like
`is_starred`, it's set post-insert, never at parse time.

Rendered as a `[color]` bracket tag in the list row (`email_list_item`, colored via a new
`category_color` helper — named ANSI colors only, `LightRed`/`Magenta` approximating orange/purple
since ratatui's `Color` has no true variant for either) and as a "Category: <Name>" line in the
detail popup; both title-bar hint strings gained `t: category`.

New `tests/it/app.rs` cases cover `next_category`'s wraparound (pure) and
`cycle_selected_category`'s full cycle through the DB (209 in-process tests, was 207). New scenario
`email_category_cycle` drives all six colors plus the wrap back to untagged, checking both the DB
column and the list-row tag (162 TUI scenarios, was 161). See `src/app/CLAUDE.md`'s `mail.rs` row,
`src/email/CLAUDE.md`'s `message.rs`/`store.rs` rows, and `src/tui/CLAUDE.md`'s `keys.rs`/`ui.rs` row.

## Slice 23: attachment-presence filter (done, 2026-09-27)

`H` in the email list cycles `App.attachment_filter` (`Option<bool>`) through merged (all mail) ->
has attachments only -> no attachments only -> merged, via `App::cycle_attachment_filter` — same
fixed 3-state shape as `cycle_focus_filter`, but simpler: `EmailMessage.has_attachments` is always
known (computed at query time by `email::store::get_recent`'s `EXISTS(...)` subquery, never `None`),
so unlike focus there's no "unclassified" bucket to fall out of. `refresh_emails` gained a matching
`.retain()` step, placed right after `focus_filter`'s in the existing account -> folder -> focus ->
attachment -> snooze chain. No schema change needed — `email_attachments` already existed (Slice 9).
Title bar gained `H: attachment filter` and a `· attach: Yes`/`No` tag, same style as the other
filters' tags. `handle_email_key` (`src/tui/keys.rs`) crossed clippy's 100-line function limit once
`H` was added, so the four filter-cycle keys (`A`/`F`/`I`/`H`) were split into a private
`handle_email_filter_key` helper.

New `tests/it/app.rs` case `cycle_attachment_filter_walks_has_then_lacks_then_back_to_merged` covers
the full 3-state cycle against the DB (210 in-process tests, was 209). New scenario
`email_attachment_filter` seeds one message with an `email_attachments` row and one without, checking
both the `[attach]` list tag and all three filter states (163 TUI scenarios, was 162). See
`src/app/CLAUDE.md`'s `mail.rs` row and `src/tui/CLAUDE.md`'s `keys.rs`/`ui.rs` row.

## Slice 24: unread/starred filter chips (done, 2026-09-27)

`U` cycles `App.unread_filter` and `S` cycles `App.starred_filter` (both `Option<bool>`) through
merged (all mail) -> matching only -> non-matching only -> merged, via `App::cycle_unread_filter`/
`App::cycle_starred_filter` — same fixed 3-state shape as `cycle_attachment_filter`:
`EmailMessage.is_read`/`is_starred` are always known, never `None`, so neither has an "unclassified"
bucket to fall out of. `refresh_emails` gained two matching `.retain()` steps, placed right after
`attachment_filter`'s in the account -> folder -> focus -> attachment -> unread -> starred -> snooze
chain. No schema change needed — both columns already existed. Title bar gained `U: unread filter`/
`S: starred filter` and `· unread: Yes`/`No` / `· starred: Yes`/`No` tags, same style as the other
filters. `handle_email_filter_key` (`src/tui/keys.rs`, split out in Slice 23) grew from four to six
filter-cycle keys (`A`/`F`/`I`/`H`/`U`/`S`).

New `tests/it/app.rs` cases `cycle_unread_filter_walks_unread_then_read_then_back_to_merged` and
`cycle_starred_filter_walks_starred_then_unstarred_then_back_to_merged` cover both full 3-state
cycles against the DB (214 in-process tests, was 212). New scenarios `email_unread_filter` and
`email_starred_filter` each seed two messages, mark one read/starred via a raw `UPDATE`, and check
all three filter states plus (for starred) the `●` list marker (165 TUI scenarios, was 163). See
`src/app/CLAUDE.md`'s `mail.rs` row and `src/tui/CLAUDE.md`'s `keys.rs`/`ui.rs` row.

## Slice 25: sender-domain filter (done, 2026-09-27)

`@` in the email list cycles `App.domain_filter` (`Option<String>`) through `None` (merged, the
default) and every sender domain with >= 1 stored message, alphabetical
(`email::store::distinct_domains`: `SELECT DISTINCT substr(from_addr, instr(from_addr, '@') + 1)
... ORDER BY domain`, mirroring `distinct_accounts`/`distinct_folders`'s shape), then back to
`None`, via `App::cycle_domain_filter` — same dynamic-list cycle shape as
`cycle_account_filter`/`cycle_folder_filter`. `EmailMessage.from_addr` is always a bare address
(`message.rs::parse_raw` already splits any display name off via `mail-parser`), so the retain
predicate is a plain `rsplit('@').next()` comparison, no re-parsing needed. `refresh_emails`
gained a matching `.retain()` step, placed last in the account -> folder -> focus -> attachment ->
unread -> starred -> domain -> snooze chain. Title bar gained `@: domain filter` and a
`· domain: {domain}` tag, same style as the other filters' tags.

Not bound to `G`: `src/app/motion.rs`'s vim-motion layer claims `G` (bottom-of-list, paired with
`gg`) ahead of every per-view key handler in the email list (`src/tui/CLAUDE.md`'s "Motions run
first" rule), so a `G` binding here would never fire — caught by a failing TUI scenario before
shipping (see `docs/DEVELOPMENT.md`'s Slice 25 entry). `@` was free (checked via `grep -n
"MotionKey::Char\|Char('@')" src/app/motion.rs src/tui/keys.rs`) and reads naturally for "domain".

New `tests/it/app.rs` case `cycle_domain_filter_walks_every_domain_then_back_to_merged` covers the
full cycle against the DB (215 in-process tests, was 214). New scenario `email_domain_filter`
seeds two messages at different domains and checks all three filter states (166 TUI scenarios, was
165). See `src/app/CLAUDE.md`'s `mail.rs` row, `src/email/CLAUDE.md`'s `store.rs` row, and
`src/tui/CLAUDE.md`'s `keys.rs`/`ui.rs` row.

## Explicitly deferred

- OAuth2 / Gmail-native auth.
- **Calendar integration with email** (e.g. surfacing today's events in the email view, or vice
  versa, or writing an accepted invite into `schedule_blocks` instead of `tasks`). Not started —
  [Slice 19](#slice-19-meeting-invites-done-2026-09-27) covers detect-and-accept only.
- **Multiple simultaneous account inbox views** (side-by-side, not just the account filter's
  one-at-a-time cycle). Not started — `App.account_filter` is a single `Option<String>`, so this
  would need a real layout change, not just another filter state.
- **Advanced filtering/sorting** beyond date, priority, focus, attachment presence, unread, star
  state and sender domain (e.g. a saved combination of filters). Not started.

## Known limitations

- First sync (no stored UID yet) caps at the most recent 25 messages
  (`INITIAL_SYNC_LIMIT` in `src/email/client.rs`), not the entire mailbox history —
  fetching full RFC822 bodies for an entire real inbox is slow and memory-heavy.
  Older mail is never backfilled; only new mail from that point on is synced.
- `.env` is loaded once at startup by `dotenvy::dotenv()` (`main.rs`), never overriding a var
  already in the process environment - a real shell export always wins over `.env`.
