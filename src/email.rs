pub(crate) mod client;
pub mod config;
pub mod message;
pub mod priority;
pub(crate) mod store;
pub(crate) mod sync;

pub use client::{ImapMailSource, MailSource};
pub use config::EmailConfig;
pub use message::EmailMessage;
pub use priority::EmailSort;
