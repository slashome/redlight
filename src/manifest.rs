//! Per-device manifest: the known state of every file synced for each item.
//!
//! Lives on each device at `<device-root>/.redlight/manifest.toml`. Serves as
//! the local source of truth used during pairwise reconciliation.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Manifest format version. Bump on any breaking shape change.
pub const CURRENT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub hash: String,
    pub mtime: i64,
    pub size: u64,
    pub last_sync: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemEntries {
    #[serde(default)]
    pub files: HashMap<String, FileEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub device: String,
    #[serde(default)]
    pub items: HashMap<String, ItemEntries>,
}

impl Manifest {
    /// Empty manifest tagged with the given device name and current version.
    pub fn new(device: impl Into<String>) -> Self {
        Self {
            version: CURRENT_VERSION,
            device: device.into(),
            items: HashMap::new(),
        }
    }

    /// Load and validate a manifest from disk.
    pub fn load(path: &Path) -> Result<Self> {
        let s = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let m: Manifest =
            toml::from_str(&s).with_context(|| format!("parsing {}", path.display()))?;
        if m.version != CURRENT_VERSION {
            bail!(
                "manifest version mismatch in {}: expected {}, found {}",
                path.display(),
                CURRENT_VERSION,
                m.version
            );
        }
        Ok(m)
    }

    /// Atomic save: write to a sibling tmp file, fsync, then rename.
    pub fn save(&self, path: &Path) -> Result<()> {
        let parent = path
            .parent()
            .with_context(|| format!("manifest path has no parent: {}", path.display()))?;
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let s = toml::to_string_pretty(self).context("serializing manifest")?;
        let tmp = tmp_sibling(path);
        {
            let mut f =
                fs::File::create(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
            f.write_all(s.as_bytes())
                .with_context(|| format!("writing {}", tmp.display()))?;
            f.sync_all()
                .with_context(|| format!("fsync {}", tmp.display()))?;
        }
        fs::rename(&tmp, path)
            .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
        Ok(())
    }

    pub fn get_entry(&self, item: &str, file: &str) -> Option<&FileEntry> {
        self.items.get(item)?.files.get(file)
    }

    pub fn update_entry(&mut self, item: &str, file: &str, entry: FileEntry) {
        self.items
            .entry(item.to_string())
            .or_default()
            .files
            .insert(file.to_string(), entry);
    }

    pub fn remove_entry(&mut self, item: &str, file: &str) -> Option<FileEntry> {
        self.items.get_mut(item)?.files.remove(file)
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn entry(hash: &str, ts: i64) -> FileEntry {
        FileEntry {
            hash: hash.into(),
            mtime: ts,
            size: 4096,
            last_sync: ts + 1,
        }
    }

    #[test]
    fn new_is_empty_and_versioned() {
        let m = Manifest::new("jarvis");
        assert_eq!(m.version, CURRENT_VERSION);
        assert_eq!(m.device, "jarvis");
        assert!(m.items.is_empty());
    }

    #[test]
    fn update_and_get_entry() {
        let mut m = Manifest::new("jarvis");
        m.update_entry("music", "Beatles/Yesterday.mp3", entry("abc", 100));
        let got = m.get_entry("music", "Beatles/Yesterday.mp3").unwrap();
        assert_eq!(got.hash, "abc");
        assert_eq!(got.size, 4096);
    }

    #[test]
    fn get_entry_missing_returns_none() {
        let m = Manifest::new("jarvis");
        assert!(m.get_entry("music", "nope.mp3").is_none());
    }

    #[test]
    fn remove_entry_returns_old_value() {
        let mut m = Manifest::new("jarvis");
        m.update_entry("music", "a.mp3", entry("x", 1));
        let removed = m.remove_entry("music", "a.mp3");
        assert!(removed.is_some());
        assert!(m.get_entry("music", "a.mp3").is_none());
    }

    #[test]
    fn save_then_load_roundtrip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("manifest.toml");

        let mut m = Manifest::new("jarvis");
        m.update_entry("music", "a.mp3", entry("hash-a", 100));
        m.update_entry("music", "b.mp3", entry("hash-b", 200));
        m.update_entry("admin", "contrat.pdf", entry("hash-c", 300));
        m.save(&path).unwrap();

        let loaded = Manifest::load(&path).unwrap();
        assert_eq!(loaded, m);
    }

    #[test]
    fn save_creates_missing_parent_dir() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested/sub/manifest.toml");
        let m = Manifest::new("jarvis");
        m.save(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn save_is_atomic_no_tmp_left_behind() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("manifest.toml");
        Manifest::new("jarvis").save(&path).unwrap();
        let entries: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(entries, vec!["manifest.toml"]);
    }

    #[test]
    fn save_overwrites_existing_atomically() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("manifest.toml");

        let mut m = Manifest::new("jarvis");
        m.update_entry("music", "a.mp3", entry("v1", 100));
        m.save(&path).unwrap();

        m.update_entry("music", "a.mp3", entry("v2", 200));
        m.save(&path).unwrap();

        let loaded = Manifest::load(&path).unwrap();
        assert_eq!(loaded.get_entry("music", "a.mp3").unwrap().hash, "v2");
    }

    #[test]
    fn load_rejects_wrong_version() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("manifest.toml");
        let bad = r#"
version = 999
device = "jarvis"
"#;
        fs::write(&path, bad).unwrap();
        let err = Manifest::load(&path).unwrap_err();
        assert!(err.to_string().contains("version mismatch"));
    }

    #[test]
    fn load_missing_file_errors_clearly() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("absent.toml");
        let err = Manifest::load(&path).unwrap_err();
        assert!(err.to_string().contains("reading"));
    }
}
