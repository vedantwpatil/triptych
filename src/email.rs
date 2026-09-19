pub(crate) mod client;
pub mod config;
pub mod message;
pub(crate) mod store;

pub use client::{ImapMailSource, MailSource};
pub use config::EmailConfig;
pub use message::EmailMessage;
