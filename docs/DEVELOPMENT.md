# Development Log

Dev workflow, changelog, known issues for Triptych. See [`../CLAUDE.md`](../CLAUDE.md) (module
map, build/test/lint), [`roadmap.md`](./roadmap.md), [`roadmap-email.md`](./roadmap-email.md).

## Workflow

```
cargo build
cargo test      # tests/it/ (in-process, in-memory sqlite) + tests/it/cli.rs (spawns the binary)
cargo clippy --all-targets
```

Run all three before considering a change done. Known Issues resolved in place, never deleted.

End-to-end check (drives the real binary in sandboxes, ~1 min; see [`TUI_DRIVER.md`](./TUI_DRIVER.md)):

```
python3 tests/tui/tui_suite.py -j 4      # FAIL = regression; XFAIL = open Known Issue; XPASS = fixed, drop its marker
```

## Changelog

- 2026-09-29 (alerts, canvas titles): desktop deadline alerts (`src/notify.rs`, `sync/notify.rs`; new
  `tasks.notified_tier`, reset when a deadline moves). Canvas titles become `CS-472: Quiz 3`
  (`canvas::tidy_title`; old untouched titles are rewritten on re-poll). Todo ties sort by soonest
  deadline. New `docs/SETUP.md` and `docs/USAGE.md`. The suite sets `TRIPTYCH_NOTIFY_CMD=true` so no real
  alerts fire; scenario `notify_deadline_alert` overrides it with a logger.
- 2026-09-29 (todo order): completed tasks sort below open ones, and toggling completion keeps the
  cursor on the same task (`toggle_completed` uses `reload_tasks_keep_selection`).
- 2026-09-29 (canvas): Canvas assignment sync. `src/canvas.rs` parses the per-user iCal feed
  (`CANVAS_ICS_URL`) and upserts VEVENTs as tasks keyed on new `tasks.external_id` (unique partial
  index, added in `src/migrations.rs`). `sync/canvas.rs` polls every 15 min inside the TUI;
  `triptych canvas sync` runs it once. TUI list now refreshes on the 2s tick. Tests: `tests/it/canvas.rs`,
  two in `tests/it/cli.rs`, scenario `canvas_feed_syncs_into_todo`. Limits: assignments removed from
  the feed are not deleted; a re-poll updates only `deadline` of open tasks; imported tasks are not
  auto-allocated until `schedule reallocate`; all-day dates become 23:59 local.
- 2026-09-29 (roadmap): recorded that Canvas assignment sync is not built (design in `future-features.md`)
  and that integrating the todo list into the calendar is planned but low priority.
- 2026-09-29 (verification): `todo_add_cold_model` fails under `-j 4` on the pre-change commit too (3 of 3 runs),
  so it is not a regression from the sort/reword work. `todo_add_llm_nonblocking` failed once in 3 runs on
  the new code (22.6s, real Ollama) and 0 of 3 on the old; unexplained, watch it.
- 2026-09-29 (todo reword): `e` in the todo list reopens the input prompt pre-filled with the selected
  task; Enter saves it as the new description (no NLP re-parse, so date/priority/tags stay). Tests:
  `reword_task_updates_description_and_keeps_selection`, TUI scenario `todo_reword`.
- 2026-09-29 (formatting): added `rustfmt.toml` (edition 2024, width 100, stable options only) and ran
  `cargo fmt` over the tree once; `cargo fmt --check` is now part of the pre-done checks.
- 2026-09-29 (todo order): `App::load_tasks` now stable-sorts the list by `urgency::effective_priority`
  (highest first, ties keep `item_order`), so date-raised tasks rise too. The DB `item_order` is
  unchanged. `load_tasks_orders_by_priority_then_item_order`.
- 2026-09-27 (next-weekday date bug): "math homework due next monday" said on a Sunday resolved to
  tomorrow instead of the Monday 8 days out. Root cause: `rules.rs`'s `parse_date_phrase` handed any
  `next <weekday>` match to `chrono_english::parse_date_string` with `Dialect::Us`, and that crate's
  US dialect treats an explicit "next friday" the same as bare "friday" (the very next occurrence) —
  only `Dialect::Uk` adds the extra week an explicit "next" implies. Confirmed by reading the
  installed crate source (`chrono-english-0.1.8/src/types.rs`): only `Direction::Next`'s branch
  checks the dialect flag, so `last`/`this`/bare weekdays are unaffected, and numeric dates (`12/25`)
  never reach `chrono_english` at all (`parse_numeric_date` handles those separately) — a one-word
  fix, `Dialect::Us` -> `Dialect::Uk`, with no other call site touched. New `tests/it/nlp_rules.rs`
  case `deadline_by_next_weekday_skips_the_immediate_occurrence` computes tomorrow's weekday name at
  runtime and asserts the resolved deadline lands 7 days past it, so the regression can't drift with
  the calendar. All pre-existing "next weekday" tests only asserted the resolved weekday name, never
  a date offset, so none needed changes. Full gate green: `cargo build`/`cargo clippy --all-targets`
  clean, `cargo test --test it` 228 passed (was 227), full TUI suite 168/168. See
  `src/nlp/CLAUDE.md`'s Weekdays gotcha.
- 2026-09-27 (full-body deadline fallback): `App::convert_selected_email_to_task` used to only look
  at an email's subject + snippet for a deadline; a date stated further down the body (past what the
  list view shows) was silently dropped. It's now `async`: when the subject+snippet parse finds no
  deadline, it fetches the full body via `email::store::get_body`, cleans it with the new
  `email::message::clean_body_for_deadline_scan` (strips quoted-reply chains and signature blocks,
  since a bare date in either is far likelier to be someone else's older message or footer noise than
  the sender's own ask), and runs the result through the new `nlp::rules::extract_deadline_only`
  (reuses `parse_segments` but keeps only a trigger-word-anchored `Segment::Deadline` — "by"/"due"/
  "before" + date — discarding every bare `Segment::Date`). The result feeds `submit_task`'s new
  `body_deadline` param, which `insert_parsed_task` applies via `.or()` only when the parse's own
  `deadline` is `None` — this fallback can only ever supply a `deadline`, never a title or `due_date`.
  New scenarios `email_convert_body_deep_due` (date only in body prose, title still stays the subject)
  and `email_convert_quote_skip` (deadline-shaped phrase inside a quoted reply must not leak through)
  bring the TUI suite to 168 scenarios (was 166). See `src/app/CLAUDE.md`'s `mail.rs`/`tasks.rs` rows,
  `src/email/CLAUDE.md`'s `message.rs` row, `src/nlp/CLAUDE.md`'s `rules.rs` row.
- 2026-09-27 (sender-domain filter, Slice 25): `@` in the email list cycles `App.domain_filter`
  (`Option<String>`) through `None` (merged) and every sender domain with a stored message,
  alphabetical, then back — same dynamic-list cycle shape as `cycle_account_filter`/
  `cycle_folder_filter`, via new `App::cycle_domain_filter` and `email::store::distinct_domains`
  (`SELECT DISTINCT substr(from_addr, instr(from_addr, '@') + 1) ...`, mirroring
  `distinct_accounts`/`distinct_folders`). `refresh_emails` gained a matching `.retain()` step,
  last in the filter chain (after `starred_filter`, before snooze). Title bar gained
  `@: domain filter` and a `· domain: {domain}` tag.
  First wired to `G` for "domain," which never fired: `src/app/motion.rs` claims `G` for the vim
  `gg`/`G` bottom motion, and per `src/tui/CLAUDE.md`'s "motions run first" rule in the email list,
  the per-view key handler never sees it. Caught by the new TUI scenario failing with `screen lacks
  'domain: ex.com'` on the very first run; root-caused by grepping `MotionKey::Char` in
  `src/app/motion.rs`, then switched every reference (`src/tui/keys.rs`, `src/tui/ui/email.rs`,
  `src/app/mail.rs`, `src/app.rs`) from `G` to `@`, confirmed free via `grep -n
  "MotionKey::Char\|Char('@')"` across both files. New `tests/it/app.rs` case
  `cycle_domain_filter_walks_every_domain_then_back_to_merged` (215 in-process tests, was 214). New
  scenario `email_domain_filter` seeds two messages at different domains and drives all three
  filter states (166 TUI scenarios, was 165). Full gate green: `cargo build`/`cargo test` (215
  passed)/`cargo clippy --all-targets` clean, full TUI suite 166/166. See
  `docs/roadmap-email.md`'s Slice 25, `src/app/CLAUDE.md`'s `mail.rs` row, `src/email/CLAUDE.md`'s
  `store.rs` row, `src/tui/CLAUDE.md`'s `keys.rs`/`ui.rs` row.
- 2026-09-27 (unread/starred filter chips, Slice 24): `U` and `S` in the email list cycle
  `App.unread_filter`/`App.starred_filter` (both `Option<bool>`) through merged (all mail) ->
  matching only -> non-matching only -> merged, via `App::cycle_unread_filter`/
  `App::cycle_starred_filter` — same fixed 3-state shape as Slice 23's `cycle_attachment_filter`,
  since `EmailMessage.is_read`/`is_starred` are always known, never unclassified. `refresh_emails`
  gained two more `.retain()` steps right after `attachment_filter`'s. Title bar gained `U: unread
  filter`/`S: starred filter` and `· unread: Yes`/`No` / `· starred: Yes`/`No` tags.
  `handle_email_filter_key` (`src/tui/keys.rs`) grew from four to six filter-cycle keys. New
  `tests/it/app.rs` cases cover both full 3-state cycles (214 in-process tests, was 212). New
  scenarios `email_unread_filter`/`email_starred_filter` seed two messages, mark one read/starred via
  a raw `UPDATE`, and check all three filter states (165 TUI scenarios, was 163). See
  `docs/roadmap-email.md`'s Slice 24, `src/app/CLAUDE.md`'s `mail.rs` row, `src/tui/CLAUDE.md`'s
  `keys.rs`/`ui.rs` row.
- 2026-09-27 (Rust idiom review): pass over the GPU-efficiency changes plus a repo-wide antipattern
  scan (`&Vec<T>`/`&String` params, reflexive `.clone()`, index loops, manual `Arc<Mutex>`, ad hoc
  statics, missing `Debug`, `Box<dyn Trait>`). `src/nlp/ollama_client.rs`'s `OllamaRequest` changed
  from owning `String`/`Option<String>` fields to borrowing (`OllamaRequest<'a>`, `&'a str` model
  and prompt, `Option<&'static str>` format) — every call site serializes and drops the request
  immediately, so owning a copy of `self.model` and the built prompt allocated for no reason.
  `bulk_mail_heuristic` switched from `to_lowercase()` to `to_ascii_lowercase()` since the keyword
  list is ASCII-only, skipping the Unicode case-folding table. Repo-wide scan found nothing else to
  fix: no raw-`Vec`/`String` signatures, no manual `Arc<Mutex>`/`Rc<RefCell>`, no ad hoc global
  statics, no `Debug`-missing public types (`EmailConfig`/`SmtpConfig`'s custom `Debug` impls that
  redact passwords are intentional), `src/lib.rs`'s `BoxError` is a standard top-level error alias.
  The two `account.clone()` calls in `open_folder_browser`'s IMAP-config loop
  (`src/app/mail.rs`) are not reflexive — `config` is moved into `ImapMailSource::new` before the
  clones happen, so `account` has to be captured before that move and re-cloned per discovered
  folder to give each result tuple its own owned `String`. `cargo build`/`cargo test` (212 passed)/
  `cargo clippy --all-targets` all clean after both fixes; no behavior change, so the TUI suite
  wasn't re-run.
- 2026-09-27 (Ollama/GPU efficiency, triage+summary): four changes to cut GPU load from the email
  feature's background LLM calls. (1) `NLPParser` now owns a second `OllamaClient` (`triage_client`,
  model `qwen2.5:1.5b`) for `summarize`/`triage`, leaving the original 7B client dedicated to
  `parse`'s structured extraction — triage/summary are short classification/summarization tasks that
  don't need `parse`'s accuracy, and the smaller model loads and infers faster. (2) new
  `bulk_mail_heuristic(subject, snippet)` in `src/nlp/ollama_client.rs` matches the same
  `newsletter`/`receipt`/`notification`/`unsubscribe` keyword list the TUI test driver's fake Ollama
  server already treats as bulk mail; `NLPParser::triage` tries it before ever calling Ollama, so an
  obvious newsletter/receipt resolves for free — mirrors the existing regex-first `parse()` pipeline
  (see `src/nlp/CLAUDE.md`). (3) `TRIAGE_BATCH_LIMIT` (`src/app/mail.rs`) lowered 20 -> 10: most rows
  now resolve via the heuristic with no Ollama call at all, so a smaller cap still clears a normal
  backlog in one or two passes while capping worst-case GPU load per pass. (4) every `OllamaRequest`
  (and `warm()`'s ad-hoc JSON) now sends `keep_alive: "30m"`, so Ollama keeps a model resident between
  calls instead of unloading it on its own idle timeout — an unload-then-reload mid-session costs far
  more GPU time than steady inference. `NLPParser::prewarm()` now warms both clients concurrently
  (`tokio::join!`), logging a warning per-client on failure without propagating one. Updated
  `tests/tui/tui_suite.py`'s `email_triage_after_sync` to expect exactly 1 real triage request (only
  "Quarterly planning notes" is ambiguous enough to reach the fake server; "Weekly Newsletter" is now
  caught by the heuristic) — DB-classification assertion unchanged. New
  `tests/it/nlp_llm.rs` cases cover the heuristic's keyword match (case-insensitive) and its `None`
  fallthrough for ambiguous mail (212 in-process tests, was 210). Full TUI suite unaffected
  (163 scenarios). See `src/nlp/CLAUDE.md`.
- 2026-09-27 (attachment-presence filter, Slice 23): `H` in the email list cycles
  `App.attachment_filter` (`Option<bool>`) through merged (all mail) -> has attachments only -> no
  attachments only -> merged, via `App::cycle_attachment_filter` — same fixed 3-state shape as
  `cycle_focus_filter`, but simpler: `EmailMessage.has_attachments` is always known (computed at
  query time by `email::store::get_recent`'s `EXISTS(...)` subquery), so there's no "unclassified"
  bucket to fall out of. `refresh_emails` gained a matching `.retain()` step in the existing
  account -> folder -> focus -> attachment -> snooze chain. No schema change needed — the
  `email_attachments` table already existed (Slice 9). Title bar gained `H: attachment filter` and a
  `· attach: Yes`/`No` tag. Adding a fifth filter-cycle key pushed `handle_email_key`
  (`src/tui/keys.rs`) over clippy's 100-line function limit, so `A`/`F`/`I`/`H` were split into a
  private `handle_email_filter_key` helper. New `tests/it/app.rs` case
  `cycle_attachment_filter_walks_has_then_lacks_then_back_to_merged` covers the full cycle against
  the DB (210 in-process tests, was 209). New scenario `email_attachment_filter` seeds one message
  with an `email_attachments` row and one without, checking the `[attach]` list tag and all three
  filter states (163 TUI scenarios, was 162). See [`roadmap-email.md`](./roadmap-email.md)'s
  Slice 23, `src/app/CLAUDE.md`'s `mail.rs` row, `src/tui/CLAUDE.md`'s `keys.rs`/`ui.rs` row.
- 2026-09-27 (categories / color tags, Slice 22): Outlook's colored-category tagging, one tag per
  message rather than Outlook's several. `t` in the email list or detail popup cycles the selected
  message through a fixed six-color palette (`app::mail::CATEGORY_ORDER`: red, orange, yellow,
  green, blue, purple), wrapping to untagged after purple, via pure `app::mail::next_category` and
  `App::cycle_selected_category` — same shape as `toggle_selected_star`. New nullable
  `email_messages.category` column (idempotent `ALTER TABLE`, `src/migrations.rs`) and
  `email::store::set_category`; `EmailMessage.category` was added to the struct and all three of
  `store.rs`'s `EmailMessage`-selecting queries, but not to `NewEmail` — set post-insert, like
  `is_starred`, never at parse time. Rendered as a `[color]` list-row tag (new `category_color`
  helper, named ANSI colors only — `LightRed`/`Magenta` approximate orange/purple, ratatui has no
  true variant for either) and a "Category: <Name>" detail-popup line; both title-bar hints gained
  `t: category`. New `tests/it/app.rs` cases cover `next_category`'s wraparound and
  `cycle_selected_category`'s full DB cycle (209 in-process tests, was 207). New scenario
  `email_category_cycle` drives all six colors plus the wrap, checking both the DB column and the
  list-row tag (162 TUI scenarios, was 161). See [`roadmap-email.md`](./roadmap-email.md)'s Slice 22,
  `src/app/CLAUDE.md`'s `mail.rs` row, `src/email/CLAUDE.md`'s `message.rs`/`store.rs` rows,
  `src/tui/CLAUDE.md`'s `keys.rs`/`ui.rs` row.
- 2026-09-27 (rule actions `archive`/`delete`, Slice 21): extends Slice 18's rules
  (`subject`/`from` match on `star`/`read`) with the two actions that need a live IMAP round-trip.
  `parse_rule_spec` now also accepts `archive`/`delete`. `apply_rule_action` (`src/app/mail.rs`) took
  `email_id: i64`; it now takes `&EmailMessage`, since `archive`/`delete` need `account`/`folder`/
  `uid` too. Those two actions hand off to new `spawn_archive`/`spawn_delete` helpers, factored out of
  `archive_selected_email`/`delete_selected_email` — the manual `a`/`d` keypress and a rule match now
  share one `tokio::spawn` + `mpsc`-report code path and one cleanup method
  (`apply_archive_result`/`apply_delete_result`) — `run_email_rules` still runs inline right after
  every sync, never awaiting the network itself, since `apply_rule_action` only ever spawns and
  returns for these two actions. No IMAP config for the account, or an out-of-range UID, silently
  no-ops the action (still marks the email checked), same guard-clause shape the manual paths use.
  New `tests/it/email_rules.rs` case covers `parse_rule_spec` accepting both actions; new
  `tests/it/app.rs` case covers the no-config no-op path (207 in-process tests, was 205). New
  scenario `email_rule_archive_delete` drives both actions end-to-end against the fake IMAP server
  (161 TUI scenarios, was 160).
  Verifying it exposed a real concurrency bug in the *test fixture*, not `triptych` itself: two rule
  actions firing in the same sync pass each spawn their own IMAP connection, and this was the first
  scenario in the suite's history to make two connections mutate `tests/tui/fakeimap.py`'s
  `mailbox.json` at the same time. `Mailbox.save()` wrote to a fixed `.tmp` path shared across every
  connection's own `Mailbox` instance (each `Handler` builds a fresh one), so two concurrent saves
  raced `Path.replace` and one threw `FileNotFoundError` — captured directly via a standalone
  reproduction script reading `sb.imap_log()`, not inferred. Fixed with a module-level `_BOX_LOCK`
  wrapped around every load-mutate-save sequence (`STORE`, `MOVE`, `COPY`, `expunge()`) plus a
  per-`(pid, thread)` tmp filename in `save()` as defense in depth; confirmed stable across 5
  standalone repro runs and 3 harness runs with zero failures before trusting it. See
  [`roadmap-email.md`](./roadmap-email.md)'s Slice 21, `src/app/CLAUDE.md`'s `mail.rs` row,
  `tests/tui/CLAUDE.md`'s `Mailbox` concurrency gotcha.
- 2026-09-27 (unified inbox AI triage / Focused Inbox split, Slice 17): every synced email now
  gets a one-time binary classification, Focused or Other, via the same local Ollama model already
  used for parsing and summaries — Outlook's Focused/Other split, no new dependency.
  `email_messages.triage_focused` (idempotent `ALTER TABLE`, `src/migrations.rs`) is `NULL` until
  classified. `OllamaClient::triage`/`build_triage_prompt` (`src/nlp/ollama_client.rs`) mirror
  `summarize`'s shape (ignores the sticky `ollama_available` flag), fencing the subject and a
  400-char-capped snippet as untrusted data, same convention as the summary prompt.
  `App::run_email_triage` (`src/app/mail.rs`) runs a background pass over
  `email::store::pending_triage` (rows with `triage_focused IS NULL`, capped at
  `TRIAGE_BATCH_LIMIT` = 20/pass) after every mail/folder sync, guarded by a `triage_running` flag
  so passes never overlap; results land over a new `triage_rx` channel, applied in place
  (`apply_triage`, wired into `run_app`'s `tokio::select!` in `src/tui.rs`) without a full
  `refresh_emails` reload. `I` in the email list cycles `App.focus_filter` through `None` (merged)
  -> `Some(true)` (Focused only) -> `Some(false)` (Other only) -> `None`
  (`App::cycle_focus_filter`); `refresh_emails`'s `.retain()` chain gained a `focus_filter` step
  right after `folder_filter` — a not-yet-classified message matches neither `Some` state, so it
  drops from both filtered views until triage catches up. UI: `· focus: Focused`/`Other` in the
  title, `[other]` row tag (Focused stays untagged, same as the folder tag's "don't clutter the
  common case" convention).
  New `tests/it/nlp_llm.rs` case asserts the triage prompt fences subject/snippet as untrusted data
  (181 in-process tests, was 180); new scenarios `email_focus_filter` (drives `I` end-to-end
  against seeded rows) and `email_triage_after_sync` (real fake-IMAP sync + fake-Ollama
  classification, asserting the DB is actually populated by the background pass) (158 TUI
  scenarios, was 156). `tests/tui/fakeollama.py` gained a `"triage"` prompt kind, answering
  `{"focused": false}` when the fenced `<email>` body contains a bulk-mail keyword ("newsletter"/
  "receipt"/"notification"/"unsubscribe"), `{"focused": true}` otherwise — matched against only the
  fenced section, since the prompt's own instructions name those same keywords (and the literal
  substring `<email>`, in "the `<email>` tags") as examples before the real fence appears, so the
  regex requires the newline that only follows the genuine opening tag (`r"<email>\n(.*)</email>"`)
  to avoid the instructions text itself being swallowed into the classified haystack — caught via
  `email_triage_after_sync` initially failing with every message misclassified `Other`, root-caused
  with a standalone regex repro before fixing. Full gate green: `cargo build/clippy` clean, `cargo
  test` 181/181, TUI suite 158/158. See [`roadmap-email.md`](./roadmap-email.md)'s Slice 17,
  `src/app/CLAUDE.md`'s `mail.rs` row, `src/nlp/CLAUDE.md`'s `ollama_client.rs`/`parser.rs` rows,
  `src/tui/CLAUDE.md`'s key-binding table, `docs/TUI_FAKES.md`.
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
  tests; TUI suite 156/156). See [`roadmap-email.md`](./roadmap-email.md)'s Slice 16,
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
  [`roadmap-email.md`](./roadmap-email.md)'s Slice 15, `src/email/CLAUDE.md`'s `client.rs`/`sync.rs`
  rows, `src/sync/CLAUDE.md`'s `mail.rs` row and Pattern section, `tests/tui/CLAUDE.md`'s
  `fakeimap.py` row.
- 2026-09-27 (smart NLP extraction from email body, Slice 14): converting an email to a task (`c`
  in the email list) used to parse the subject alone; `App::convert_selected_email_to_task`
  (`src/app/mail.rs`) now folds `email.snippet` (the 200-char body preview, already loaded on every
  list row, no extra DB fetch) in alongside the subject whenever it's non-empty, so a due date or
  `!!`-style priority marker sitting only in the body reaches the same regex/LLM pipeline that
  already understands "by tomorrow". Per `src/nlp/CLAUDE.md`, unresolved input becomes the task's
  title verbatim, so blindly parsing subject+body risked garbled titles on any email with no
  recognizable phrase. Fixed with a new `title_override: Option<String>` on `App::submit_task`
  (threaded through `TaskParse`/`apply_task_parse`/`insert_parsed_task`, applied before
  `classify_task` so category classification also sees the real title): when set, it replaces
  whatever title the parse produced, right before insert. `convert_selected_email_to_task` passes
  the bare subject as the override only when it actually folded a snippet in; an email with no
  snippet skips the override entirely and keeps the parser's own cleaned-up title exactly as
  before this slice (caught in self-review: an unconditional override would have regressed the
  existing `email_convert_task` scenario, whose seeded emails always carry a non-empty default
  snippet, by putting the raw, unstripped subject into every converted task's title instead of the
  parser's cleaned version). `submit_task`'s other two call sites (`src/tui/keys.rs`'s general
  non-email background add, an existing `tests/it/app.rs` test) pass `None`, unchanged. New
  `tests/it/app.rs` case
  `converting_an_email_extracts_a_deadline_from_the_body_but_keeps_the_subject_as_title` (a subject
  with no date phrase, a snippet that has one) (180 in-process tests, was 179); new scenario
  `email_convert_body_due` covers the same end-to-end (154 TUI scenarios, was 153). See
  [`roadmap-email.md`](./roadmap-email.md)'s Slice 14, `src/app/CLAUDE.md`'s `tasks.rs`/`mail.rs`
  rows.
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
  [`roadmap-email.md`](./roadmap-email.md)'s Slice 13, `src/app/CLAUDE.md`'s `mail.rs` row,
  `src/email/CLAUDE.md`'s `client.rs`/`sync.rs`/`store.rs` rows, `src/tui/CLAUDE.md`'s key-binding
  table, `docs/TUI_FAKES.md`.
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
  [`roadmap-email.md`](./roadmap-email.md)'s Slice 12, `src/app/CLAUDE.md`'s `mail.rs`/`model.rs`
  rows, `src/email/CLAUDE.md`'s `store.rs` row, `src/tui/CLAUDE.md`'s key-binding table.
- 2026-09-27 (per-account filter, Slice 11): `A` in the email list cycles `App.account_filter`
  through `None` (merged, the default) and every account with >= 1 stored message, alphabetical
  (new `email::store::distinct_accounts`), then back. `App::cycle_account_filter` picks the next
  label and calls `refresh_emails`, which retains only that account's rows on `App.emails` itself
  right after the `get_recent` load and before the priority/date sort — filter-at-source, the same
  pattern `email_sort` already uses to reorder `App.emails` in place, so every action indexing into
  `App.emails`/`selected_email` (open, delete, archive, star, convert-to-task, search) stays
  correct with no parallel filtered view to keep in sync. The list title now shows
  `sorted by {sort} · account: {label}` while a filter is active. New `tests/it/app.rs` cases cover
  the filter-retain behavior and a full cycle through two accounts and back to merged (172
  in-process tests, was 170); new scenario `email_account_filter` drives the `A` key across three
  seeded emails spanning two accounts (151 TUI scenarios, was 150). See
  [`roadmap-email.md`](./roadmap-email.md)'s Slice 11, `src/app/CLAUDE.md`'s `mail.rs` row,
  `src/email/CLAUDE.md`'s `store.rs` row, `src/tui/CLAUDE.md`'s key-binding table.
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
  [`roadmap-email.md`](./roadmap-email.md)'s Slice 10, `src/app/CLAUDE.md`'s `mail.rs` row,
  `src/email/CLAUDE.md`'s `drafts.rs` row, `src/tui/CLAUDE.md`'s key-binding table.
- 2026-09-27 (full-text search over email body, Slice 9): `/` in the Email view previously only
  matched the in-memory list rows' subject and sender, since `store::get_recent` deliberately never
  loads `body_text` into them. New `email::store::search_body_matches(pool, needle)` runs one
  `SELECT id FROM email_messages WHERE body_text LIKE ? ESCAPE '\'` (`%`/`_` escaped) and returns
  the matching ids as a `HashSet`; `App::search_step` unions that against the existing subject/
  sender check before walking the list with `motion::find_match` — only for the email view, the
  todo list's search stays purely in-memory. `App::commit_search`/`search_step` are now `async` (the
  one DB round-trip), so `keys.rs`'s three call sites (search prompt `Enter`, `n`/`N` in the todo
  and email views) gained an `.await`, as did all eight existing search tests in `tests/it/app.rs`.
  New `email_search_matches_body_text_via_db_query` test (165 in-process tests, was 164) and new
  scenario `vim_email_search_body`, which searches a phrase present only in one seeded row's body
  (149 TUI scenarios, was 148).

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
  [`roadmap-email.md`](./roadmap-email.md)). New `tests/it/email_thread.rs` (4 tests). Considered
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
  the scenarios were written. Docs: [`TUI_FAKES.md`](./TUI_FAKES.md) gained a "Fake SMTP" section.
- 2026-09-23 (email features, vim motions): items 2-5 of [`future-features.md`](./future-features.md).
  Email: `s` spawns one sync and reports "N new" / "All emails gathered"; the three copies of the fetch
  pipeline are now `email::sync::sync_account`; the list sorts by a display-time priority score
  (`src/email/priority.rs`, `▲`/`△`, `o` toggles date order); opening a message asks Ollama for a summary,
  cached in `email_messages.summary` (idempotent `ALTER` in `src/migrations.rs`, a failed summary is not
  cached). `TRIPTYCH_OLLAMA_URL` overrides the Ollama host and `tests/tui/fakeollama.py` stubs it
  ([`TUI_FAKES.md`](./TUI_FAKES.md)). Vim: counts, `gg`/`G`/`NG`, `0`/`$`, `Ctrl-d`/`Ctrl-u`, `/` search with
  `n`/`N` in the todo and email lists, calendar and popup included (`src/app/motion.rs`, `search.rs`); other
  Ctrl chords are now ignored so `Ctrl-d` never types `d`. Priority reordering sinks a converted email, which
  broke `email_stray_output`; the scenario was changed, not the code. Tests: 133 in-process, suite 136
  (16 `vim_*`, `imap_*` and `email_*` extended).
- 2026-09-23 (feasibility): [`future-features.md`](./future-features.md) now holds a verdict, size and
  approach for each of the 8 requests. Item 8 is not reproduced as worded; the likely cause is the Calendar
  `s` picker saying "No unscheduled tasks available." for tasks already scheduled by todo `s` or calendar `a`.
- 2026-09-23 (KI-22..24, IMAP harness): a manual task now shows in every hour it spans (KI-22); an idle
  IMAP sync no longer refetches the last message (KI-23); the Email view reloads while open (KI-24).
  New `tests/tui/fakeimap.py` (local TLS IMAP server) lets the suite run the real `email sync` and the TUI's
  background sync: 14 `imap_*` scenarios. The 2s reload briefly blanked the open body popup (`get_recent`
  rows carry no body); the existing `email_detail_long_scroll` caught it, so the tick skips the popup.
  Tests: 93 in-process, suite 110.
- 2026-09-19 (KI-21, background add): `submit_task`/`apply_task_parse` (see KI-21). The Ollama scenarios
  share one model and used to race under `-j`: `daemon_prewarm_loads_model` and `todo_add_cold_model`
  unload it, and every parallel TUI start reloads it, which likely caused the one unexplained suite FAIL
  earlier today. The three now take a file lock (`_serialized`) and `_unload()` retries. Tests: 90
  in-process, suite 95.
- 2026-09-19 (KI-20, cold model): the TUI never waits for a model load. `NLPParser::set_wait_for_load(false)`
  (set in `tui::run`) makes `OllamaClient::parse` return `ModelNotLoaded` at once when `/api/ps` shows the
  model absent; the parser falls back to regex and warms the model in the background. The CLI and daemon
  still wait (90s). The 90s load limit had raised the worst-case TUI freeze from 15s to 90s. New scenario
  `todo_add_cold_model` (failed on the old code, passes now). KI-20 fixed at its root: the prompt example
  held the placeholder `<last day of next month>`, which the 7B copied (3/3), so the deadline never
  parsed; the example now holds a real date, and the prompt asks for local time. Tests: 88 in-process,
  suite 94. Found KI-21.
- 2026-09-19 (think off): `OllamaRequest` sends `think: false`. Without it `qwen3.6:27b` (a thinking
  model) put its output in a `thinking` field and returned an empty `response`, so every parse fell back
  to regex ("EOF while parsing"). With it: valid JSON, and `triptych add` in a sandbox saved the right
  deadline and 180 min (15.9s including the load). `qwen2.5:7b` is unaffected. Found KI-20 on the way.
- 2026-09-19 (KI-19, LLM prewarm): startup now loads the model through `NLPParser::prewarm` (both the
  TUI's `SyncDaemon` and `triptych daemon`), and a parse that has to wait for a load gets a 90s limit
  instead of 15s. New scenario `daemon_prewarm_loads_model` (unloads the model, starts the daemon, polls
  `/api/ps`; needs a running Ollama). Tests: 83 in-process, suite 93/93.
- 2026-09-19 (LLM baseline): `NLPParser::parse` now logs the winning layer and latency. Measured on
  M3 Max 36GB, Ollama 0.34.2, the real `build_prompt` (~670 prompt tokens, 50-75 output tokens), model
  load included in "first load". Warm parse: qwen2.5:7b 0.9-1.5s, 1.5b 0.33-0.48s, 0.5b 0.29-0.42s.
  Reload after a model is evicted (5m default `keep_alive`, waited it out): 7b 3.6s wall, 1.06s load.
  First-ever load: 7b 26s, 1.5b 18s, 0.5b 17s (`llama-server started in ..`, from the Ollama log),
  then 0.5-1.0s on later loads. `keep_alive: -1` only removes the reload, not the first load.
  Hit rate: 2/25 probe inputs reach the LLM (only unresolved deadline phrases); the probe corpus is
  the suite's regex-covered inputs plus 8 others (3 from todo.db), so real usage still needs the new log
  line (todo.db holds 3 real inputs). Not measured: cold-page-cache load after hours idle (needs
  `sudo purge`). Findings: KI-19.
- 2026-09-19 (delete fix, visual select): Deleting a task that an email was converted into failed
  with `FOREIGN KEY constraint failed` (KI-18). New: `v`/`V` in the todo list start a visual
  selection (`j`/`k` extend, `Esc` or any other key cancels) and `x`/`d`/`D` delete the whole range in
  one transaction; with no selection they delete the cursor row. Tests: 83 in-process, suite 92/92.
- 2026-09-19 (ui split): `src/tui/ui.rs` (873 lines) split into a 44-line root (`ui()` dispatch,
  `centered_rect`) plus `src/tui/ui/{todo,calendar,email,grid,popups}.rs`. `triptych::ui::*` paths
  are unchanged: the root re-exports `CalendarGrid`, `CellView`, `TimeSlot`, `build_cell_view`,
  `cell_task_displays` and `urgency_style`, so `tests/it/ui.rs` did not change. Added
  `src/tui/CLAUDE.md`. Suite 89/89, 75 tests, clippy clean.
- 2026-09-19 (tui/cli dirs): Grouped the flat `src/` files into two directories, the same
  `foo.rs` + `foo/` pattern `app` uses. `src/tui.rs` now parents `tui/{keys,ui}.rs`; `src/cli.rs`
  (clap definitions) parents `cli/{commands,daemon}.rs`. `triptych::ui` is unchanged (`lib.rs`
  re-exports `tui::ui`), so no test imports moved. `keys`, `commands` and `daemon` stay private.
  Not moved: `urgency.rs` (shared by `ui` and the CLI `list`), `logging.rs`, `migrations.rs`. The
  entries below name the old flat paths.
- 2026-09-19 (rust-idioms follow-up): Closed the items the app split left open. `run()` and the
  other entry points return `BoxError` (`Box<dyn Error + Send + Sync>`, alias in `src/lib.rs`).
  `App`, `NLPParser`, `OllamaClient` and `ImapMailSource` derive `Debug`, and `missing_debug_implementations`
  is now a lint. `EmailConfig` has a hand-written `Debug` that prints the IMAP password as
  `<redacted>` (the derived one would have leaked it into any log line; covered by
  `debug_output_redacts_the_password`). `tests/cli.rs` moved to `tests/it/cli.rs`, so all 75 tests
  link as one binary. The four crate-wide clippy allows (`missing_errors_doc`, `missing_panics_doc`,
  `must_use_candidate`, `too_long_first_doc_paragraph`) are gone: 35 `# Errors` sections, 29
  `#[must_use]` attributes and 13 shortened doc summaries. Observation: `NLPParser::parse` never
  returns `Err` (`ParseError::InvalidInput` is never built), so its `# Errors` section says so;
  making `parse` infallible is a possible follow-up. Flake: `daemon_lifecycle` failed twice in a row at
  `-j 4` under machine load (load average ~10) and passed 3 of 3 alone and on every rerun.
- 2026-09-19 (app split): `src/app.rs` (2250 lines) split into `src/app.rs` (the `App` struct, `build`,
  view toggles) plus `src/app/{model,time,tasks,allocation,calendar,placement,schedule_io,mail}.rs`;
  public paths (`triptych::app::*`) are unchanged via `pub use`. Methods and helpers shared between
  the files are `pub(super)`. A repo-wide `cargo fmt` was run (it reformatted `daemon.rs`,
  `email/client.rs`, `nlp/parser.rs`, `sync/{config,mail}.rs`), and a `rust-idioms` review narrowed
  module visibility (`cli`, `commands`, `daemon`, `keys`, `sync`, `tui` private; `email::{client,store}`
  and `nlp::ollama_client` `pub(crate)`), added `Debug` to seven public types and set `publish = false`.
  The items this left open are closed in the entry above. See
  [`../src/app/CLAUDE.md`](../src/app/CLAUDE.md). Older entries below name `src/app.rs`.
- 2026-09-19 (restructure): Repo laid out like a standard Rust package. The crate is now a library
  (`src/lib.rs`, named `triptych`) plus a 5-line `src/main.rs`; the old `main.rs` split into
  `src/commands.rs` (non-interactive CLI), `src/tui.rs` (terminal setup and event loop) and
  `src/logging.rs`. The binary is `target/debug/triptych` (was `Triptych`). All tests now live under
  `tests/`: the inline `mod tests` blocks became `tests/it/` (one test binary, 60 tests), `tools/`
  became `tests/tui/`, and `tests/cli.rs` is unchanged (it later moved to `tests/it/cli.rs`). Lints moved into `[lints]` in `Cargo.toml`,
  and clippy is clean with `--all-targets` (the old `EmailMessage` dead-code warnings vanished
  because the library's `pub` items are no longer dead). Items the tests call became `pub`. See
  [`../tests/CLAUDE.md`](../tests/CLAUDE.md). Older entries below name the old paths.
- 2026-09-19 (latest): Fixed KI-14..KI-17, found by a sandboxed todo-list test with college-student
  tasks. Bare weekday parsing (`src/nlp/rules.rs`), `[LOW]` and `[DUE ...]` badges, and automatic
  priority escalation near a date, all in the new `src/urgency.rs` and shared by `ui.rs` and the
  CLI `list`; the TUI badges are colour-coded by urgency (brighter red as it rises). Suite: 89
  scenarios, 0 XFAIL; 60 unit + 14 CLI tests. The CLI test
  `nlp_parses_tags_priority_and_relative_date` moved to a far-future date, because "tomorrow !!" now
  correctly shows `[URGENT↑]`; `priority_rises_when_the_date_is_near` covers the raise.
- 2026-09-19 (later): Fixed KI-1..KI-13 using the driver: each scenario reproduced its bug before the
  fix and passes after (suite: 82 scenarios, 0 XFAIL). Details and file references are under
  Resolved. Largest changes: `src/nlp/rules.rs` time/date parsing (KI-2, 8 new unit tests), the
  async deadline parse (KI-13), and removal of the fuzzy cache (KI-1). No known bugs remain open.
- 2026-09-19: Feature audit plus a headless TUI driver. Audited every shipped feature by running the
  real binary (CLI, daemon, TUI) in sandboxes and found 13 bugs, all filed under Known Issues
  (KI-1..KI-13) and none fixed yet. Built `tests/tui/tuidrive.py` (detached pty session an agent can
  `send` keys to and read the screen from, plus `db`/`cli` against the same sandbox) and
  `tests/tui/tui_suite.py` (81 scenarios: 57 pass, 24 are expected failures that reproduce the open
  issues). Docs: [`TUI_DRIVER.md`](./TUI_DRIVER.md), [`../tests/tui/CLAUDE.md`](../tests/tui/CLAUDE.md),
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

None.

Every finding (KI-1..KI-25) is fixed and covered by a passing test.

### Resolved

Found in the 2026-09-19 audit, the todo-list test and the LLM baseline (KI-1..21), then in the 2026-09-23
calendar and IMAP runs (KI-22..24), then in the 2026-09-27 Slice 10 verification run (KI-25); fixed the
same day, each verified with its `tests/tui/tui_suite.py` scenario (named at the end of each entry).

- **KI-25** (2026-09-27) `tests/tui/tuidrive.py`'s `Term.cursor()` misread the calendar time label
  whenever the current-hour `▸` accent (`src/tui/ui/calendar.rs`) landed on a row also covered by an
  open popup's `│` border — the regex only tolerated one leading decorator character before the
  digits, not two stacked. Test-harness bug only, wall-clock-time-dependent, not a product bug; full
  details and fix in the Changelog above. `cal_cursor_moves`, `vim_cal_motions`,
  `vim_cal_input_literal`.
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
- **KI-22** (2026-09-23) A manually scheduled task showed, and could be picked up, only in its start hour:
  cells matched `scheduled_at.hour() == hour` and the cache carried no duration (allocations had one, but
  their check missed a start off the hour, such as 12:30). One rule, `span_covers_hour`, now serves
  `cell_tasks`, the grid render and the auto-scheduler's free-slot check, so a 180 min task from 07:00
  fills 07-09 and a 12:30 hour spans 12 and 13. Not covered: `find_next_available_slot` still does not
  check that the task being placed fits before the next occupied hour. `cal_long_task_spans`,
  `tests/it/app.rs` (two tests).
- **KI-21** An add whose text reached the LLM blocked the TUI event loop for the parse (~1-1.5s with a
  warm 7B, up to 15s if Ollama hung): `App::add_task` was awaited inline in `tui/keys.rs` and, for
  email conversion, in `app/mail.rs`. Now `App::submit_task` spawns the parse and closes the popup at
  once; the result arrives on `task_rx` and `apply_task_parse` inserts it (the same shape as KI-13's
  `deadline_rx`). Email conversion carries the email id through, so `finish_email_conversion` links the
  new task by id, and a second Enter before the first parse lands is skipped, not duplicated. The task
  goes below the cursor as it is when the parse lands. `add_task` (CLI, tests) still parses inline.
  `add_task_at_selected_cell` never parsed, so it was never affected. `tests/it/app.rs` (two tests),
  `todo_add_llm_nonblocking` (failed on the inline code, passes now).
- **KI-20** The LLM's `deadline` was dropped for "before the end of next month" (3/3 runs, the task saved
  with no deadline although `strategy=Ollama`). Root cause: the `build_prompt` example held the placeholder
  `<last day of next month>T23:59:59+00:00`, and `qwen2.5:7b` echoed it, so no parser could read it.
  The example now holds a real date (`build_prompt` computes it). Also: the prompt asked for UTC, but
  its examples used local hours, so "at 3pm" was stored as 15:00Z (11:00 EDT); it now asks for local time
  with no offset. `parse_timestamp` reads a naive timestamp as local, keeps an offset one, and logs a
  `warn` (field name only) when it drops one. Rerun on the real 7B: 3/3 deadlines saved (23:59:59 EDT),
  "at 3pm" stored as 19:00Z. `tests/it/nlp_llm.rs`.

- **KI-19** The LLM path could never warm up. (1) The prewarm in `src/sync/ollama.rs` and
  `src/cli/daemon.rs` called `nlp.parse` with a plain string, which the regex layer resolves in ~1ms, so
  Ollama was never touched. (2) The first-ever load of a model took 17-26s (see the 2026-09-19 baseline),
  longer than `OLLAMA_TIMEOUT_MS` (15s); the client disconnect aborts the load, so it restarted from zero
  next time. All 28 `/api/generate` calls in `~/.ollama/logs/server.log` (2026-09-17..19) were `499`
  ("client connection closed before llama-server finished loading"), none `200`. Fix: `NLPParser::prewarm`
  calls `OllamaClient::warm` (empty-prompt `/api/generate`, 90s limit) from both prewarm sites (the daemon
  runs it in a spawned task, so the socket answers at once), and `OllamaClient::parse` checks `/api/ps` and
  uses the 90s `OLLAMA_LOAD_TIMEOUT_MS` instead of 15s while the model is not resident. Checked against a
  temporary 17GB default model (`qwen3.6:27b`, since reverted): `add` waited 19.3s and Ollama logged
  `POST /api/generate 200 19.28s`, where the old client would have logged `499` at 15s. `daemon_prewarm_loads_model`.

- **KI-18** Deleting a todo that came from an email (`x`, `triptych rm`, `clear`) failed with
  `FOREIGN KEY constraint failed`: `email_messages.task_id` references `tasks(id)` with no `ON DELETE`
  rule, and SQLite cannot alter one in place. `run_email_migration` now creates the
  `email_messages_unlink_task` trigger (`BEFORE DELETE ON tasks` sets the link to NULL), which covers
  every delete path and existing databases on next start. It must stay after the table rebuild, because
  `RENAME TO` would repoint it at the dropped table. `tests/it/app.rs` (three `*_unlinks_*` tests),
  `todo_delete_email_linked`.
- **KI-14** A bare weekday was not parsed: "call mom on sunday" kept "on sunday" in the title and got
  no date; "friday at 3pm" failed too. `parse_chrono_candidate` (`src/nlp/rules.rs`) now accepts full
  weekday names, resolved to the next such day (never today). Abbreviations (`sat`, `sun`) are left
  alone because they are ordinary words. `nlp_on_weekday`, `nlp_bare_weekday_time`.
- **KI-15** Priority 0 (Low) showed no badge in the TUI or `triptych list`. Both now show `[LOW]`
  through `urgency::priority_badge`. `cli_low_badge`, `todo_escalation_badges`.
- **KI-16** Deadlines ("by friday") showed no badge. Both views now show `[DUE Fri]`, `[DUE TMR]`,
  `[DUE 09/30]` (plus a time when it is not end of day) or `[OVERDUE]`; finished tasks show none.
  `cli_deadline_badge`, `todo_escalation_badges`.
- **KI-17** (feature) Priority now rises as the date nears. `urgency::effective_priority` takes the
  earlier of `scheduled_at` and `deadline` and sets a floor: <=24h or overdue is Urgent, <=3 days
  High, <=7 days Medium. The stored priority never changes and never goes down, so moving the date
  away undoes the raise; a raised badge carries an arrow (`[URGENT↑]`). Finished tasks keep their own
  priority. Display only: the auto-scheduler still orders by deadline and calendar colours are
  unchanged. Also: the TUI hides the time on date-only tasks (`[09/20]`, not `[09/20 12:00am]`).
  The TUI colours the priority, date and deadline badges by the effective level (`urgency_style` in
  `src/ui.rs`): LOW dark gray, MED gray, HIGH red, URGENT bold bright red. Only named ANSI colours
  are used, never RGB or 256-colour indexes, so the user's terminal theme picks the shades.
  `cli_priority_escalates`, `todo_escalation_badges`, `todo_urgency_colors`.

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
- Not covered by the suite: `H`/`L` edge behaviour in the calendar, and IMAP against a real server
  (`fakeimap.py` speaks only what `client.rs` sends: no IDLE, no OAuth, no partial-body fetch). Also unconfirmed: whether editing the deadline of an already-scheduled
  task should drop its existing allocation (observed, not judged a bug).
