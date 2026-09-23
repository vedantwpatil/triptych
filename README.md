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

## Installation

**Prerequisites**

- Rust 1.70+
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

| Key     | Action                        |
| ------- | ----------------------------- |
| `j/k`   | Navigate tasks (`5j`, `gg`, `G`, `Ctrl-d/u`) |
| `/`     | Search (`n`/`N` next/previous) |
| `a`     | Add new task                  |
| `Enter` | Toggle completion             |
| `x`/`d` | Delete task (or selection)    |
| `v`/`V` | Select rows (`j/k` extend, `Esc` cancels) |
| `s`     | Auto-schedule task            |
| `c`     | Switch to calendar view       |
| `m`     | Switch to email view (`s` sync, `o` sort, `v` open) |
| `H/L`   | Previous/next week (calendar) |
| `q`     | Quit                          |

### CLI Mode

```bash
# Start daemon for fast commands
triptych daemon &

# Task operations
triptych add "Buy groceries tomorrow at 4pm #personal"
triptych list
triptych done 42
triptych rm 42
triptych clear

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

- Task management with NLP parsing
- Weekly calendar view with schedule blocks
- Persistent daemon for CLI performance
- TOML-based schedule import/export
- Auto-scheduling to available time slots

### Email Client — In Progress

IMAP fetch, storage, TUI view, email-to-task conversion, and multi-account are already
working (see `docs/roadmap-email.md` for details). Still planned:

- IMAP IDLE for real-time notifications
- Keyboard-driven email triage (archive, reply, snooze)
- OAuth2 support for Gmail and other providers
- SMTP send/reply
- Enhance the nlp parsing and the automatic scheduling
- Unified inbox with AI-driven triage (using the existing Ollama integration) — not started

The goal is to bring Superhuman-like email productivity to the terminal, fully integrated with task and calendar workflows.

### Other Planned Features

- CalDAV calendar sync
- Recurring tasks
- Full-text search
- Desktop notifications
- Task dependencies
- Statistics dashboard

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
