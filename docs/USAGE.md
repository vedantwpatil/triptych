# Usage

Everyday use of Triptych: one TUI with three views (todo, calendar, email) plus a CLI. First-time
install and configuration is in [`SETUP.md`](./SETUP.md). Planned features: [`roadmap.md`](./roadmap.md).

Start the TUI with `triptych` (no arguments). `Tab`/`Shift+Tab` cycle views; `q` quits. `?` lists
the keys of the current view; any key closes it.

## Todo list

Tasks sort open first, then by priority (raised automatically as a deadline nears), then soonest
deadline. Finished tasks sink to the bottom. Badges: `[LOW]`, `[HIGH]`, `[URGENT]` (medium, the default, has none), `↑` when raised by a deadline (`[HIGH↑]`), `[DUE Fri 3:30pm]`.

| Key                                         | Action                                                 |
| ------------------------------------------- | ------------------------------------------------------ |
| `j`/`k`, `5j`, `gg`, `G`, `Ctrl-d`/`Ctrl-u` | Move (vim motions)                                     |
| `/`, `n`/`N`                                | Search, next/previous match                            |
| `a`                                         | Add a task in plain language                           |
| `r`/`R`                                     | Reword the selected task (keeps its date and priority) |
| `Enter`                                     | Toggle done (a repeating task adds its next copy)      |
| `x`/`d`                                     | Delete (a selection if one is active); says what went  |
| `u`                                         | Undo the last delete (one step, same ids)              |
| `v`/`V`                                     | Select rows; `j`/`k` extend, `Esc` cancels             |
| `s`                                         | Auto-schedule onto the calendar                        |
| `o` / `O`                                   | Open the task's first link / list all its links        |
| `c`, `m`                                    | Jump to calendar, email                                |

**Adding tasks.** `Submit report tomorrow at 3pm #work !!`: dates (`today`, `next Monday`, `10/15`),
times (`3pm`, `15:00`), tags (`#work`), priority (`!` medium, `!!` high, `!!!` urgent). Parsing runs
in the background: regex first, local LLM for anything fuzzy.

**Repeating tasks.** Add `every day`, `every weekday`, `every week`, `every month` or `every friday`
(`every fri` works too) anywhere in the text: `water plants every friday 6pm`. A trailing `daily`,
`weekly`, `monthly` or `weekdays` also counts, but only at the end or before `#tags`/`!` marks
(`pay rent monthly #bills`), so "review weekly report" stays a plain task. The phrase is removed from
the title and the row shows `↻ fri`. Marking it done (`Enter` or `triptych done`) adds the next copy
at the bottom of the list: same text, priority, tags, duration and links, dates moved by the rule and
never in the past. A scheduled time and a deadline keep the same gap. A task with no date gets a
deadline at the end of the next day. Done, reopen, done does not add a second copy. Not supported yet:
`every 2 weeks`, several weekdays, ending a series (delete the open copy). Monthly clamps short
months (31 Jan becomes 28 Feb, then stays on the 28th). Reword (`r`) keeps the rule.

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

With the TUI closed, `triptych notify install` (macOS) keeps them coming through a LaunchAgent, see
[`SETUP.md`](./SETUP.md#desktop-alerts). `triptych notify check` sends whatever is due right now and exits.
Canvas deadlines only refresh while the TUI runs (or after `triptych canvas sync`).

## New-mail alerts

While the TUI is open, each background mail sync that stores unread mail posts one grouped notification
(sender and subject, three listed, then `+N more`). Bulk mail (newsletters, receipts, ...) and mail more
than 6 hours old are skipped, so a first sync never floods. Same switch and notifier as deadline alerts.

## Job digests

Daily digest mail from `noreply@swelist.com` is scored against a resume-derived profile (role words that
raise or lower a fit, and excludes such as PhD or "Sophomore Only"). Each good fit becomes a todo task
`Apply: Company - Role` with the apply link when the mail has one (`o` opens it), and one notification
lists the new ones. At most 10 tasks per digest; digests older than 14 days are ignored. A posting is
scored once, and a task you delete stays deleted. A title the rules cannot call (no matching word, or
good and bad words cancelling) is rated 0-10 by your local Ollama model, up to 30 per scan; with Ollama
down it keeps its rule score.

`triptych jobs` runs the same scan on mail already synced and lists the postings, best first
(`--all`/`-a` adds the skipped ones with the words that decided them). Tune the profile with a TOML file
(`TRIPTYCH_JOBS_PROFILE`, see [`SETUP.md`](./SETUP.md)).

`A` on an `Apply:` task fills the application in a browser and shows what it filled and which required
fields are still empty; `y` submits, `r` rechecks after you answer them, `n`/`Esc` cancels. Needs
`TRIPTYCH_APPLY_CMD`, and `y` needs `TRIPTYCH_APPLY_SUBMIT=1` ([`APPLY.md`](./APPLY.md)). Without the
command, `A` opens the link like `o`.

## Calendar

Press `c`. A 7-day grid (7am-11pm) shows schedule blocks and scheduled tasks; the cursor starts on
today at the current hour (7am before, 10pm after the grid). Load blocks from TOML
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
| `T`                      | Focus the to-do panel (opens it on first press)     |
| `Enter` / `v`            | Cell popup: block (type, title, times) and tasks with full titles; `j`/`k` select, `m`/`u`/`e` act, `Esc` closes |
| `t`/`Esc`                | Back to todo                                        |

On a small terminal the grid shows only the hours and days that fit around the cursor and scrolls as
you move (about 10 columns per day, 2 rows per hour).

The to-do panel (right 30%) lists open tasks beside the grid. `T` focuses it and the grid cursor stays
visible: that cell is where `Enter` schedules the highlighted task (scheduling an already placed task
moves it). `j`/`k` move, `x` completes (a recurring task adds its next copy), `Esc` returns to the grid
with the panel still open, `T` again hides it.

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
| `Ctrl-N`/`Ctrl-P`, `Enter`  | In To/Cc: pick a suggested contact (from mail you have received), accept it as `Name <addr>, ` (ready for the next recipient) |
| `Enter`                     | Turn the email into a task (finds a deadline in the text)                            |
| `M`                         | Accept a meeting invite as a task                                                    |
| `r`/`u`, `f`, `t`           | Read/unread, star, cycle color category                                              |
| `a`, `d`                    | Archive; delete from the server for good (press `d` twice, any other key cancels)    |
| `z`, `Z`, `x`               | Snooze, view snoozed, unsnooze (`x` says `Not snoozed` when there is nothing to clear) |
| `D`                         | Drafts                                                                               |
| `R`                         | Rules: `n` new, e.g. `subject newsletter archive` (`star`/`read`/`archive`/`delete`); a new rule also runs over mail already synced, except `delete`, which only acts on mail that arrives after it |
| `A`,`F`,`I`,`H`,`U`,`S`,`@` | Filters: account, folder, focused/other, attachments, unread, starred, domain        |
| `B`                         | Browse server folders                                                                |
| `o`                         | Toggle priority/date sort                                                            |
| `s` (in message)            | Save attachments (a repeated name gets an `<n>-` prefix, nothing is overwritten)     |

Each list row reads left to right:
`(account) [MM/DD HH:MM] sender [folder] [other] ▲ ● [color] [N] subject [attach] [invite] [task] [snoozed until ..]`.
Tags with no state are hidden.

| Mark                 | Meaning                                                                                  |
| -------------------- | ---------------------------------------------------------------------------------------- |
| bold subject         | Unread                                                                                   |
| `▲` / `△`            | Priority: high / medium, from subject and snippet keywords. Display only; `o` sorts by it |
| `●` (yellow)         | Starred (`f`)                                                                            |
| `[folder]`           | Message lives outside `INBOX`, e.g. `[Archive]`                                          |
| `[other]`            | AI triage put it in Other (Focused mail is untagged)                                     |
| `[red]`, `[blue]`... | Color category (`t`)                                                                     |
| `[N]`                | Thread of N messages                                                                     |
| `[attach]`           | Has attachments (`s` in the message saves them)                                          |
| `[invite]`           | Meeting invite not yet accepted (`M`); becomes `[task]` once accepted                    |
| `[task]`             | Already turned into a task                                                               |
| `[snoozed until ..]` | Hidden from the main list until then (`Z` shows snoozed mail)                            |

The title bar adds `· account:`, `· folder:`, `· focus:`, `· attach:`, `· unread:`, `· starred:`, `· domain:` or
`· snoozed view` while that filter is on.

## CLI

```bash
triptych daemon &                 # optional: instant one-shot commands
triptych add "Buy milk tomorrow 4pm #home"
triptych list [--all]             # open tasks; --all/-a adds completed. Also: done <id>, rm <id>, clear
triptych canvas sync
triptych notify check | install | uninstall   # deadline alerts without the TUI
triptych email sync [--backfill] | list   # --backfill re-pulls the last 6 months
triptych schedule show | import <file> | export [file] | clear | reallocate
triptych status                   # daemon state; stop it with `triptych stop`
```
