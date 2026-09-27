pub(crate) mod client;
pub mod config;
pub(crate) mod drafts;
pub mod message;
pub mod priority;
pub mod smtp;
pub(crate) mod store;
pub(crate) mod sync;
mod tls;

pub use client::{IdleOutcome, ImapMailSource, MailSource};
pub use config::{EmailConfig, SmtpConfig};
pub use drafts::Draft;
pub use message::{EmailAttachment, EmailMessage, EmailRule};
pub use priority::EmailSort;
pub use smtp::OutgoingMessage;
