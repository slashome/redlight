//! Sync operation log.
//!
//! Append-only structured record of sync events (create/update/delete/skip per
//! file). Persisted as TOML, rotated at [`MAX_ENTRIES`] to keep the file bounded.
//! Lives on the host at `~/.local/state/redlight/sync_log.toml` and is mirrored
//! to each touched device at `<device-root>/.redlight/sync_log.toml`.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Create,
    Update,
    Delete,
    Skip,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: i64,
    pub level: LogLevel,
    pub device: String,
    pub item: String,
    pub operation: Operation,
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct LogFile {
    #[serde(default)]
    entry: Vec<LogEntry>,
}

/// Maximum number of entries kept on disk. Older entries are dropped.
pub const MAX_ENTRIES: usize = 500;

#[derive(Debug)]
pub struct SyncLog {
    path: PathBuf,
    entries: Vec<LogEntry>,
}

impl SyncLog {
    /// Open the log at `path`. If the file does not exist, returns an empty log.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let entries = if path.exists() {
            let s =
                fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
            let f: LogFile =
                toml::from_str(&s).with_context(|| format!("parsing {}", path.display()))?;
            f.entry
        } else {
            Vec::new()
        };
        Ok(Self { path, entries })
    }

    pub fn entries(&self) -> &[LogEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Append an entry, rotate if over [`MAX_ENTRIES`], persist to disk.
    pub fn append(&mut self, entry: LogEntry) -> Result<()> {
        self.entries.push(entry);
        if self.entries.len() > MAX_ENTRIES {
            let drop_count = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(0..drop_count);
        }
        self.flush()
    }

    fn flush(&self) -> Result<()> {
        let parent = self
            .path
            .parent()
            .with_context(|| format!("log path has no parent: {}", self.path.display()))?;
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let f = LogFile {
            entry: self.entries.clone(),
        };
        let s = toml::to_string_pretty(&f).context("serializing log")?;
        let tmp = tmp_sibling(&self.path);
        {
            let mut h =
                fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
            h.write_all(s.as_bytes())
                .with_context(|| format!("writing {}", tmp.display()))?;
            h.sync_all()
                .with_context(|| format!("fsync {}", tmp.display()))?;
        }
        fs::rename(&tmp, &self.path)
            .with_context(|| format!("renaming {} -> {}", tmp.display(), self.path.display()))?;
        Ok(())
    }
}

fn tmp_sibling(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".tmp");
    path.with_file_name(name)
}

/// Current Unix timestamp in seconds. Returns 0 if the system clock is broken.
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn entry(file: &str, op: Operation, level: LogLevel, ts: i64) -> LogEntry {
        LogEntry {
            timestamp: ts,
            level,
            device: "jarvis".into(),
            item: "music".into(),
            operation: op,
            file: file.into(),
            error: None,
        }
    }

    #[test]
    fn open_missing_file_returns_empty_log() {
        let dir = TempDir::new().unwrap();
        let log = SyncLog::open(dir.path().join("absent.toml")).unwrap();
        assert!(log.is_empty());
        assert_eq!(log.len(), 0);
    }

    #[test]
    fn append_then_read_back() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sync_log.toml");

        let mut log = SyncLog::open(&path).unwrap();
        log.append(entry("a.mp3", Operation::Create, LogLevel::Info, 100))
            .unwrap();
        log.append(entry("b.mp3", Operation::Update, LogLevel::Info, 200))
            .unwrap();

        let reopened = SyncLog::open(&path).unwrap();
        assert_eq!(reopened.len(), 2);
        assert_eq!(reopened.entries()[0].file, "a.mp3");
        assert_eq!(reopened.entries()[1].operation, Operation::Update);
    }

    #[test]
    fn append_with_error_field_serializes_and_loads() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sync_log.toml");
        let mut log = SyncLog::open(&path).unwrap();
        let e = LogEntry {
            timestamp: 42,
            level: LogLevel::Error,
            device: "jarvis".into(),
            item: "music".into(),
            operation: Operation::Skip,
            file: "locked.mp3".into(),
            error: Some("file is locked".into()),
        };
        log.append(e.clone()).unwrap();

        let reopened = SyncLog::open(&path).unwrap();
        assert_eq!(reopened.entries()[0], e);
    }

    #[test]
    fn rotation_keeps_last_max_entries() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sync_log.toml");
        let mut log = SyncLog::open(&path).unwrap();

        for i in 0..(MAX_ENTRIES as i64 + 50) {
            log.append(entry(
                &format!("{i}.mp3"),
                Operation::Create,
                LogLevel::Info,
                i,
            ))
            .unwrap();
        }

        assert_eq!(log.len(), MAX_ENTRIES);
        // The oldest 50 should be gone; the first kept entry should be index 50.
        assert_eq!(log.entries()[0].file, "50.mp3");
        let last = log.entries().last().unwrap();
        assert_eq!(last.file, format!("{}.mp3", MAX_ENTRIES + 50 - 1));
    }

    #[test]
    fn flush_creates_missing_parent_dir() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested/deep/sync_log.toml");
        let mut log = SyncLog::open(&path).unwrap();
        log.append(entry("x.mp3", Operation::Create, LogLevel::Info, 1))
            .unwrap();
        assert!(path.exists());
    }

    #[test]
    fn now_unix_is_after_2024() {
        // Sanity check: current time should be after Jan 1, 2024 (1704067200).
        assert!(now_unix() > 1_704_067_200);
    }
}
