# Email Client — Slice 1 (IMAP fetch → store → view → convert-to-task)

See [`DEVELOPMENT.md`](./DEVELOPMENT.md) for known issues (including a stale-docs item about this
slice's `src/email/CLAUDE.md`/`src/sync/CLAUDE.md`) and [`roadmap.md`](./roadmap.md) for the
overall feature roadmap.

## Context

README's roadmap lists a full email client (IMAP IDLE, OAuth2, multi-account, triage) as a
planned major feature. `.env` already scaffolds `IMAP_SERVER/PORT/USERNAME/PASSWORD/FOLDER`,
`SMTP_*`, and `TRIPTYCH_EMAIL_ENABLED` — confirming the intended auth path is a single
app-password IMAP account, not OAuth2 (OAuth2 needs an external Google Cloud OAuth client
this session can't provision).

This slice builds the first working vertical: fetch mail over IMAP, store it, view it in the
TUI, and convert an email into a task using the existing NLP/task pipeline — the reason this
lives in Triptych rather than as a standalone client. OAuth2, true IMAP IDLE, SMTP send/reply,
and triage actions (archive/snooze) are explicitly deferred to later slices (see below) so
later sessions don't reinvent this scoping conversation. Multi-account landed later as Slice 4
(see below) — this doc's title still says "Slice 1" for historical reasons but now covers both.

## Conventions this slice follows

- **Schema evolution**: guarded ALTER/CREATE in `src/migrations.rs`'s `run_calendar_migration`
  (checked via `column_exists`), called once from `main.rs` after `App::build()`. The
  `migrations/` dir has a single initial-schema `.sql` file untouched since — new schema goes
  into `migrations.rs`, not a new sqlx migration file.
- **Background work**: `src/sync/` — one file per service (`cache.rs`, `calendar.rs`,
  `ollama.rs`), each a `worker(pool, shutdown_rx: broadcast::Receiver<()>)` fn spawned
  conditionally in `SyncDaemon::start` based on a bool in `SyncConfig::from_env()`. Mail sync
  follows this: `src/sync/mail.rs` + `mail_sync_enabled` from `TRIPTYCH_EMAIL_ENABLED`.
- **Views**: `ViewMode` enum in `app.rs`, matched in `ui.rs::ui()` for rendering and in
  `src/keys.rs::handle_key_event` (declared via `mod keys;` in `main.rs`, single dispatch
  entry point) for input.
- **Task creation**: `App::add_task(&mut self, description: &str)` (`app.rs`) runs the full NLP
  pipeline and inserts a `Task`. Email→task conversion calls this directly with the subject
  line rather than duplicating insert logic.
- **Crates**: `async-imap` (`default-features = false, features = ["runtime-tokio"]` — default
  is `runtime-async-std`), paired with `tokio-rustls` (project already pulls in rustls via
  sqlx's `runtime-tokio-rustls`) + a root-cert crate, since async-imap ships no TLS itself.
  MIME parsing via `mail-parser`.
- Active DB file is `sqlite:todo.db` (`DB_URL` const in `app.rs`) — `triptych.db` at repo root
  is unused (0 bytes), leave it alone.

## What's in this slice

1. **Deps** (`Cargo.toml`): `async-imap`, `tokio-rustls`, `rustls-native-certs`, `mail-parser`.
2. **Schema**: `email_messages` table (`uid`, `message_id`, `account`, `folder`, `from_addr`,
   `from_name`, `subject`, `date_utc`, `snippet`, `is_read`, `task_id` FK → `tasks(id)`),
   uniqueness on `(account, message_id)` — not `message_id` alone, since the same Message-ID
   can land in more than one account. No dedicated `accounts` table — see the multi-account
   note below.
3. **`src/email/` module**: `config.rs` (`EmailConfig::all_from_env`), `message.rs`
   (`EmailMessage` + `parse_raw`), `client.rs` (`MailSource` trait + `ImapMailSource`),
   `store.rs` (insert/query/mark-read/link-task/`max_uid` helpers).
4. **Daemon wiring**: `src/sync/mail.rs::mail_sync_worker` polls (not true IDLE) every 60s,
   spawned from `SyncDaemon::start` when `mail_sync_enabled`.
5. **TUI**: `ViewMode::Email`, list view, `'m'` to toggle from the todo list, `Enter` to convert
   the selected email into a task, `r` to mark read.
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
(`src/sync/mail.rs`) loops all configured accounts sequentially each 60s tick, one
`ImapMailSource` per account; a failure syncing one account is logged and doesn't stop the
others. The TUI email view (`ui.rs::render_email_view`) is a merged inbox tagged with each
message's account — no per-account switcher/filter.

## Explicitly deferred

- True IMAP IDLE (push) — polling stands in for now.
- OAuth2 / Gmail-native auth.
- SMTP send/reply — `SMTP_*` env vars stay unused this slice.
- Archive/snooze/delete triage actions.
- Smart NLP extraction from email body (task title = subject only, for now).
- UIDVALIDITY tracking — a folder UID reset could in theory skip mail; not handled.
- Per-account UI (switcher/filter) — the merged inbox view added in Slice 4 shows every
  account's mail together, tagged by account, with no way to filter to just one yet.
- **Unified inbox with AI-driven triage.** Not started, no design work done. Idea: rank/
  surface the most important mail across all accounts using the Ollama integration already in
  this codebase (`src/sync/ollama.rs`'s warmup, `src/nlp/`'s parsing) instead of adding a new
  LLM dependency — e.g. a local model call that scores each synced message's importance and
  the TUI sorts/highlights on that score. Distinct from the plain archive/snooze triage above:
  this is automatic ranking, not manual action. Also tracked in `docs/roadmap.md` as Slice 6.
- **Per-email summary in the detail view.** Not started, no design work done. Currently the
  detail popup (`App::open_selected_email`, `ui.rs`'s detail popup) shows the raw `body_text`
  as-is; the list's `snippet` is a truncation (`body_preview(200)`), not a summary — neither
  extracts key points. Idea: reuse `src/nlp/ollama_client.rs`'s `OllamaClient` pattern (same
  `qwen2.5:7b` model, already warm via `src/sync/ollama.rs`'s warmup task) with a new
  summarization prompt, store the result in a new column alongside `snippet`/`body_text`.
  Generate at sync time (`App::sync_email_accounts`/`src/sync/mail.rs::sync_mail`), not on
  popup-open — `OLLAMA_TIMEOUT_MS` budgets 15s per call, and doing that synchronously when the
  user opens an email would reintroduce the kind of UI stall the non-blocking sync fix (see
  `DEVELOPMENT.md`) already removed. Tradeoff: adds one LLM call per synced message to the sync
  path — either accept slower sync for a richer list/detail view, or batch/cache summaries the
  way `nlp/parser.rs`'s LRU cache does for parsed input.

## Known limitations

- First sync (no stored UID yet) caps at the most recent 25 messages
  (`INITIAL_SYNC_LIMIT` in `src/email/client.rs`), not the entire mailbox history —
  fetching full RFC822 bodies for an entire real inbox is slow and memory-heavy.
  Older mail is never backfilled; only new mail from that point on is synced.
- `.env` is not auto-loaded (no `dotenvy` wired in) — `TRIPTYCH_EMAIL_ENABLED` and
  `IMAP_*` must be present in the actual process environment (`source .env` before
  running the daemon or CLI), matching this project's existing convention of reading
  env vars directly rather than parsing a file.
