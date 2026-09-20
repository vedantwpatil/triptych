# TUI Driver and Scenario Suite

Two Python tools in [`../tests/tui/`](../tests/tui/CLAUDE.md) that run the real `triptych` binary in a
pseudo-terminal so an agent (or a human) can drive and inspect the TUI without a real terminal.
Part of the dev workflow in [`DEVELOPMENT.md`](./DEVELOPMENT.md); module map in
[`../CLAUDE.md`](../CLAUDE.md). The Claude Code skill `triptych-tui` points here.

## Safety

Every session/scenario gets a throwaway sandbox dir with its own `todo.db`, socket and log
(`DATABASE_URL`, `TRIPTYCH_SOCKET_PATH`, `TRIPTYCH_LOG_PATH`). The driver strips `IMAP_*`/`SMTP_*`,
sets `TRIPTYCH_EMAIL_ENABLED=false` and refuses `--env` overrides of the three path vars. Never run
the binary by hand against the real `todo.db` / `$TMPDIR/triptych.sock`.

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
python3 tests/tui/tuidrive.py cli list                                 # CLI in the same sandbox
python3 tests/tui/tuidrive.py cli --bg daemon                          # flags go BEFORE args
python3 tests/tui/tuidrive.py resize 20 60 | restart | ls | path | stop [--keep] [--all]
```

Key tokens for `send`: `ENTER ESC TAB BTAB BS SPACE UP DOWN LEFT RIGHT HOME END PGUP PGDN DEL`,
`C-x` (control), `t:literal text`, or one character (`j`, `a`, `[`). Escape sequences are written
atomically with ~50ms pacing (crossterm reads a split `ESC [ A` as two keys). `--compact` strips box
characters; `--marks` also lists highlighted spans (the calendar cursor cell is bg `7f7f7f`).
`send` waits for the screen to settle (`--settle`), then prints it. Exit codes: the process exit
shows as `EXITED code=N`.

## tui_suite.py: scenario suite

95 scenarios, each in its own child process and sandbox, run in parallel. About 1 minute at `-j 4`.

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
| `cal_*`   | 23    | grid, cursor, week nav, block form, task picker, move, deadlines, stacking    |
| `email_*` | 10    | list, detail popup, mark read, convert to task, `email list` (seeded rows)    |

Result statuses: `PASS`, `FAIL` (regression, exit 1), `XFAIL` (known bug still failing),
`XPASS` (known bug now passes: remove its `known=` marker; `--strict` makes it fail). No bug is open
today, so every scenario is a plain PASS. When a new bug is found, write a scenario that asserts the
correct behaviour and tag it `known="KI-n"` (file KI-n under Known Issues in
[`DEVELOPMENT.md`](./DEVELOPMENT.md#known-issues)): it is then the executable repro. Fixing the bug
flips it to XPASS; delete the marker and move the issue to Resolved.

## Adding a scenario

In `tests/tui/tui_suite.py`: `@scenario("name", known=None)` (name up to 28 chars) on `def _(c: Ctx)`.
`Ctx` gives `c.cli(...)`, `c.db(sql)`, `c.tasks()`, `c.add(text)`, `c.seed_blocks(...)`,
`c.seed_emails(n)`, `c.tui()` / `c.cal()` (returns a `Term`), `c.see(text, gone=, timeout=)`,
`c.check(cond, msg)` and `c.eq(got, want, msg)`. `Term` has `press`, `type`, `text`, `has`,
`wait_for`, `cursor`, `resize`, `style_of(text)` (fg hex and bold of a word; palette slots 0-15 show as
the xterm hex, e.g. `ff0000` = slot 9). Assert on DB rows and screen text, not on timing.

## Known limits

- Ollama is not redirectable (`http://localhost:11434` is hardcoded), so NLP scenarios depend on the
  local Ollama state; most inputs return from the regex fast path first. Deadline edits in the
  calendar parse in the background (KI-13), so the UI stays responsive during the 15s Ollama call.
- Real IMAP sync is not exercised (email scenarios seed `email_messages` directly).
- `pyte` renders text and colour, not pixels; wide/emoji glyphs may misalign columns.
