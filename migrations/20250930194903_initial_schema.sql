-- Add migration script here
CREATE TABLE IF NOT EXISTS tasks (
    id INTEGER PRIMARY KEY,
    description TEXT NOT NULL,
    completed BOOLEAN NOT NULL DEFAULT FALSE,
    item_order INTEGER,
    scheduled_at TEXT,
    priority INTEGER DEFAULT 0,
    tags TEXT,
    natural_language_input TEXT
);

-- Entity types for timeline
CREATE TABLE IF NOT EXISTS entity_types (
    id INTEGER PRIMARY KEY,
    entity_name TEXT UNIQUE NOT NULL
);

INSERT OR IGNORE INTO entity_types (entity_name) VALUES ('task'), ('event'), ('email');

-- Timeline entries
CREATE TABLE IF NOT EXISTS timeline_entries (
    id INTEGER PRIMARY KEY,
    entity_type_id INTEGER NOT NULL,
    entity_id INTEGER NOT NULL,
    created_at TEXT DEFAULT CURRENT_TIMESTAMP,
    scheduled_at TEXT,
    completed_at TEXT,
    priority INTEGER DEFAULT 0,
    tags TEXT,
    FOREIGN KEY (entity_type_id) REFERENCES entity_types(id)
);

-- Events table
CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL,
    description TEXT,
    start_time TEXT NOT NULL,
    end_time TEXT NOT NULL,
    location TEXT,
    calendar_id TEXT,
    created_at TEXT DEFAULT CURRENT_TIMESTAMP
);

-- Emails table
CREATE TABLE IF NOT EXISTS emails (
    id INTEGER PRIMARY KEY,
    message_id TEXT UNIQUE NOT NULL,
    subject TEXT NOT NULL,
    sender TEXT NOT NULL,
    recipients TEXT,
    body_text TEXT,
    body_html TEXT,
    received_at TEXT NOT NULL,
    folder_name TEXT DEFAULT 'INBOX',
    is_read BOOLEAN DEFAULT FALSE,
    is_flagged BOOLEAN DEFAULT FALSE
);
