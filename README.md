# Triptych

<p align="center">
  <strong>A high-performance, terminal-based productivity suite combining tasks, calendar, and email in a unified interface</strong>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/rust-%23000000.svg?style=for-the-badge&logo=rust&logoColor=white" alt="Rust">
  <img src="https://img.shields.io/badge/sqlite-%2307405e.svg?style=for-the-badge&logo=sqlite&logoColor=white" alt="SQLite">
  <img src="https://img.shields.io/badge/tokio-async-blue?style=for-the-badge" alt="Tokio">
</p>

## Overview

Triptych is a privacy-first, local-first productivity application built for terminal power users. It combines natural language task parsing, intelligent caching, and keyboard-driven workflows to achieve sub-100ms response times comparable to Superhuman, TickTick, and Notion Calendar.

### Key Features

- **🚀 Sub-100ms Performance**: Tiered caching architecture achieves <1ms response times for 70% of operations
- **🧠 Natural Language Parsing**: Intelligent NLP using Ollama (Qwen2.5-7B) with regex fast-path fallback
- **⌨️ Keyboard-First**: Vim-style navigation with command palette (planned)
- **🔒 Privacy-First**: 100% local data storage with SQLite, no cloud dependencies
- **⚡ Background Sync**: Async daemon pre-warms models and syncs data in the background
- **🎯 Smart Task Management**: Automatic priority detection, tag extraction, and date parsing

## Performance Metrics

| Operation | Latency | Strategy |
|-----------|---------|----------|
| Exact cache hit | <1ms | LRU cache (1000 entries) |
| Fuzzy cache match | 2-5ms | Jaro-Winkler similarity (85% threshold) |
| Regex parsing | 20-30ms | Pattern matching for structured input |
| Ollama (warm) | 200-500ms | Complex natural language queries |
| **Weighted Average** | **~50ms** | Across typical workload distribution |

## Tech Stack

### Core Runtime
- **Rust** with **Tokio** async runtime for concurrent background operations
- **Ratatui + Crossterm** for immediate-mode TUI rendering
- **SQLite with SQLx** for local-first data storage and compile-time query checking

### NLP Pipeline
- **Ollama** (Qwen2.5-7B) for complex natural language understanding
- **Custom regex patterns** for fast-path structured input parsing
- **LRU cache** with fuzzy matching (strsim) for instant responses
- **4-layer tiered fallback**: Exact cache → Fuzzy cache → Regex → Ollama → Fallback

### Architecture Patterns
- **Structured concurrency** with graceful shutdown using broadcast channels
- **Interior mutability** with `tokio::sync::Mutex` for shared state
- **Local-first sync** with eventual consistency patterns
- **Modern Rust 2018+** module structure

## Installation

### Prerequisites

1. **Rust toolchain** (1.70+)
```

curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

```

2. **Ollama** (for NLP parsing)
```


# macOS/Linux

curl -fsSL https://ollama.com/install.sh | sh

# Start Ollama service

ollama serve

# Pull the Qwen2.5 model

ollama pull qwen2.5:7b

```

3. **SQLite** (usually pre-installed on macOS/Linux)

### Build & Run

```


# Clone the repository

git clone https://github.com/yourusername/triptych.git
cd triptych

# Set up the database

export DATABASE_URL="sqlite:todo.db"

# Run database migrations

cargo sqlx migrate run

# Build and run in TUI mode

cargo run

# Or build for release

cargo build --release
./target/release/triptych

```

## Usage

### TUI Mode (Recommended)

```

cargo run

```

**Keybindings:**
- `j/k` - Navigate up/down
- `a` - Add new task (enter natural language)
- `x` - Delete selected task
- `Enter` - Toggle task completion
- `Esc` - Return to normal mode
- `q` - Quit

### CLI Mode

```


# Add tasks with natural language

cargo run add "Submit report tomorrow at 3pm \#work !!"
cargo run add "Buy groceries after work \#personal"
cargo run add "Review PR before standup \#dev !urgent"

# List all tasks

cargo run list

# Complete a task

cargo run done 1

# Remove a task

cargo run rm 2

# Clear completed tasks

cargo run clear

```

### Natural Language Examples

Triptych understands natural language input:

```

"Submit report tomorrow at 3pm \#work !!"
→ Task: "Submit report"
→ Due: Tomorrow 3:00 PM
→ Priority: Urgent
→ Tags: [work]

"Remind me after standup to review the PR"
→ Task: "Review the PR"
→ Due: After next event (standup)
→ Priority: Medium

"Buy milk and eggs \#groceries"
→ Task: "Buy milk and eggs"
→ Tags: [groceries]
→ Priority: Medium

```

## Architecture

### Project Structure

```

triptych/
├── src/
│   ├── main.rs              \# CLI entry point \& TUI event loop
│   ├── app.rs               \# App state \& database operations
│   ├── ui.rs                \# Ratatui rendering logic
│   ├── cli.rs               \# Clap command definitions
│   ├── sync.rs              \# Background sync daemon
│   ├── nlp.rs               \# NLP module declaration
│   └── nlp/
│       ├── parser.rs        \# Tiered parsing orchestration
│       ├── ollama_client.rs \# HTTP client for Ollama API
│       ├── regex_patterns.rs\# Fast-path pattern matching
│       └── types.rs         \# ParsedItem, Task, Event types
├── migrations/
│   └── 20250930194903_initial_schema.sql
├── Cargo.toml
└── README.md

```

### Database Schema

```

-- Tasks with NLP metadata
CREATE TABLE tasks (
id INTEGER PRIMARY KEY,
description TEXT NOT NULL,
completed BOOLEAN DEFAULT 0,
priority INTEGER DEFAULT 1,
tags TEXT,                          -- JSON array
scheduled_at TIMESTAMP,
natural_language_input TEXT,        -- Original input
item_order INTEGER,
created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
);

-- Unified timeline for tasks/events/emails
CREATE TABLE timeline_entries (
id INTEGER PRIMARY KEY,
entity_type TEXT NOT NULL,          -- 'task', 'event', 'email'
entity_id INTEGER NOT NULL,
scheduled_at TIMESTAMP,
priority INTEGER
);

```

### Background Sync Daemon

The sync daemon runs concurrently with the main application:

1. **Pre-warms Ollama** on startup (eliminates 2-3s cold start)
2. **Preloads cache** from top 100 most-used patterns in SQLite
3. **Email sync** (IMAP IDLE ready, pending configuration)
4. **Calendar sync** (CalDAV ready, pending configuration)

Uses `tokio::select!` for graceful shutdown with 5-second timeout.

## Configuration

### NLP Settings

Modify `src/nlp/parser.rs`:

```

// Cache capacity (default: 1000)
LruCache::new(NonZeroUsize::new(1000).unwrap())

// Fuzzy match threshold (default: 0.85)
let similarity_threshold = 0.85;

```

### Ollama Model

Switch to faster inference:

```


# Use 1.5B model (3-5x faster, minimal accuracy loss)

ollama pull qwen2.5:1.5b

```

Update `src/nlp/ollama_client.rs`:
```

const MODEL: \&str = "qwen2.5:1.5b";

```

### Sync Daemon

Modify `src/sync.rs` `SyncConfig`:

```

SyncConfig {
ollama_warmup_enabled: true,
cache_preload_enabled: true,
email_sync_enabled: false,      // Enable when IMAP configured
calendar_sync_enabled: false,   // Enable when CalDAV configured
email_check_interval_secs: 300,
}

```

## Development

### Running Tests

```

cargo test

```

### Database Migrations

```


# Create new migration

sqlx migrate add migration_name

# Apply migrations

sqlx migrate run

# Generate offline query metadata (for CI)

cargo sqlx prepare

```

### Performance Profiling

```


# Build with release optimizations

cargo build --release

# Profile with flamegraph

cargo install flamegraph
sudo flamegraph --bin triptych

```

## Roadmap

### MVP (Complete ✓)
- [x] Natural language task parsing with NLP
- [x] SQLite persistence with migrations
- [x] TUI with vim keybindings
- [x] CLI mode for scripting
- [x] Background sync daemon
- [x] Tiered caching architecture

### Next Phase (In Progress)
- [ ] Command palette (Ctrl+K fuzzy search)
- [ ] Email integration (IMAP IDLE + SMTP)
- [ ] Calendar sync (CalDAV)
- [ ] Unified timeline view
- [ ] Persistent cache to SQLite

### Future Enhancements
- [ ] Recurring tasks with cron-style scheduling
- [ ] Time-blocking with 15/30-minute intervals
- [ ] Quick-add for scheduled blocks (academic/work/personal)
- [ ] Configurable color themes
- [ ] Export to iCalendar format
- [ ] Multi-device sync (optional)

