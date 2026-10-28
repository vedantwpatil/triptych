# Email triage history

Local-LLM triage and summaries, rules, categories, filters, search, and email-to-task conversion. Newest first.
Hub, workflow and open issues: [`../DEVELOPMENT.md`](../DEVELOPMENT.md). Code:
[`../../src/email/CLAUDE.md`](../../src/email/CLAUDE.md), [`../../src/nlp/CLAUDE.md`](../../src/nlp/CLAUDE.md).
Related: [`email-messages.md`](./email-messages.md), [`nlp.md`](./nlp.md), [`../roadmap-email.md`](../roadmap-email.md).

## Changelog

- 2026-10-27 (email to task key): `Enter` in the list converted the email to a task, surprising anyone
  expecting it to open. `Enter` now opens the message (like `v`) and `T` converts, in the list or the
  open message. Scenario: `email_enter_opens`.
- 2026-10-21 (audit fixes, rules and triage): each item has a test.
  - Rules: a `delete` rule acts only on mail that arrives after it (also the first rule; it used to
    sweep every stored message, a permanent server delete). `rule_applied` is 0 new, 1 checked,
    2 backlog; `create_rule` moves checked mail to 2 (unchecked too for `delete`), `run_email_rules`
    does new mail then the backlog, and `rule_fires` keeps `delete` out of the backlog. The Rules
    popup says so. This supersedes the "each new rule re-runs all rules" line in the 2026-10-21 email
    follow-ups entry of [`email-messages.md`](./email-messages.md).
  - Triage: the drain rests between passes as long as the pass took (at most 10s).
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
  filter states (163 TUI scenarios, was 162). See [`roadmap-email.md`](../roadmap-email.md)'s
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
  list-row tag (162 TUI scenarios, was 161). See [`roadmap-email.md`](../roadmap-email.md)'s Slice 22,
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
  [`roadmap-email.md`](../roadmap-email.md)'s Slice 21, `src/app/CLAUDE.md`'s `mail.rs` row,
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
  test` 181/181, TUI suite 158/158. See [`roadmap-email.md`](../roadmap-email.md)'s Slice 17,
  `src/app/CLAUDE.md`'s `mail.rs` row, `src/nlp/CLAUDE.md`'s `ollama_client.rs`/`parser.rs` rows,
  `src/tui/CLAUDE.md`'s key-binding table, `docs/TUI_FAKES.md`.
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
  [`roadmap-email.md`](../roadmap-email.md)'s Slice 14, `src/app/CLAUDE.md`'s `tasks.rs`/`mail.rs`
  rows.
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
  [`roadmap-email.md`](../roadmap-email.md)'s Slice 11, `src/app/CLAUDE.md`'s `mail.rs` row,
  `src/email/CLAUDE.md`'s `store.rs` row, `src/tui/CLAUDE.md`'s key-binding table.
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

## Resolved issues

Indexed in [`../DEVELOPMENT.md`](../DEVELOPMENT.md#known-issues).

- **KI-12** Pressing Enter twice on a converted email created a duplicate task.
  `convert_selected_email_to_task` now returns early when the email already has a `task_id`.
  `email_convert_twice`.
