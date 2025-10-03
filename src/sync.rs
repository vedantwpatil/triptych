mod cache;
mod calendar;
mod config;
mod daemon;
mod email;
mod ollama;

// Re-export public API
pub use config::SyncConfig;
pub use daemon::SyncDaemon;
