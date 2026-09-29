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
| `r`                                         | Reword the selected task (keeps its date and priority) |
| `Enter`                                     | Toggle done                                            |
| `x`/`d`                                     | Delete (a selection if one is active)                  |
| `v`/`V`                                     | Select rows; `j`/`k` extend, `Esc` cancels             |
| `s`                                         | Auto-schedule onto the calendar                        |
| `c`, `m`                                    | Jump to calendar, email                                |

**Adding tasks.** `Submit report tomorrow at 3pm #work !!`: dates (`today`, `next Monday`, `10/15`),
times (`3pm`, `15:00`), tags (`#work`), priority (`!` medium, `!!` high, `!!!` urgent). Parsing runs
in the background: regex first, local LLM for anything fuzzy.

**Colours.** Every colour is one of the 16 named terminal colours, so your terminal theme decides the
shades. `[HIGH]`/`[URGENT]` and near deadlines are red (urgent is bold and brighter), far deadlines cyan,
`[LOW]` dim. Course codes (`CS-472`) are bold, one colour per course. Category tints the text (deep
work blue, admin yellow, learning cyan, fitness green), tags are magenta, done tasks are dim and struck
through with a green check.

## Canvas assignments

With `CANVAS_ICS_URL` set ([`SETUP.md`](./SETUP.md#canvas)), assignments import as tasks on startup and
every 15 minutes, titled `CS-472: Quiz 3` with the due time as the deadline. Re-syncs never duplicate;
a moved due date updates open tasks, and your rewording, priority and completion are kept. Removed
assignments are not deleted. `triptych canvas sync` imports on demand.

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
| `t`/`Esc`                | Back to todo                                        |

`triptych schedule reallocate` re-runs the deep-work allocator (imported tasks are placed then).

## Email

Press `m`. Set up accounts first ([`SETUP.md`](./SETUP.md#email)). Sync runs in the background; `s` forces it.

| Key                         | Action                                                                               |
| --------------------------- | ------------------------------------------------------------------------------------ |
| `v` / `Esc`                 | Open / close a message                                                               |
| `c`, `R`, `A`, `F`          | Compose; reply, reply-all, forward (in a message)                                    |
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
triptych email sync | list
triptych schedule show | import <file> | export [file] | clear | reallocate
triptych status                   # daemon state; stop it with `triptych stop`
```
