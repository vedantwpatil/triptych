# Setup

Install, configure and first run. Day-to-day use is in [`USAGE.md`](./USAGE.md); code-level context in
[`../CLAUDE.md`](../CLAUDE.md); dev workflow in [`DEVELOPMENT.md`](./DEVELOPMENT.md).

## Install

Needs Rust (2024 edition toolchain), SQLite 3.35+, and optionally [Ollama](https://ollama.ai).

```bash
git clone https://github.com/vedantwpatil/triptych.git && cd triptych
cargo build --release          # binary: target/release/triptych
ollama pull qwen2.5:7b         # optional: LLM fallback for natural-language input
```

Without Ollama, task input still works through the regex parser; only fuzzy phrases lose parsing.
`cargo run` works everywhere below in place of `triptych`.

## First run

Run `triptych` from the directory where you want the data. The database `todo.db` is created there
and migrated automatically. Config is read from a `.env` file in the working directory (plain
`KEY=value` or `export KEY=value` lines) and from the process environment.

## Configuration

Every variable is optional.

| Variable | Purpose | Default |
| --- | --- | --- |
| `DATABASE_URL` | SQLite database | `sqlite:todo.db` |
| `TRIPTYCH_OLLAMA_URL` | Ollama endpoint | local default port |
| `TRIPTYCH_SOCKET_PATH` | Unix socket of the CLI daemon | `$TMPDIR/triptych.sock` |
| `TRIPTYCH_LOG_PATH` | Log file (never stdout) | `$TMPDIR/triptych.log` |
| `CANVAS_ICS_URL` | Canvas calendar feed; enables assignment sync | unset (off) |
| `TRIPTYCH_NOTIFY` | `0`/`false`/`off`/`no` turns deadline alerts off | on |
| `TRIPTYCH_NOTIFY_CMD` | Program run as `cmd <title> <body>` instead of the system notifier | `osascript` (macOS), `notify-send` (Linux) |
| `TRIPTYCH_EMAIL_ENABLED` | `true` starts background mail sync | off |

### Canvas

Canvas, Calendar, "Calendar Feed": copy the `.ics` link. It carries a secret token, so treat it like a
password and keep `.env` out of git.

```
CANVAS_ICS_URL="https://<school>.instructure.com/feeds/calendars/user_XXXX.ics"
```

Check with `triptych canvas sync`. A `403` usually means a stale (regenerated) or disabled feed.

### Email

```
TRIPTYCH_EMAIL_ENABLED=true
IMAP_SERVER=imap.example.com
IMAP_PORT=993
IMAP_USERNAME=me@example.com
IMAP_PASSWORD=app-password       # use an app password where the provider offers them
IMAP_FOLDER=INBOX
IMAP_ARCHIVE_FOLDER=Archive
SMTP_SERVER=smtp.example.com     # sending: compose, reply, forward
SMTP_PORT=587
SMTP_USERNAME=me@example.com
SMTP_PASSWORD=app-password
EMAIL_SIGNATURE="-- me"          # optional
TRIPTYCH_ATTACHMENT_DIR=~/Downloads/triptych   # where `s` saves attachments
```

Several mailboxes: set `IMAP_ACCOUNTS=personal,work`, then suffix each variable with the upper-cased
label (`IMAP_SERVER_PERSONAL`, `SMTP_PASSWORD_WORK`, ...). The unsuffixed variables are then ignored.
Field-level detail: [`roadmap-email.md`](./roadmap-email.md).

### Desktop alerts

On by default while the TUI is open. macOS: the first alert may need permission under System Settings,
Notifications (alerts are posted through `osascript`). Linux: install `libnotify` for `notify-send`.
Point `TRIPTYCH_NOTIFY_CMD` at any script (ntfy, Slack webhook, ...) to route them elsewhere.

### Weekly schedule

Recurring blocks (classes, focus time) come from a TOML file: format in the README, load it with
`triptych schedule import schedule.toml` ([`USAGE.md`](./USAGE.md#calendar)).

## Troubleshooting

- **Ollama not responding:** `ollama serve`, then `ollama list` to confirm `qwen2.5:7b` is there.
- **Stale daemon socket:** `rm $TMPDIR/triptych.sock`, then `triptych daemon &`.
- **No Canvas tasks in the TUI:** run `triptych canvas sync`; TUI sync failures only reach the log file.
- **Something odd:** read `$TMPDIR/triptych.log`.
