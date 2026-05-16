//! Bridges abstract device I/O.
//!
//! Each bridge knows how to read and write files on one kind of device:
//! the local filesystem (host or mounted drive), MTP over USB (Android by
//! default), or ADB (Android override). The sync engine works against the
//! [`Bridge`] trait regardless of the underlying transport.

use std::path::{Path, PathBuf};

use anyhow::Result;

pub mod fs;

pub use fs::FsBridge;

/// Metadata for a single file. `path` is relative to the root passed to
/// [`Bridge::list_files`]; `size` is in bytes; `mtime` is a Unix timestamp
/// in seconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMeta {
    pub path: PathBuf,
    pub size: u64,
    pub mtime: i64,
}

/// Read/write a single device's files. Implementations are expected to be
/// safe to share across threads (`Send + Sync`); the daemon serializes
/// operations per device but may interleave across devices.
pub trait Bridge: Send + Sync {
    /// Recursively list files under `start`. Returned paths are relative to
    /// `start`. Directories themselves are skipped.
    fn list_files(&self, start: &Path) -> Result<Vec<FileMeta>>;

    /// Copy a file from the device into a local destination on the host.
    /// Parents of `local` are created if missing.
    fn get_file(&self, remote: &Path, local: &Path) -> Result<()>;

    /// Copy a local file onto the device. Parents of `remote` are created
    /// if missing.
    fn put_file(&self, local: &Path, remote: &Path) -> Result<()>;

    /// Delete a file on the device. Errors if the file does not exist.
    fn delete_file(&self, path: &Path) -> Result<()>;

    /// Create a directory on the device, including parents.
    fn make_dir(&self, path: &Path) -> Result<()>;

    /// Read file metadata (size + mtime) without transferring contents.
    fn get_metadata(&self, path: &Path) -> Result<FileMeta>;

    /// Read a UTF-8 text file (used for the per-device manifest and log).
    fn read_text(&self, path: &Path) -> Result<String>;

    /// Write a UTF-8 text file, creating parent directories if missing.
    fn write_text(&self, path: &Path, content: &str) -> Result<()>;
}
