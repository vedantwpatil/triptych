# Triptych

**Triptych** is a privacy-first, local-first productivity application built in Rust that aims to match the performance and features of tools like Superhuman, TickTick, and Notion Calendar while keeping all your data on your machine.

## ✨ Highlights

- **⚡ Sub-100ms Performance**: CLI commands complete in under 100ms via persistent daemon architecture
- **🧠 Intelligent NLP Parsing**: 3-layer parsing system (cache → regex → Ollama) with 95%+ accuracy
- **🔒 Privacy-First**: All data stored locally in SQLite, no cloud dependencies
- **⌨️ Keyboard-Driven**: Vim-style keybindings with zero input lag (<1ms response time)
- **🚀 Modern Async Architecture**: Built with Tokio for concurrent background tasks
- **📧 Real-time Email**: IMAP IDLE for instant email notifications (in development)

## 🎯 Motivation

Modern productivity tools sacrifice either **privacy** (cloud-only) or **performance** (slow desktop apps). Triptych solves this by:

- Keeping data local with SQLite (privacy + speed)
- Using Rust + async I/O for sub-100ms response times
- Leveraging local LLMs (Ollama) for natural language understanding
- Running a persistent background daemon to eliminate cold starts

## 📦 Installation

### Prerequisites

- Rust 1.70+ ([install](https://rustup.rs/))
- Ollama ([install](https://ollama.ai/)) with Qwen2.5-7B model
- SQLite 3.35+

### Build from Source

```bash
git clone https://github.com/vedantwpatil/triptych.git
cd triptych
cargo build --release

# Install Ollama model
ollama pull qwen2.5:7b

# Run the application
cargo run
```

## 🚀 Usage

### CLI Mode (Quick Commands)

```bash
# Start persistent daemon for instant CLI commands
triptych daemon &

# Add tasks with natural language
triptych add "Submit report tomorrow at 3pm #work !!"
# ✓ Added task: "Submit report" (ID: 42, via daemon)
# Takes <100ms!

# List all tasks
triptych list

# Mark task as complete
triptych done 42

# Remove a task
triptych rm 42

# Clear completed tasks
triptych clear

# Stop daemon
triptych stop
```

### TUI Mode (Interactive Interface)

```bash
# Launch interactive TUI
triptych

# Keybindings:
# j/k       - Navigate tasks
# a         - Add new task
# Enter     - Toggle task completion
# x         - Delete task
# q         - Quit
```

### Natural Language Examples

The NLP parser understands various formats:

```bash
triptych add "Buy groceries tomorrow at 4:12 PM"
triptych add "Team meeting next Monday #important"
triptych add "Fix critical bug today !!! #dev"
triptych add "Call John in the evening #personal"
```

Extracts:

- **Time**: 12/24-hour format with AM/PM
- **Dates**: today, tomorrow, next Monday, etc.
- **Tags**: \#work, \#personal, \#important
- **Priority**: ! (medium), !! (high), !!! (urgent)

## 🏗️ Project Structure

```
src/
├── main.rs                    # Entry point & TUI event loop
├── app.rs                     # Application state & database operations
├── cli.rs                     # Clap CLI argument definitions
├── daemon.rs                  # Persistent Unix socket daemon
├── ui.rs                      # Ratatui UI rendering
├── nlp/                       # Natural language processing
│   ├── mod.rs                 # Module exports
│   ├── parser.rs              # 3-layer parsing (cache→regex→ollama)
│   ├── ollama_client.rs       # Ollama HTTP client
│   ├── regex_patterns.rs      # Fast-path pattern matching
│   └── types.rs               # ParseResult, Task, Event types
└── sync/                      # Background sync workers
    ├── mod.rs                 # Public API
    ├── config.rs              # SyncConfig
    ├── daemon.rs              # SyncDaemon orchestration
    ├── ollama.rs              # Ollama pre-warming worker
    ├── cache.rs               # Cache preloading worker
    ├── email.rs               # Email sync (IMAP IDLE)
    └── calendar.rs            # Calendar sync (CalDAV)

migrations/
└── 20250930194903_initial_schema.sql  # Database schema

Cargo.toml                     # Dependencies & metadata
```

## 🛠️ Tech Stack

- **Runtime**: Rust with Tokio async runtime
- **TUI**: Ratatui + Crossterm for terminal interface
- **Database**: SQLite with SQLx (compile-time checked queries)
- **NLP**: Ollama (Qwen2.5-7B) + Regex with LRU caching
- **IPC**: Unix sockets for CLI-daemon communication
- **CLI**: Clap for argument parsing

## ⚡ Performance Metrics

| Operation        | Before Optimization | After      | Improvement    |
| :--------------- | :------------------ | :--------- | :------------- |
| TUI first parse  | 2-3s                | <500ms     | **80% faster** |
| Cached parse     | 20-30ms             | <1ms       | **95% faster** |
| CLI add (direct) | 5-7s                | 1-3s       | 60% faster     |
| CLI add (daemon) | 5-7s                | **<100ms** | **98% faster** |
| Input lag        | 16-100ms            | <1ms       | Zero lag       |

### NLP Parsing Layers

1. **Exact cache** (<1ms): 100% accuracy for repeated inputs
2. **Fuzzy cache** (2-5ms): 85-99% accuracy with Jaro-Winkler matching
3. **Regex** (20-30ms): 95% accuracy for structured patterns
4. **Ollama** (200-500ms): 85% accuracy for complex natural language

## 🗂️ Database Schema

```sql
CREATE TABLE tasks (
    id INTEGER PRIMARY KEY,
    description TEXT NOT NULL,
    completed BOOLEAN DEFAULT FALSE,
    item_order INTEGER,
    scheduled_at DATETIME,
    priority INTEGER DEFAULT 1,
    tags TEXT,  -- JSON array
    natural_language_input TEXT
);

CREATE TABLE events (
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL,
    start_time DATETIME NOT NULL,
    end_time DATETIME,
    location TEXT,
    tags TEXT
);

CREATE TABLE emails (
    id INTEGER PRIMARY KEY,
    message_id TEXT UNIQUE NOT NULL,
    subject TEXT NOT NULL,
    sender TEXT NOT NULL,
    received_at DATETIME NOT NULL,
    is_read BOOLEAN DEFAULT FALSE
);
```

## 🗺️ Roadmap

### ✅ Completed

- [x] Core NLP parsing with 3-layer architecture
- [x] LRU cache with fuzzy matching
- [x] Background sync daemon for TUI
- [x] Persistent daemon for CLI speed
- [x] Event-driven TUI with zero lag
- [x] 12/24-hour time parsing
- [x] Tags, priorities, and scheduling

### 🚧 In Progress

- [ ] IMAP IDLE email integration
- [ ] CalDAV calendar sync
- [ ] TUI email/calendar views

### 📋 Planned

- [ ] Full-text search and filtering
- [ ] Recurring tasks
- [ ] Desktop notifications
- [ ] Task dependencies
- [ ] Export/import (JSON, CSV)
- [ ] OAuth2 for Gmail
- [ ] Multi-account support
- [ ] Statistics dashboard

## 🔧 Configuration

### Daemon Configuration

Edit `src/sync/config.rs`:

```rust
impl Default for SyncConfig {
    fn default() -> Self {
        Self {
            ollama_warmup_enabled: true,
            cache_preload_enabled: true,
            email_sync_enabled: false,     // Enable when IMAP configured
            calendar_sync_enabled: false,
            email_check_interval_secs: 300,
        }
    }
}
```

### IMAP Setup (Coming Soon)

```toml
[sync]
email_enabled = true
imap_server = "imap.gmail.com"
imap_port = 993
imap_username = "your-email@gmail.com"
imap_password = "your-app-password"
```

## 🐛 Troubleshooting

### Ollama Connection Issues

```bash
# Check if Ollama is running
ollama list

# Start Ollama service
ollama serve

# Pre-warm model
ollama run qwen2.5:7b "test"
```

### Daemon Not Starting

```bash
# Check if socket exists
ls /tmp/triptych.sock

# Remove stale socket
rm /tmp/triptych.sock

# Restart daemon
triptych daemon
```

### Slow NLP Parsing

Switch to smaller model for 3-5x faster parsing:

```bash
ollama pull qwen2.5:1.5b
```

Edit `src/nlp/ollama_client.rs` to use `qwen2.5:1.5b`.

## 🙏 Acknowledgments

- Built with [Ratatui](https://ratatui.rs/) for TUI
- Powered by [Ollama](https://ollama.ai/) for local LLM inference
- Inspired by [Superhuman](https://superhuman.com/), [TickTick](https://ticktick.com/), and [Notion Calendar](https://www.notion.so/product/calendar)

---

**Status**: Active development | **Version**: 0.1.0-alpha | **Rust**: 1.70+
