//! Integration tests in one binary, so it links once.
//! `cli` spawns the compiled binary; the rest call the library directly.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod app;
mod cli;
mod email_config;
mod email_message;
mod email_priority;
mod motion;
mod nlp_llm;
mod nlp_rules;
mod ui;
mod urgency;
