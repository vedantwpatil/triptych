# Triptych

A fast, keyboard-driven productivity TUI built in Rust. Manage tasks and schedules from your terminal with natural language input and sub-100ms response times.

## Current Features

**Task Management**

- Add tasks with natural language: `"Submit report tomorrow at 3pm #work !!"`
- Priority levels, tags, and smart categorization (deepwork, admin, learning, etc.)
- Vim-style navigation: counts (`5j`), `gg`/`G`, `Ctrl-d`/`Ctrl-u`, `/` search with `n`/`N`

**Weekly Calendar**

- Visual 7-day schedule grid (7am-11pm)
- Define recurring time blocks via TOML configuration
- Auto-schedule tasks to matching available slots
- Week navigation with conflict detection

**Performance**

- Persistent daemon architecture for instant CLI commands (<100ms)
- 3-layer NLP parsing: cache → regex → local LLM (Ollama)
- Zero input lag in TUI (<1ms response time)

Full guides: [`docs/SETUP.md`](./docs/SETUP.md) (install and configuration) and [`docs/USAGE.md`](./docs/USAGE.md) (every feature and key).

## Installation

**Prerequisites**

- Rust (2024 edition)
- Ollama with Qwen2.5-7B model
- SQLite 3.35+

```bash
git clone https://github.com/vedantwpatil/triptych.git
cd triptych
cargo build --release

# Install Ollama model
ollama pull qwen2.5:7b
```

## Usage

### TUI Mode

```bash
cargo run
```

**Keybindings**

| Key     | Action                                              |
| ------- | --------------------------------------------------- |
| `j/k`   | Navigate tasks (`5j`, `gg`, `G`, `Ctrl-d/u`)        |
| `/`     | Search (`n`/`N` next/previous)                      |
| `a`     | Add new task                                        |
| `r`/`R` | Reword selected task                                |
| `Enter` | Toggle completion                                   |
| `x`/`d` | Delete task (or selection)                          |
| `v`/`V` | Select rows (`j/k` extend, `Esc` cancels)           |
| `s`     | Auto-schedule task                                  |
| `c`     | Switch to calendar view                             |
| `m`     | Switch to email view (`s` sync, `o` sort, `v` open) |
| `H/L`   | Previous/next week (calendar)                       |
| `q`     | Quit                                                |

### CLI Mode

```bash
# Start daemon for fast commands
triptych daemon &

# Task operations
triptych add "Buy groceries tomorrow at 4pm #personal"
triptych list          # open tasks; `triptych list --all` includes completed
triptych done 42
triptych rm 42
triptych clear

# Canvas assignments (set CANVAS_ICS_URL to your Canvas calendar feed; also polled every 15 min in the TUI)
triptych canvas sync

# Schedule management
triptych schedule show
triptych schedule import schedule.toml
triptych schedule export backup.toml

# Stop daemon
triptych stop
```

### Natural Language Parsing

```bash
triptych add "Team meeting next Monday #important"
triptych add "Fix critical bug today !!! #dev"
triptych add "Call John in the evening #personal"
```

Supported syntax:

- **Time**: 12/24-hour format, AM/PM
- **Dates**: today, tomorrow, next Monday, specific dates
- **Tags**: #work, #personal, #dev
- **Priority**: ! (medium), !! (high), !!! (urgent)

## Configuration

### Weekly Schedule Template

Create a `schedule.toml` to define recurring time blocks:

```toml
[[blocks]]
day = "monday_wednesday_friday"
start = "09:00"
end = "12:00"
type = "deepwork"
title = "Focus Time"

[[blocks]]
day = "tuesday_thursday"
start = "14:00"
end = "15:00"
type = "admin"
title = "Emails & Planning"
```

Import with `triptych schedule import schedule.toml`.

## Tech Stack

- **TUI**: Ratatui + Crossterm
- **Database**: SQLite with SQLx
- **NLP**: Ollama (local LLM) + regex patterns + LRU cache
- **Runtime**: Tokio async
- **IPC**: Unix sockets for daemon communication

## Roadmap

### Implemented

- Task management with NLP parsing, urgency badges, vim motions and `/` search
- Weekly calendar view with schedule blocks; deadline tasks auto-allocated into blocks
- Persistent daemon for CLI performance
- TOML-based schedule import/export
- Canvas assignment sync (`CANVAS_ICS_URL`)
- Deadline alerts, also with the TUI closed (`triptych notify install`, macOS)
- New-mail alerts while the TUI is open (unread, non-bulk mail)
- Job-digest triage: SWE List postings scored against your profile become `Apply:` tasks (`triptych jobs`); `A` fills the application in a browser and submits only after you approve (`docs/APPLY.md`)
- Recurring tasks: `water plants every friday`, finishing one adds the next (`docs/USAGE.md`)
- To-do panel inside the calendar (`T`): schedule or complete tasks without leaving the grid
- Email: multi-account IMAP with IDLE, SMTP reply/forward, archive/delete/snooze, Focused/Other
  triage, rules, categories, drafts, email-to-task with body deadline scan
  (see `docs/roadmap-email.md`)

### Next

1. Email work that feeds tasks and the calendar
2. CalDAV calendar sync (a stub exists; not functional)

Later: OAuth2 for Gmail, task dependencies, statistics dashboard. Status detail: `docs/roadmap.md`.

## Troubleshooting

**Ollama not responding**

```bash
ollama serve        # Start Ollama service
ollama list         # Verify model is installed
```

**Daemon issues**

```bash
rm /tmp/triptych.sock   # Remove stale socket
triptych daemon         # Restart
```

**Slow parsing**: Use a smaller model (`ollama pull qwen2.5:1.5b`) and update `src/nlp/ollama_client.rs`.

## Acknowledgments

Built with [Ratatui](https://ratatui.rs/) and [Ollama](https://ollama.ai/). Inspired by Superhuman, TickTick, and Notion Calendar.

---

**Status**: Active development (alpha)
