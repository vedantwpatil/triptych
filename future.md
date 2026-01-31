# Smart Task Scheduling Implementation Plan

## Overview

Implement intelligent task scheduling that allocates tasks to deepwork blocks based on deadlines, with automatic reallocation when new urgent tasks arrive.

## Core Concepts

### Block Types

- **Deepwork** (90min) - Primary blocks for focused task work
- **Admin/Buffer** - Low-cognitive tasks (email, cleaning)
- **Bio-Maintenance** - Sleep, eat, recover (not schedulable)
- **Training** - Exercise (not schedulable)
- **Class** - Lectures (not schedulable)
- **Social/Life** - Unstructured (not schedulable)

### Scheduling Rules

1. Tasks are allocated to deepwork blocks (primarily) or admin blocks
2. All deepwork blocks are equal - any task can go in any block
3. Deadline proximity determines priority
4. When new urgent task arrives, system reallocates all tasks
5. Tasks may span multiple non-consecutive blocks

---

## Phase 1: Database Schema Changes

**File:** `src/migrations.rs`

Add to `run_calendar_migration()`:

```sql
-- Hard deadline (separate from scheduled_at)
ALTER TABLE tasks ADD COLUMN deadline TEXT;

-- Duration in minutes (default 90 for deepwork)
ALTER TABLE tasks ADD COLUMN duration_minutes INTEGER DEFAULT 90;

-- Block allocations: which blocks are assigned to which task
CREATE TABLE IF NOT EXISTS task_block_allocations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    block_date TEXT NOT NULL,           -- "2025-02-03"
    block_start_time TEXT NOT NULL,     -- "10:00"
    block_end_time TEXT NOT NULL,       -- "11:30"
    allocated_minutes INTEGER NOT NULL, -- How much of this block is used
    created_at TEXT DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_allocations_task ON task_block_allocations(task_id);
CREATE INDEX IF NOT EXISTS idx_allocations_date ON task_block_allocations(block_date);
```

---

## Phase 2: Update Task Structs

**File:** `src/nlp/types.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub title: String,
    pub due_date: Option<DateTime<Utc>>,      // When to work on it (scheduled_at)
    pub deadline: Option<DateTime<Utc>>,       // NEW: Hard due date
    pub duration_minutes: Option<i32>,         // NEW: How long task takes
    pub tags: Vec<String>,
    pub priority: Priority,
    pub is_scheduled: bool,
}
```

**File:** `src/app.rs`

```rust
#[derive(Clone, FromRow, Debug)]
pub struct Task {
    pub id: i64,
    pub description: String,
    pub completed: bool,
    pub item_order: Option<i64>,
    pub scheduled_at: Option<DateTime<Utc>>,
    pub deadline: Option<DateTime<Utc>>,       // NEW
    pub duration_minutes: Option<i32>,         // NEW (default 90)
    pub priority: i32,
    pub tags: Option<String>,
    pub natural_language_input: Option<String>,
    pub task_category: Option<String>,
}

#[derive(Clone, FromRow, Debug)]
pub struct TaskBlockAllocation {
    pub id: i64,
    pub task_id: i64,
    pub block_date: String,
    pub block_start_time: String,
    pub block_end_time: String,
    pub allocated_minutes: i32,
}
```

---

## Phase 3: NLP Parsing for Deadline & Duration

**File:** `src/nlp/regex_patterns.rs`

Add patterns:

```rust
// Deadline: "by Friday", "due tomorrow", "due Wednesday"
static DEADLINE_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:by|due|before)\s+(tomorrow|today|monday|tuesday|wednesday|thursday|friday|saturday|sunday)").unwrap()
});

// Duration: "2h", "90m", "duration:2h", "3 hours"
static DURATION_HOURS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(\d+(?:\.\d+)?)\s*(?:h|hours?|hr)\b").unwrap()
});

static DURATION_MINUTES: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(\d+)\s*(?:m|mins?|minutes?)\b").unwrap()
});
```

Add extraction functions:

- `extract_deadline(input) -> Option<DateTime<Utc>>` - Parse "by Friday" to end of that day
- `extract_duration(input) -> Option<i32>` - Parse "2h" to 120 minutes

Update `clean_title()` to remove these patterns from the task title.

---

## Phase 4: Core Scheduling Algorithm

**File:** `src/app.rs`

### Main Reallocation Function

```rust
impl App {
    /// Reallocate all tasks to deepwork blocks based on deadlines
    pub async fn reallocate_all_tasks(&mut self) -> Result<AllocationResult, sqlx::Error> {
        // 1. Get all incomplete tasks with deadlines, sorted by deadline
        let tasks = self.get_tasks_by_deadline().await?;

        // 2. Get all deepwork blocks for next 2 weeks
        let blocks = self.get_available_deepwork_blocks(14).await?;

        // 3. Clear existing allocations
        self.clear_all_allocations().await?;

        // 4. Allocate tasks earliest-deadline-first
        let mut block_usage: HashMap<(NaiveDate, String), i64> = HashMap::new();
        let mut conflicts = Vec::new();

        for task in &tasks {
            let needed_minutes = task.duration_minutes.unwrap_or(90);
            let deadline = match task.deadline {
                Some(dl) => dl,
                None => continue, // Skip tasks without deadlines
            };

            // Find blocks before deadline
            let available: Vec<_> = blocks.iter()
                .filter(|b| b.date < deadline.date_naive())
                .filter(|b| self.block_has_capacity(&block_usage, b, needed_minutes))
                .collect();

            // Allocate across blocks
            let allocated = self.allocate_task_to_blocks(
                task.id,
                needed_minutes,
                &available,
                &mut block_usage
            ).await?;

            if allocated < needed_minutes {
                conflicts.push(TaskConflict {
                    task_id: task.id,
                    description: task.description.clone(),
                    needed: needed_minutes,
                    allocated,
                    deadline,
                });
            }
        }

        self.load_tasks().await?;

        Ok(AllocationResult { conflicts })
    }

    /// Called whenever a task is added or updated
    pub async fn on_task_changed(&mut self) -> Result<(), sqlx::Error> {
        let result = self.reallocate_all_tasks().await?;

        if !result.conflicts.is_empty() {
            self.status_message = Some((
                format!("Warning: {} task(s) cannot fit before deadline", result.conflicts.len()),
                std::time::Instant::now(),
            ));
        }

        Ok(())
    }
}
```

### Helper Functions

```rust
/// Get tasks sorted by deadline (earliest first)
async fn get_tasks_by_deadline(&self) -> Result<Vec<Task>, sqlx::Error>;

/// Get deepwork blocks for next N days
async fn get_available_deepwork_blocks(&self, days: i64) -> Result<Vec<BlockInstance>, sqlx::Error>;

/// Check if a block has remaining capacity
fn block_has_capacity(&self, usage: &HashMap<...>, block: &BlockInstance, needed: i32) -> bool;

/// Allocate a task across available blocks, return minutes actually allocated
async fn allocate_task_to_blocks(...) -> Result<i32, sqlx::Error>;

/// Clear all task-block allocations
async fn clear_all_allocations(&self) -> Result<(), sqlx::Error>;
```

---

## Phase 5: CLI & Schedule Import

**File:** `src/cli.rs`

```rust
#[derive(Subcommand)]
pub enum Commands {
    // ... existing commands ...

    /// Schedule management
    #[command(subcommand)]
    Schedule(ScheduleCommands),
}

#[derive(Subcommand)]
pub enum ScheduleCommands {
    /// Import schedule from TOML file
    Import { file: PathBuf },
    /// Export schedule to TOML file
    Export { #[arg(default_value = "schedule.toml")] file: PathBuf },
    /// Show current week's schedule with allocations
    Show,
    /// Trigger reallocation of all tasks
    Reallocate,
}
```

**File:** `Cargo.toml`

```toml
toml = "0.8"
```

### TOML Format

```toml
# schedule.toml

[[blocks]]
day = "monday"
start = "10:00"
end = "11:30"
type = "deepwork"
title = "Morning Focus"

[[blocks]]
day = "monday"
start = "20:00"
end = "21:30"
type = "deepwork"
title = "Evening Session"

[[blocks]]
day = "tuesday"
start = "09:30"
end = "11:30"
type = "deepwork"
title = "Project Sprint"
```

---

## Phase 6: Integration Points

### Task Addition Flow

```
User: "MATH 475 homework by Wednesday 3h"

1. NLP Parser extracts:
   - title: "MATH 475 homework"
   - deadline: Wednesday 23:59
   - duration: 180 minutes
   - category: "deepwork" (auto-classified)

2. Task inserted into database

3. on_task_changed() triggered:
   - reallocate_all_tasks() runs
   - All tasks sorted by deadline
   - Blocks allocated earliest-deadline-first
   - Conflicts reported if any

4. UI updated to show allocations
```

### Calendar View Updates

The calendar view should show:

- Schedule blocks (from schedule_blocks table)
- Task allocations within blocks (from task_block_allocations table)
- Conflict warnings for tasks that don't fit

---

## Implementation Order

1. **Database migrations** - Add deadline, duration_minutes columns + allocations table
2. **Update Task structs** - Both app.rs and nlp/types.rs
3. **NLP patterns** - Deadline and duration parsing
4. **Core algorithm** - reallocate_all_tasks() and helpers
5. **Integration** - on_task_changed() hook in add_task()
6. **CLI commands** - Schedule import/export/show
7. **UI updates** - Show allocations in calendar view

---

## Testing Strategy

1. **Unit tests for NLP parsing:**
   - "by Friday" → correct deadline DateTime
   - "3h" → 180 minutes
   - "due tomorrow 2h" → both extracted

2. **Integration tests for allocation:**
   - Single task fits in available blocks
   - Multiple tasks sorted by deadline
   - Conflict detection when blocks exhausted
   - Reallocation when new urgent task added

3. **Manual testing:**
   - Import schedule from TOML
   - Add tasks via CLI
   - Verify calendar shows allocations
   - Add urgent task, verify reallocation

---

## Files to Modify

| File                        | Changes                                                              |
| --------------------------- | -------------------------------------------------------------------- |
| `src/migrations.rs`         | Add deadline, duration_minutes columns; create allocations table     |
| `src/app.rs`                | Update Task struct, add allocation logic, add reallocation algorithm |
| `src/nlp/types.rs`          | Add deadline, duration_minutes to NLP Task struct                    |
| `src/nlp/regex_patterns.rs` | Add DEADLINE_PATTERN, DURATION patterns, extraction functions        |
| `src/cli.rs`                | Add Schedule subcommand with Import/Export/Show/Reallocate           |
| `src/main.rs`               | Handle new CLI commands                                              |
| `src/ui.rs`                 | Show task allocations in calendar view                               |
| `Cargo.toml`                | Add `toml = "0.8"` dependency                                        |

---

## Default Durations by Category

```rust
fn default_duration(category: &str) -> i32 {
    match category {
        "deepwork" => 90,
        "admin" => 30,
        "learning" => 60,
        _ => 60,
    }
}
```
