//! Redlight — USB multi-device sync.

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod bridges;
pub mod config;
pub mod daemon;
pub mod manifest;
pub mod snapshot;
pub mod sync;
pub mod sync_log;
pub mod watcher;
