# Usage

Everyday use of Triptych: one TUI with three views (todo, calendar, email) plus a CLI. First-time
install and configuration is in [`SETUP.md`](./SETUP.md). Planned features: [`roadmap.md`](./roadmap.md).

Start the TUI with `triptych` (no arguments). `Tab`/`Shift+Tab` cycle views; `q` quits.

## Todo list

Tasks sort open first, then by priority (raised automatically as a deadline nears), then soonest
deadline. Finished tasks sink to the bottom. Badges: `[LOW]`, `[HIGH]`, `[URGENT]` (medium, the default, has none), `↑` when raised by a deadline (`[HIGH↑]`), `[DUE Fri 3:30pm]`.

| Key                                         | Action                                                 |
| ------------------------------------------- | ------------------------------------------------------ |
| `j`/`k`, `5j`, `gg`, `G`, `Ctrl-d`/`Ctrl-u` | Move (vim motions)                                     |
| `/`, `n`/`N`                                | Search, next/previous match                            |
| `a`                                         | Add a task in plain language                           |
| `r`/`R`                                     | Reword the selected task (keeps its date and priority) |
| `Enter`                                     | Toggle done                                            |
| `x`/`d`                                     | Delete (a selection if one is active)                  |
| `v`/`V`                                     | Select rows; `j`/`k` extend, `Esc` cancels             |
| `s`                                         | Auto-schedule onto the calendar                        |
| `o` / `O`                                   | Open the task's first link / list all its links        |
| `c`, `m`                                    | Jump to calendar, email                                |

**Adding tasks.** `Submit report tomorrow at 3pm #work !!`: dates (`today`, `next Monday`, `10/15`),
times (`3pm`, `15:00`), tags (`#work`), priority (`!` medium, `!!` high, `!!!` urgent). Parsing runs
in the background: regex first, local LLM for anything fuzzy.

**Colours.** Every colour is one of the 16 named terminal colours, so your terminal theme decides the
shades. `[HIGH]`/`[URGENT]` and near deadlines are red (urgent is bold and brighter), far deadlines cyan,
`[LOW]` dim. Course codes (`CS-472`) are bold, one colour per course. Category tints the text (deep
work blue, admin yellow, learning cyan, fitness green), tags are magenta, done tasks are dim and struck
through with a green check.

## Editing text

Every text prompt (new task, search, snooze, rules, calendar forms, email compose) has a caret:

| Key | Action |
| --- | --- |
| `Left` / `Right`, `Home` / `End` | Move by character, jump to line start / end (`Ctrl-a` / `Ctrl-e`) |
| `Option`/`Alt` + `Left`/`Right` (or `Alt-b` / `Alt-f`), `Ctrl` + `Left`/`Right` | Move by word |
| `Backspace`, `Delete` | Delete before / at the caret |
| `Option`/`Alt` + `Backspace`, `Ctrl-w` | Delete the word before the caret |

On macOS Terminal and iTerm2, enable "Use Option as Meta key" so Option sends Alt.

## Canvas assignments

With `CANVAS_ICS_URL` set ([`SETUP.md`](./SETUP.md#canvas)), assignments import as tasks on startup and
every 15 minutes, titled `CS-472: Quiz 3` with the due time as the deadline. Re-syncs never duplicate;
a moved due date updates open tasks, and your rewording, priority and completion are kept. Removed
assignments are not deleted, and Canvas tasks cannot be deleted by hand (`x`/`d`, `rm`, `clear`); mark them done instead. `triptych canvas sync` imports on demand.

**Links.** Each Canvas task keeps the assignment page and every link in its description (repos, videos,
PDFs, Canvas files). `⇗2` after a title means two links beyond the first. `o` opens the first link (the
assignment page) in your browser. `O` lists them all: `j`/`k` and `Enter`, or `1`-`9`, open one; `Esc`
closes. A task with a single link opens it directly. Links appear after the next sync, so run
`triptych canvas sync` once after upgrading.

## Deadline alerts

While the TUI is open, a desktop notification fires once when a task comes within 24 hours of its
deadline and once within 1 hour (batched if several). Moving a deadline re-arms it. Completed and
overdue tasks are skipped. Off with `TRIPTYCH_NOTIFY=off`.

## Calendar

Press `c`. A 7-day grid (7am-11pm) shows schedule blocks and scheduled tasks. Load blocks from TOML
with `triptych schedule import file.toml` (`--clear` replaces existing).

| Key                      | Action                                              |
| ------------------------ | --------------------------------------------------- |
| `h`/`j`/`k`/`l`, `H`/`L` | Move cursor; previous/next week                     |
| `n`                      | New schedule block (form: `Tab` between fields)     |
| `s`                      | Pick a task to place at the cursor                  |
| `a`                      | Add a task straight into the cell                   |
| `m`                      | Pick up a scheduled task, then `m` again to drop it |
| `u` / `d`                | Unschedule task / delete block                      |
| `e`                      | Edit the deadline of the task under the cursor      |
| `[`/`]`                  | Cycle tasks stacked in one cell                     |
| `Enter` / `v`            | Cell popup: block (type, title, times) and tasks with full titles; `j`/`k` select, `m`/`u`/`e` act, `Esc` closes |
| `t`/`Esc`                | Back to todo                                        |

Blocks imported from TOML cannot be deleted with `d` (it says so); edit the file and re-import with
`--clear`. Blocks made with `n` stay deletable. Blocks imported before this existed count as manual until
re-imported. Repeats need one entry: `day` takes `weekdays`, `weekends`, `daily` or a `_` list
(`mon_wed_fri`).

`triptych schedule reallocate` re-runs the deep-work allocator (imported tasks are placed then).

## Email

Press `m`. Set up accounts first ([`SETUP.md`](./SETUP.md#email)). Sync runs in the background; `s` forces it.

| Key                         | Action                                                                               |
| --------------------------- | ------------------------------------------------------------------------------------ |
| `v` / `Esc`                 | Open / close a message                                                               |
| `c`, `R`, `A`, `F`          | Compose; reply, reply-all, forward (in a message)                                    |
| `Ctrl-N`/`Ctrl-P`, `Enter`  | In To/Cc: pick a suggested contact (from mail you have received), accept it as `Name <addr>` |
| `Enter`                     | Turn the email into a task (finds a deadline in the text)                            |
| `M`                         | Accept a meeting invite as a task                                                    |
| `r`/`u`, `f`, `t`           | Read/unread, star, cycle color category                                              |
| `a`, `d`                    | Archive, delete                                                                      |
| `z`, `Z`, `x`               | Snooze, view snoozed, unsnooze                                                       |
| `D`                         | Drafts                                                                               |
| `R`                         | Rules: `n` new, e.g. `subject newsletter archive` (`star`/`read`/`archive`/`delete`) |
| `A`,`F`,`I`,`H`,`U`,`S`,`@` | Filters: account, folder, focused/other, attachments, unread, starred, domain        |
| `B`                         | Browse server folders                                                                |
| `o`                         | Toggle priority/date sort                                                            |
| `s` (in message)            | Save attachments                                                                     |

## CLI

```bash
triptych daemon &                 # optional: instant one-shot commands
triptych add "Buy milk tomorrow 4pm #home"
triptych list                     # also: done <id>, rm <id>, clear
triptych canvas sync
triptych email sync [--backfill] | list   # --backfill re-pulls the last 6 months
triptych schedule show | import <file> | export [file] | clear | reallocate
triptych status                   # daemon state; stop it with `triptych stop`
```
