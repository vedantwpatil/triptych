//! Integration tests in one binary, so it links once.
//! `cli` spawns the compiled binary; the rest call the library directly.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod app;
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
mod motion;
mod nlp_llm;
mod nlp_rules;
mod notify;
mod ui;
mod urgency;
