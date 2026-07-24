# Email Client — Slice 1 (IMAP fetch → store → view → convert-to-task)

## Context

README's roadmap lists a full email client (IMAP IDLE, OAuth2, multi-account, triage) as a
planned major feature. `.env` already scaffolds `IMAP_SERVER/PORT/USERNAME/PASSWORD/FOLDER`,
`SMTP_*`, and `TRIPTYCH_EMAIL_ENABLED` — confirming the intended auth path is a single
app-password IMAP account, not OAuth2 (OAuth2 needs an external Google Cloud OAuth client
this session can't provision).

This slice builds the first working vertical: fetch mail over IMAP, store it, view it in the
TUI, and convert an email into a task using the existing NLP/task pipeline — the reason this
lives in Triptych rather than as a standalone client. OAuth2, multi-account, true IMAP IDLE,
SMTP send/reply, and triage actions (archive/snooze) are explicitly deferred to later slices
(see below) so later sessions don't reinvent this scoping conversation.

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
  `main.rs`'s inline key match (inside `run_app`'s event loop) for input.
  **Note:** `src/keys.rs` defines a `handle_key_event` dispatcher but nothing declares
  `mod keys;` in `main.rs` — it's dead code, not compiled in. The live dispatch is the inline
  match in `main.rs`. This is a pre-existing inconsistency, not something this feature fixes.
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
2. **Schema**: `email_messages` table (`uid`, `message_id` unique, `folder`, `from_addr`,
   `from_name`, `subject`, `date_utc`, `snippet`, `is_read`, `task_id` FK → `tasks(id)`).
   No `accounts` table yet — single account via env.
3. **`src/email/` module**: `config.rs` (`EmailConfig::from_env`), `message.rs` (`EmailMessage`
   + `parse_raw`), `client.rs` (`MailSource` trait + `ImapMailSource`), `store.rs` (insert/
   query/mark-read/link-task helpers).
4. **Daemon wiring**: `src/sync/mail.rs::mail_sync_worker` polls (not true IDLE) every 60s,
   spawned from `SyncDaemon::start` when `mail_sync_enabled`.
5. **TUI**: `ViewMode::Email`, list view, `'m'` to toggle from the todo list, `Enter` to convert
   the selected email into a task, `r` to mark read.
6. **CLI**: `triptych email sync` / `triptych email list` for testing without the daemon/TUI.

## Explicitly deferred

- True IMAP IDLE (push) — polling stands in for now.
- OAuth2 / Gmail-native auth.
- Multi-account (needs an `accounts` table).
- SMTP send/reply — `SMTP_*` env vars stay unused this slice.
- Archive/snooze/delete triage actions.
- Smart NLP extraction from email body (task title = subject only, for now).
- UIDVALIDITY tracking — a folder UID reset could in theory skip mail; not handled.

## Known limitations

- First sync (no stored UID yet) caps at the most recent 25 messages
  (`INITIAL_SYNC_LIMIT` in `src/email/client.rs`), not the entire mailbox history —
  fetching full RFC822 bodies for an entire real inbox is slow and memory-heavy.
  Older mail is never backfilled; only new mail from that point on is synced.
- `.env` is not auto-loaded (no `dotenvy` wired in) — `TRIPTYCH_EMAIL_ENABLED` and
  `IMAP_*` must be present in the actual process environment (`source .env` before
  running the daemon or CLI), matching this project's existing convention of reading
  env vars directly rather than parsing a file.
