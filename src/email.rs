pub mod client;
pub mod config;
pub mod message;
pub mod store;

pub use client::{ImapMailSource, MailSource};
pub use config::EmailConfig;
pub use message::EmailMessage;
