# Email messages history

Reading and acting on mail: list and detail, compose/reply/forward, drafts, delete/archive, attachments,
threads, stars, snooze, contacts, the fake SMTP server. Newest first.
Hub, workflow and open issues: [`../DEVELOPMENT.md`](../DEVELOPMENT.md). Code:
[`../../src/email/CLAUDE.md`](../../src/email/CLAUDE.md), [`../../src/app/CLAUDE.md`](../../src/app/CLAUDE.md).
Related: [`email-sync.md`](./email-sync.md), [`email-triage.md`](./email-triage.md), [`../roadmap-email.md`](../roadmap-email.md).

## Changelog

- 2026-10-21 (email follow-ups): closes what the UX fixes and perf entries in [`tui-ux.md`](./tui-ux.md) left open.
  - List: the cursor row is `Modifier::REVERSED`, so unread bold and per-span colors stay visible.
    Only the visible rows are built (`scroll_offset` in `ui/email.rs`; the offset lives on
    `App.email_list_state`, so the list scrolls only as far as the cursor needs).
  - Compose: accepting a suggestion appends `, `; `address_list` in `email/smtp.rs` trims a trailing
    `,`/`;` from `To:` and `Cc:`.
  - Rules: the popup says new rules also cover synced mail. `store::create_rule` resets
    `rule_applied`, so each new rule re-runs all rules once over stored mail. A manual unstar or
    unread can be overridden again; a `delete` rule is a permanent server delete.
  - Triage: `run_email_triage` loops passes until nothing is pending. `TriageDone.finished` ends the
    drain (empty backlog or a failure; the next sync retries) and only that report clears `triage_running`.
  - Daemon: the cache preload runs in a spawned task, so the socket accepts before it finishes. A
    preload DB error is logged, no longer fatal.
  - IMAP: `fetch_new` returns at most `SYNC_CHUNK` (100) messages, the oldest after the cursor.
    `sync_folder` stores each chunk and moves the cursor before the next (up to 20 chunks per call; a
    bigger backlog resumes on the next sync), so a timeout keeps the earlier chunks.
  - Tests: 360 `cargo test` tests; new scenarios `email_rules_popup_note` and `imap_sync_in_chunks`
    (suite is 190, 187 gating).
- 2026-09-30 (email legend): `docs/USAGE.md`'s Email section now explains every list-row mark (`▲ △ ●`, `[folder]`, `[other]`, `[N]`, `[attach]`, `[invite]`, `[task]`, `[snoozed until ..]`) and the title-bar filter tags. Docs only.
- 2026-09-29 (contact noise filter): `store::contacts` drops automated senders (`is_noise_address`: noreply, notifications, mailer-daemon, ...) and senders whose every message triage marked Other. Still senders only.
- 2026-09-29 (compose contact autofill): To/Cc suggest contacts while typing (name or address, the
  fragment after the last `,`). `email::store::contacts` (senders by frequency) loads into
  `App.contacts` in `toggle_to_email`; pure `suggest_contacts` matches (5 max); `Ctrl-N`/`Ctrl-P` move,
  `Enter` accepts (`compose_accept_suggestion`, else newline as before). Sent-mail recipients are not
  included yet. Scenario `email_compose_autofill`.
- 2026-09-27 (snooze, Slice 12): `z` in the email list opens a spec prompt (new
  `InputMode::EmailSnooze`, `render_snooze_box`); `10m`/`2h`/`3d` snooze relative to now,
  `tomorrow`/`nextweek` land at 8am local (DST-safe via `resolve_local_datetime`) — parsed by the
  pure `App::parse_snooze_spec(spec, now) -> Option<DateTime<Utc>>`. Enter (`App::commit_snooze`)
  writes the result to a new `email_messages.snoozed_until` column (idempotent migration in
  `src/migrations.rs`, `store::set_snooze`) and refreshes; an unparseable spec reports a status
  message and changes nothing. `refresh_emails` hides a message with a future `snoozed_until` from
  the normal list — same filter-at-source shape as Slice 11's account filter, applied right after
  it and before the priority/date sort — and shows only those when `Z`
  (`App::toggle_show_snoozed`) flips `App.show_snoozed`. `x` clears a snooze early
  (`App::unsnooze_selected_email` / `store::clear_snooze`). A lapsed snooze needs no separate
  clear: once `snoozed_until` is in the past, the same filter treats it as not-snoozed, so the
  message just reappears next refresh — nothing writes `NULL` back on its own. The list title shows
  `· snoozed view` while active, and a currently-snoozed row is tagged `[snoozed until Sat 08:00]`.
  Folder browsing beyond the one-way archive destination was considered for this slice and deferred
  instead: `EmailConfig` has one fixed `imap_folder` per account and every `ImapMailSource::select()`
  call uses it directly, so a folder switcher is a larger, separate unit of work than snooze turned
  out to be — done above, Slice 13. New `tests/it/app.rs` cases cover `parse_snooze_spec` (minutes/hours/days, the
  `tomorrow`/`nextweek` keywords, rejecting zero/negative/garbage specs) and the
  snooze/unsnooze/toggle round trip through `refresh_emails` (177 in-process tests, was 172); new
  scenario `email_snooze_hide_show` drives `z`/`Z`/`x` end-to-end (152 TUI scenarios, was 151). See
  [`roadmap-email.md`](../roadmap-email.md)'s Slice 12, `src/app/CLAUDE.md`'s `mail.rs`/`model.rs`
  rows, `src/email/CLAUDE.md`'s `store.rs` row, `src/tui/CLAUDE.md`'s key-binding table.
- 2026-09-27 (drafts, Slice 10): `Ctrl-D` in the compose/reply/forward popup saves the current form
  to a new `email_drafts` table (`CREATE TABLE IF NOT EXISTS`, `src/migrations.rs`) instead of
  sending, and closes the popup; `D` in the email list opens a drafts popup
  (`render_drafts_popup`) listing every saved draft, most-recently-updated first
  (`src/email/drafts.rs::list_drafts`) — `j`/`k`/arrows move, `Enter` resumes the selected draft back
  into compose (`App::resume_selected_draft`), `d` deletes it (`App::delete_selected_draft`), `Esc`
  closes, following the same "popup owns simple list navigation, motions disabled" pattern as
  `CalendarInputMode::TaskPicker`. `ComposeState` gained `draft_id: Option<i64>`: resuming a draft
  sets it so a later save overwrites the same row (`save_draft`'s upsert) instead of inserting a
  duplicate, and `SendResult` now carries `draft_id` through the existing send-spawn-and-report
  channel so `apply_send_result` can delete the stale draft row after a successful send — a draft
  never survives past becoming a real sent message. New `tests/it/app.rs` cases: save-inserts,
  save-overwrites-the-resumed-row, list-loads-most-recent-first, resume-reopens-and-closes-the-list,
  delete-removes-from-db-and-the-open-list (170 in-process tests, was 165). New scenario
  `email_drafts_lifecycle` drives save -> list -> resume -> send -> auto-delete -> delete-from-popup
  end-to-end against the fake SMTP server (150 TUI scenarios, was 149; see KI-25 above for an
  unrelated harness bug this same verification run surfaced and fixed). See
  [`roadmap-email.md`](../roadmap-email.md)'s Slice 10, `src/app/CLAUDE.md`'s `mail.rs` row,
  `src/email/CLAUDE.md`'s `drafts.rs` row, `src/tui/CLAUDE.md`'s key-binding table.
- 2026-09-27 (`References`-header threading, Slice 7): `App::thread_count`
  (`src/app/mail.rs`) now reconstructs real conversations instead of only grouping by subject.
  New private `MsgIdForest` (a union-find keyed by bare `Message-ID` strings) unions every loaded
  message's own id with each id in its `References` header (already stored per-message as
  `references_header` since an earlier slice, unused for grouping until now) — an ancestor never
  fetched into `App::emails` still links its children into one component, since the forest's
  nodes are id strings, not indices, so two replies to the same unfetched parent land in one
  thread even though that parent has no row. Falls back to the existing `normalize_subject`
  grouping only when the target's own component comes back size 1 (no header info at all, or
  every referenced id is likewise absent from what's loaded) — this covers the common case of a
  first-in-thread message or a client that drops the header. `thread_count`'s signature and both
  UI call sites (list `[N]` badge, detail popup's "N messages in this thread") are unchanged.
  New `tests/it/email_thread.rs` cases: a header chain wins over an unrelated subject, two
  siblings link through a shared unfetched ancestor id, and a header pointing at nothing loaded
  still falls back to subject grouping (158 in-process tests, was 155). No TUI-scenario changes:
  `fakeimap.py` never sets `references_header` on synced mail, so this is exercised at the unit
  level only.

- 2026-09-27 (attachments, Slice 8): metadata-only — bytes are never persisted. New
  `email_attachments` table (`ON DELETE CASCADE`, `src/migrations.rs`), extracted at parse time by
  `message.rs::parse_raw` via `mail-parser`'s `Message::attachments()` and written in the same
  transaction as the message (`store::insert_new`, gated on `rows_affected() == 1` before trusting
  `last_insert_rowid()`). `store::get_recent` gained `EmailMessage.has_attachments` via a cheap
  indexed `EXISTS(...)` subquery. Saving re-fetches over IMAP on demand rather than reading a local
  copy: new `MailSource::fetch_attachments(uid)` (one connect, all parts), `App::save_selected_attachments`
  (`s` in the detail popup, spawned like delete/archive) writes each part under
  `TRIPTYCH_ATTACHMENT_DIR/<email_id>/` (falls back to `$TMPDIR/triptych-attachments`), sanitizing
  each `Content-Disposition: filename` first — `app::sanitize_filename` keeps only the final path
  component, since that header is attacker-controlled wire data and a raw join risks
  `../../etc/passwd`-style traversal. `App.email_attachments` caches metadata per email id for the
  detail popup's "Attachments: ..." line and the list's `[attach]` tag, populated on open like
  `email_summaries`. New `tests/it/email_attachments.rs`: `parse_raw` extraction (filename/
  content-type/size, `header_only` skip, no-attachment case) and `sanitize_filename` (plain name,
  traversal stripped to last component, empty/traversal-only rejected) (164 in-process tests, was
  158). `fakeimap.py`'s `make_message(..., attachment=True)` builds a multipart PDF message; new
  scenario `imap_tui_attachment_save` drives save-to-disk end-to-end (148 TUI scenarios, was 147).
- 2026-09-27 (real IMAP archive/folder-move, Slice 6): `a` in the email list or detail popup
  archives the selected message. `MailSource::archive(uid)` (`src/email/client.rs`, own
  `ARCHIVE_TIMEOUT` 30s, same budget as delete's) tries RFC 6851 `UID MOVE` to
  `EmailConfig::archive_folder` (new field, `IMAP_ARCHIVE_FOLDER[_<LABEL>]`, defaults
  `"Archive"`) first. Read `async-imap-0.11.3`'s own source before writing this (per this
  project's "don't guess a crate signature" rule): `uid_mv`/`uid_copy` both call
  `run_command_and_check_ok` internally, which loops reading the tagged completion itself, so
  unlike delete's `uid_store`/`expunge` they need no manual stream-draining. A server that
  answers `Bad`/`No` (no `MOVE` capability) falls back to the classical `UID COPY` + `UID STORE
  +FLAGS.SILENT (\Deleted)` + `EXPUNGE` sequence RFC 6851 defines MOVE as equivalent to — that
  fallback's `uid_store`/`expunge` calls do still return streams and are drained exactly as
  delete's are (`try_next` loop, then `try_collect`). `App::archive_selected_email` spawns the
  call and reports over a new `archive_rx` channel (same spawn-and-report pattern as
  delete/send/sync); the local row is only dropped (`store::delete_email`, reused as-is — an
  archived row and a deleted row leave the same local state) after the server confirms.
  `apply_archive_result` mirrors `apply_delete_result`'s cleanup exactly: closes the detail
  popup on success, leaves it open with "Archive failed: ..." otherwise. Does not auto-create
  the destination folder — a missing folder surfaces as that same failure message. No
  confirmation prompt, same reasoning as delete (no confirm-dialog pattern anywhere in this
  codebase). `tests/tui/fakeimap.py` gained `UID MOVE`/`UID COPY` support plus
  `Mailbox.disable_move()` (forces a `BAD` response so the fallback path is actually exercised,
  not just the happy path) — moved/copied messages land in a `mailbox.json["archived"]` list
  rather than real multi-mailbox SELECT/LIST state, since the real client's MOVE/COPY target
  never needs to be the currently-selected mailbox. Three new scenarios: `imap_tui_archive`,
  `imap_tui_archive_popup`, `imap_tui_archive_no_move` (147 scenarios total, was 144). No new
  Rust-level unit tests, same rationale as Slice 5: the interesting logic (the MOVE-then-
  fallback branch) has no new pure/testable function, so it's covered end-to-end through the
  fake IMAP server instead (155 in-process tests, unchanged).
- 2026-09-27 (real IMAP delete, Slice 5): `d` in the email list or detail popup permanently
  deletes the selected message. `MailSource::delete(uid)` (`src/email/client.rs`, own
  `DELETE_TIMEOUT` 30s) does `UID STORE +FLAGS.SILENT (\Deleted)` then `EXPUNGE` against that
  message's own account (`EmailConfig::for_account`, new); both returned `async-imap` streams are
  fully drained (`try_collect`, not a bare unpolled drop or `.await` alone) rather than dropped,
  since an unpolled stream leaves that command's responses unread on the wire and desyncs the next
  command's parsing (the `expunge()` stream is not `Unpin`, so `try_collect` — takes `self`, no
  `Unpin` bound — replaces the `try_next` loop used elsewhere in this file). `App::delete_selected_email`
  spawns the call and reports over a new `delete_rx` channel (same
  spawn-and-report pattern as send/sync); the local row (`store::delete_email`) is only dropped on
  confirmed server success, so a network error never silently loses mail the server still has.
  `apply_delete_result` closes the detail popup on success, leaves it open with "Delete failed:
  ..." otherwise. No confirmation prompt: this codebase has no confirm-dialog pattern anywhere
  (todo delete is immediate too), so email delete follows that convention. `tests/tui/fakeimap.py`
  gained `UID STORE`/`EXPUNGE` support (RFC-correct sequence-number shifting on expunge); two new
  scenarios, `imap_tui_delete`/`imap_tui_delete_popup` (144 scenarios total, was 142). No new
  Rust-level unit tests: the feature has no pure logic beyond a `u32::try_from` guard, and is only
  reachable end-to-end through a real or fake IMAP round-trip, so it is covered at the TUI-scenario
  layer instead (155 in-process tests, unchanged).
- 2026-09-27 (email starring/mark-unread, Slice 3): `email_messages.is_starred` (idempotent
  `ALTER TABLE`, `src/migrations.rs`), `store::set_starred`/`mark_unread` mirroring `mark_read`'s
  shape. `f` toggles the selected message's star (`App::toggle_selected_star`); `u` marks it unread
  again (`App::mark_selected_email_unread`) — useful since opening the detail popup auto-marks
  read. Both bound in the email list and the detail popup (`handle_email_detail_key` in
  `src/tui/keys.rs` is now `async` to await the DB write), both re-run `refresh_emails`. Rendered as
  a yellow `●` before the subject in the list and a "Starred" line in the detail popup — display
  only, does not affect `urgency.rs`'s sort score. Tests: `tests/it/app.rs` gained 2 (star toggle,
  mark-unread); the 4 pre-existing sites constructing `EmailMessage` by struct literal
  (`tests/it/email_priority.rs`, `tests/it/email_compose.rs`) updated for the new field.
- 2026-09-27 (email thread grouping, Slice 4): `App::normalize_subject`/`thread_count`
  (`src/app/mail.rs`, pure) group the already-loaded merged inbox by subject with repeated
  `Re:`/`Fwd:`/`Fw:` prefixes stripped and case ignored; a blank normalized subject never groups
  (returns a thread size of 1, so subjectless mail is never bucketed together). Rendered as a
  `[N]` badge before the subject in the list and an "N messages in this thread" line in the detail
  popup, only when `N > 1`. Local, subject-based stand-in — not a `References`/`In-Reply-To`
  conversation reconstruction, which the already-stored `references_header` column would need a
  cross-message header query to build; still deferred (see
  [`roadmap-email.md`](../roadmap-email.md)). New `tests/it/email_thread.rs` (4 tests). Considered
  real IMAP delete/archive as this increment's slice instead; deferred it — `fakeimap.py` has no
  `STORE`/`EXPUNGE` support yet, so it is untestable end-to-end without extending the fake server
  first. Tests: 155 in-process (was 149).
- 2026-09-27 (email send/reply/forward, Slice 2): hand-rolled RFC 5321 SMTP client
  (`src/email/smtp.rs`), sharing TLS setup with the IMAP client via new `src/email/tls.rs`.
  `SmtpConfig` (`src/email/config.rs`) mirrors `EmailConfig`'s multi-account env scheme, keyed by
  the same account label, so `.env`'s previously-dead `SMTP_*` vars are now read. New
  `InputMode::EmailCompose` and `App::email_compose: Option<ComposeState>` (`src/app/model.rs`)
  back a compose/reply/forward popup (`src/tui/ui/email.rs`): `c` in the email list starts a
  blank compose, `R`/`A`/`F` in the detail popup start reply/reply-all/forward (subject prefixed
  once, quoted original appended below the editable body, RFC 5322 `References` chained from the
  original's own header plus its `Message-ID`), Tab/BackTab cycle To/Cc/Subject/Body, Ctrl-S sends
  in the background (`App::send_compose`, reported over a new `send_rx` channel, same
  spawn-and-report pattern as mail sync and NLP parses), Esc cancels. Reply-all's Cc merges the
  original's To+Cc, dropping the direct recipient and the sending account's own address. Pure
  logic (`reply_subject`/`forward_subject`/`chain_references`/`quote_original`/
  `merge_reply_all_cc`) is unit-tested directly (`tests/it/email_compose.rs`, 8 tests); the
  compose-editing mechanics (push/backspace/field-cycling/newline) are tested through a real `App`
  (`tests/it/app.rs`, 4 tests) — deliberately not testing `SmtpConfig::for_account`'s env-gated
  path in-process, matching this project's existing convention of never mutating global env vars
  inside the shared test binary (see `tests/it/email_config.rs`). Tests: 149 in-process (was 137).
- 2026-09-27 (email send, TUI coverage): `tests/tui/fakesmtp.py`, a threaded STARTTLS SMTP server
  reusing `fakeimap.py`'s throwaway CA, speaking exactly what `src/email/smtp.rs` sends (EHLO,
  STARTTLS, second EHLO, `AUTH LOGIN`, `MAIL FROM`/`RCPT TO`/`DATA` with dot-stuffing, QUIT); a
  mode file rejects `AUTH LOGIN` or every `RCPT TO` to exercise "Send failed: ...". Wired into
  `tuidrive.py` (`start --smtp`, `smtp mode <mode>`, `smtp log`, `Sandbox.smtp()`/`smtp_log()`;
  `Sandbox.env()` merges the IMAP and SMTP CAs into one `SSL_CERT_FILE` when both fakes run in one
  sandbox). Six new `tui_suite.py` scenarios: `email_compose_send`, `email_compose_cancel`,
  `email_reply_send` (asserts `In-Reply-To`/`References` against the recorded message),
  `email_reply_all_cc` (Cc merge), `email_forward_send` (no threading headers), `email_send_auth_fail`
  (status text and that the draft survives). Suite: 142 (was 136), 0 XFAIL, 0 regressions. No bug found
  in `src/email/smtp.rs`; every protocol step and UI behaviour matched on the first automated run once
  the scenarios were written. Docs: [`TUI_FAKES.md`](../TUI_FAKES.md) gained a "Fake SMTP" section.
- 2026-09-23 (email features, vim motions): items 2-5 of [`future-features.md`](../future-features.md).
  Email: `s` spawns one sync and reports "N new" / "All emails gathered"; the three copies of the fetch
  pipeline are now `email::sync::sync_account`; the list sorts by a display-time priority score
  (`src/email/priority.rs`, `▲`/`△`, `o` toggles date order); opening a message asks Ollama for a summary,
  cached in `email_messages.summary` (idempotent `ALTER` in `src/migrations.rs`, a failed summary is not
  cached). `TRIPTYCH_OLLAMA_URL` overrides the Ollama host and `tests/tui/fakeollama.py` stubs it
  ([`TUI_FAKES.md`](../TUI_FAKES.md)). Vim: counts, `gg`/`G`/`NG`, `0`/`$`, `Ctrl-d`/`Ctrl-u`, `/` search with
  `n`/`N` in the todo and email lists, calendar and popup included (`src/app/motion.rs`, `search.rs`); other
  Ctrl chords are now ignored so `Ctrl-d` never types `d`. Priority reordering sinks a converted email, which
  broke `email_stray_output`; the scenario was changed, not the code. Tests: 133 in-process, suite 136
  (16 `vim_*`, `imap_*` and `email_*` extended).

## Resolved issues

Indexed in [`../DEVELOPMENT.md`](../DEVELOPMENT.md#known-issues).

- **KI-27** (2026-10-20) `save_attachments_to` wrote each attachment under its own name, so two with the
  same name (a signature `image.png`) overwrote each other and the status still said "Saved 2". Names
  are now made unique by `attachment_filenames`. `attachment_filenames_never_reuse_a_name`.
- **KI-10** Email detail popup scroll was unclamped. `render_email_detail_popup` (`src/ui.rs`) clamps
  the offset using `Paragraph::line_count`, which needs ratatui's `unstable-rendered-line-info`
  feature (enabled in `Cargo.toml`). Its count includes block borders. `email_detail_scroll`,
  `email_detail_long_scroll`.

### Earlier fixes (not numbered)

- No way to read an email's body in the TUI - only a 200-char snippet was stored, and nothing
  rendered it beyond the list. Fixed 2026-09-18: `body_text` column + `v` detail popup (see
  Changelog).
