# TUI Driver and Scenario Suite

Two Python tools in [`../tests/tui/`](../tests/tui/CLAUDE.md) that run the real `triptych` binary in a
pty so an agent or human can drive and inspect the TUI without a terminal. See
[`DEVELOPMENT.md`](./DEVELOPMENT.md), [`../CLAUDE.md`](../CLAUDE.md); the `triptych-tui` skill points here.

## Safety

Every session/scenario gets a throwaway sandbox dir with its own `todo.db`, socket and log
(`DATABASE_URL`, `TRIPTYCH_SOCKET_PATH`, `TRIPTYCH_LOG_PATH`). The driver strips `IMAP_*`/`SMTP_*`,
sets `TRIPTYCH_EMAIL_ENABLED=false`, strips `TRIPTYCH_OLLAMA_URL` and refuses `--env` overrides of the
three path vars. Email sync and summaries only reach the sandbox's own fakes ([`TUI_FAKES.md`](./TUI_FAKES.md)). Never run the binary by hand against the real `todo.db` / `$TMPDIR/triptych.sock`.

## tuidrive.py: interactive driver

`start` spawns a detached server that owns the pty and a `pyte` terminal emulator (alt-screen
aware), so separate shell calls drive one live TUI. First run bootstraps `pyte` into
`target/tuidrive-venv`. Sessions live under `$TUIDRIVE_HOME` (default `/tmp/tuidrive-<uid>`);
`-s NAME` picks a session (default `default`). Keep session names short: unix-socket paths max 104
bytes on macOS.

```
python3 tests/tui/tuidrive.py start [--size 130x42] [--seed file.sql] [--pre 'add "x !!"'] [--build]
python3 tests/tui/tuidrive.py send c a t:"buy milk #home !!" ENTER     # keys, then prints the screen
python3 tests/tui/tuidrive.py screen [--compact]                       # current screen, no input
python3 tests/tui/tuidrive.py wait "Weekly Calendar" [--gone --regex --timeout 10]
python3 tests/tui/tuidrive.py db "SELECT id, description FROM tasks"   # read or write session DB
python3 tests/tui/tuidrive.py cli [--bg] list                          # CLI in the same sandbox (flags BEFORE args)
python3 tests/tui/tuidrive.py resize 20 60 | restart | ls | path | stop [--keep] [--all]
```

Key tokens for `send`: `ENTER ESC TAB BTAB BS SPACE UP DOWN LEFT RIGHT HOME END PGUP PGDN DEL`,
`C-x` (control), `t:literal text`, or one character (`j`, `a`, `[`). Escape sequences are written
atomically with ~50ms pacing (crossterm reads a split `ESC [ A` as two keys). `--compact` strips box
characters; `--marks` also lists highlighted spans (the calendar cursor cell is bg `7f7f7f`).
`send` waits for the screen to settle (`--settle`), then prints it; a dead process shows `EXITED code=N`.

## tui_suite.py: scenario suite

136 scenarios, each in its own child process and sandbox, run in parallel. About 1 minute at `-j 4`.

```
python3 tests/tui/tui_suite.py [--list] [--only 'cal_*,todo_add_edit_cancel'] [-j 4] [--build] [-v] [--strict]
```

| Group     | Count | Covers                                                                        |
| --------- | ----- | ----------------------------------------------------------------------------- |
| `cli_*`   | 10    | add/list/done/rm/clear, priorities, badges, escalation, bad usage             |
| `daemon_*`| 6     | start/status/stop, add through the socket, second instance                    |
| `nlp_*`   | 17    | dates, weekdays, times, ranges, durations, tags, priorities                   |
| `sched_*` | 9     | schedule import/export/show/clear/reallocate, overlaps, bad input             |
| `todo_*`  | 20    | add/cancel/toggle/delete, `v` visual delete, linked-email delete, badges, persist, `s`, view cycle |
| `cal_*`   | 24    | grid, cursor, week nav, block form, task picker, move, deadlines, stacking    |
| `email_*` | 20    | list, popup, mark read, convert, `s` sync, priority order, AI summaries       |
| `imap_*`  | 14    | real `email sync` and TUI sync against the fake IMAP server           |
| `vim_*`   | 16    | counts, `gg`/`G`, `C-d`/`C-u`, `0`/`$`, `/` search, visual counts, Ctrl chords |

Statuses: `PASS`, `FAIL` (regression, exit 1), `XFAIL` (known bug still failing), `XPASS` (fixed; `--strict`
fails on it). None open today. For a new bug, write a scenario asserting the correct behaviour, tag it
`known="KI-n"`, file it under [`DEVELOPMENT.md`](./DEVELOPMENT.md#known-issues); on fix drop the marker.

## Fake servers

Scenarios never touch a real mail account or model: `Sandbox` starts local fakes on demand, `c.imap()` and
`c.sb.ollama()`, and only then puts their address in the binary's environment. Details, CLI flags and
scenario helpers: [`TUI_FAKES.md`](./TUI_FAKES.md).

## Adding a scenario

In `tests/tui/tui_suite.py`: `@scenario("name", known=None)` (name up to 28 chars) on `def _(c: Ctx)`.
`Ctx` gives `c.cli(...)`, `c.db(sql)`, `c.tasks()`, `c.add(text)`, `c.seed_blocks(...)`,
`c.seed_emails(n)`, `c.tui()` / `c.cal()` (returns a `Term`), `c.see(text, gone=, timeout=)`,
`c.check(cond, msg)` and `c.eq(got, want, msg)`. `Term` has `press`, `type`, `text`, `has`,
`wait_for`, `cursor`, `resize`, `style_of(text)` (fg hex and bold of a word; palette slots 0-15 show as
the xterm hex, e.g. `ff0000` = slot 9). Assert on DB rows and screen text, not on timing.

## Known limits

- Without `c.sb.ollama()`, NLP scenarios use the local Ollama (`localhost:11434`); most inputs return
  from the regex fast path first. The fake replies with canned text, so it proves plumbing, not quality.
- IMAP is a fake: it proves protocol use, TLS trust and cursor logic, not real provider quirks (OAuth, IDLE).
- `pyte` renders text and colour, not pixels; wide/emoji glyphs may misalign columns.
