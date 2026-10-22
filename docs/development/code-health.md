# Code health history

Repo layout and module splits, lint and idiom passes, audits, docs and planning refreshes. Newest first.
Hub, workflow and open issues (KI-33, the `mail.rs` split): [`../DEVELOPMENT.md`](../DEVELOPMENT.md). Module map:
[`../../CLAUDE.md`](../../CLAUDE.md). Code: [`../../src/app/CLAUDE.md`](../../src/app/CLAUDE.md).
Related: [`testing.md`](./testing.md), [`../roadmap.md`](../roadmap.md).

## Changelog

- 2026-10-22 (binary size): release `triptych` went from 21.0 MiB to 9.3 MiB, found with
  `cargo bloat --release --bin triptych --crates` (without `--bin` it analyses `import_schedule`).
  - `[profile.release]` (`Cargo.toml`): `strip`, `lto = "fat"`, `codegen-units = 1` (21.0 to 12.6 MiB),
    `opt-level = "s"` (to 9.9). The file was 2x its `.text`: symbols and debug names were shipped.
  - `reqwest` without `http2` drops `h2` and hyper's HTTP/2 path (to 9.3). Ollama (localhost) and the
    Canvas feed work over HTTP/1.1; `canvas sync` against an https host still negotiates TLS.
  - Not done: `panic = "abort"` (to 8.6) turns a panic in a background worker into a process exit;
    dropping `tracing-subscriber`'s `env-filter` (about 0.3 MiB of regex) loses `RUST_LOG` directives;
    `aws_lc_sys` (0.7 MiB) and `ring` are two crypto providers pulled by rustls and sqlx.
  - Check: `cargo test` 365 passed, clippy and fmt clean; the release binary ran `add`/`list`.
- 2026-10-21 (audit fixes, module split): `src/app/mail_auto.rs` now holds triage and rules, moved out of
  `mail.rs` (2,026 to 1,739 lines).
- 2026-10-20 (audit fixes, KI-26..KI-32): a read-through of the repo found one data-loss bug and
  several robustness gaps; all fixed except the `mail.rs` split (KI-33, open).
  - Alerts (`src/notify.rs`): `due_alerts` marks `notified_tier` before `send`, and `send` only logged
    a failure, so a broken notifier lost the alert for good. `check_once` now uses a private
    `claim_alerts` that also returns what each alert claimed, `send` returns `bool`, and a failed send
    puts the previous tier back (`WHERE notified_tier = <claimed>`, so a concurrent claim is not
    undone). `due_alerts` keeps its signature. `check_once` returns `CheckReport { shown, failed }`,
    and `triptych notify check` prints `▲ N alert(s) could not be shown` to stderr when any failed.
  - Attachments (`src/app/mail.rs`): `attachment_filenames` gives each attachment a unique name (a
    repeat gets `<n>-` in front); before, two `image.png` overwrote each other while the status said
    "Saved 2".
  - CLI daemon (`src/cli/daemon.rs`): both sides did one `read` into 8 KB, so a longer request or
    response failed to parse. Both now `read_to_end` capped at `MAX_MESSAGE` (1 MiB; the client
    half-closes after writing, so EOF ends the request), and `send_to_daemon` times out after 120s
    (`REQUEST_TIMEOUT`, above the 90s cold-model wait). There is no short timeout on the `Health`
    check on purpose: a second `triptych daemon` started during the first one's preload would
    otherwise treat the live socket as stale and remove it.
  - Canvas (`src/canvas.rs`): `fetch_feed` had no timeout and could hang the worker and
    `canvas sync`; now 30s (checked by hand against a local listener that never answers: the sync
    failed after 30s).
  - Database path (`src/app.rs`): the default `sqlite:todo.db` is relative to the working directory.
    When it has to create the file and `DATABASE_URL` is unset, it prints `Creating a new database at
    <absolute path>` to stderr.
  - Task insert: `insert_task_row(pool, &NewTask)` (`src/app/tasks.rs`) is now the one `tasks`
    INSERT; `insert_parsed_task` and the daemon's `add_task_to_db` both call it (they had drifted
    copies of the same 11-column statement).
  - Tests/docs: `assert_is_empty` (new in clippy 1.99) allowed in `tests/it/main.rs`, so the tree is
    warning-free again. Docs brought in line with the code (module map, sync config, test layout,
    SMTP note) and the `mail.rs` row of `src/app/CLAUDE.md` cut from a 9 KB cell to short groups.
    `daemon_prewarm_loads_model` flaked under `-j 4` (it read `daemon.out` the moment the shared
    model showed as loaded); it now waits up to 15s for the `Pre-warmed` line.
    Suite: 286 `cargo test` tests (4 new); pty suite 182/182.
- 2026-09-30 (docs refresh): `roadmap.md` slices and focus order rewritten (todo and email first, then
  todo in the calendar view, CalDAV last); `roadmap-outlook.md` is now a backlog, not the active goal;
  `README.md` Implemented/Next updated. Corrections: email to task auto-scheduling is done
  (`convert_selected_email_to_task` reaches `on_task_changed`); CalDAV is only a stub (Canvas ICS import
  and invite parsing are not CalDAV).
- 2026-09-29 (roadmap): recorded that Canvas assignment sync is not built (design in `future-features.md`)
  and that integrating the todo list into the calendar is planned but low priority.
- 2026-09-29 (formatting): added `rustfmt.toml` (edition 2024, width 100, stable options only) and ran
  `cargo fmt` over the tree once; `cargo fmt --check` is now part of the pre-done checks.
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
- 2026-09-23 (feasibility): [`future-features.md`](../future-features.md) now holds a verdict, size and
  approach for each of the 8 requests. Item 8 is not reproduced as worded; the likely cause is the Calendar
  `s` picker saying "No unscheduled tasks available." for tasks already scheduled by todo `s` or calendar `a`.
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
  [`../src/app/CLAUDE.md`](../../src/app/CLAUDE.md). Older entries below name `src/app.rs`.
- 2026-09-19 (restructure): Repo laid out like a standard Rust package. The crate is now a library
  (`src/lib.rs`, named `triptych`) plus a 5-line `src/main.rs`; the old `main.rs` split into
  `src/commands.rs` (non-interactive CLI), `src/tui.rs` (terminal setup and event loop) and
  `src/logging.rs`. The binary is `target/debug/triptych` (was `Triptych`). All tests now live under
  `tests/`: the inline `mod tests` blocks became `tests/it/` (one test binary, 60 tests), `tools/`
  became `tests/tui/`, and `tests/cli.rs` is unchanged (it later moved to `tests/it/cli.rs`). Lints moved into `[lints]` in `Cargo.toml`,
  and clippy is clean with `--all-targets` (the old `EmailMessage` dead-code warnings vanished
  because the library's `pub` items are no longer dead). Items the tests call became `pub`. See
  [`../tests/CLAUDE.md`](../../tests/CLAUDE.md). Older entries below name the old paths.
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
  [`src/sync/CLAUDE.md`](../../src/sync/CLAUDE.md); `too_many_lines` on 5 functions that are each one
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
- 2026-09-17: `install_panic_hook()` in `main.rs` — a panic mid-TUI used to skip the raw-mode/
  alt-screen teardown, breaking the terminal. Hook restores it before the panic prints.
- 2026-09-17: Fixed stale multi-account docs (`src/email/CLAUDE.md`, `src/sync/CLAUDE.md`) —
  described old `EmailConfig::from_env() -> Option`; code is `all_from_env() -> Vec`. Reworded
  `mail.rs`'s misleading "TRIPTYCH_EMAIL_ENABLED not set" log line.

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
(see [`email-sync.md`](./email-sync.md): the `SyncCursor` struct, the `last_uid = 0` edge case) - no further issues in
those files on re-review. `message.rs`/`ui.rs` reviewed clean: `Context`/let-else throughout
(no panics on malformed mail), iterator-based body rendering (`body.lines().map(Line::from)`,
zero-copy), `sqlx::Row::try_get` chosen over tuple `FromRow` to avoid relying on an unverified
sqlx API surface. `SyncCursor` derives `Debug, Clone, Copy, PartialEq, Eq` but not `Default` -
deliberate, not an oversight: a "default cursor" (validity 0, uid 0) has no sensible meaning
since epoch 0 never occurs on real IMAP servers.

### Earlier fixes (not numbered)

- Terminal left broken after a panic mid-TUI. Fixed 2026-09-17.
- Stale multi-account docs (`email`/`sync` `CLAUDE.md`). Fixed 2026-09-17.
- Background sync (`tokio::spawn`ed tasks: `mail_sync_worker`, `App::sync_email_accounts`,
  `client.rs`) wrote straight to stderr via `eprintln!`, corrupting the TUI's display mid-
  navigation since it bypasses ratatui's alternate-screen buffer. Fixed 2026-09-18: converted to
  `tracing::debug!/info!/warn!`, routed to a file (`$TMPDIR/triptych.log` by default) via new
  `init_tracing()` in `main.rs` (see Changelog).
