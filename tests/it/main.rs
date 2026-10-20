//! Integration tests in one binary, so it links once.
//! `cli` spawns the compiled binary; the rest call the library directly.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    // The suggested `assert_eq!(x, [] as [T; 0])` needs a type annotation at every call site.
    clippy::assert_is_empty
)]

mod allocation_edges;
mod app;
mod calendar_edges;
mod canvas;
mod cli;
mod email_attachments;
mod email_compose;
mod email_config;
mod email_message;
mod email_priority;
mod email_rules;
mod email_smtp;
mod email_thread;
mod links;
mod motion;
mod nlp_llm;
mod nlp_rules;
mod notify;
mod recurrence;
mod tasks_edges;
mod textedit;
mod ui;
mod urgency;
