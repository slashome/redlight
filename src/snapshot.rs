//! Per-device snapshots saved on the host after each successful sync.
//!
//! A snapshot is a copy of a passive device's manifest, plus metadata:
//! when the host last saw it and which host did the sync. They let
//! `rl status` show what each device contained the last time it was
//! plugged in — even when the device is currently away.
//!
//! Snapshots live at `<state_dir>/snapshots/<device>.toml`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::manifest::Manifest;

pub const CURRENT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u32,
    pub device: String,
    pub last_synced_at: i64,
    pub last_synced_with: String,
    pub manifest: Manifest,
}

impl Snapshot {
    pub fn new(manifest: Manifest, last_synced_with: impl Into<String>) -> Self {
        Self {
            version: CURRENT_VERSION,
            device: manifest.device.clone(),
            last_synced_at: now_ts(),
            last_synced_with: last_synced_with.into(),
            manifest,
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let s = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let snap: Snapshot =
            toml::from_str(&s).with_context(|| format!("parsing {}", path.display()))?;
        if snap.version != CURRENT_VERSION {
            bail!(
                "snapshot version mismatch in {}: expected {}, found {}",
                path.display(),
                CURRENT_VERSION,
                snap.version
            );
        }
        Ok(snap)
    }

    /// Atomic save into `<dir>/<device>.toml`.
    pub fn save(&self, dir: &Path) -> Result<()> {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join(format!("{}.toml", self.device));
        let mut tmp_name = path.file_name().unwrap().to_os_string();
        tmp_name.push(".tmp");
        let tmp = path.with_file_name(tmp_name);
        let s = toml::to_string_pretty(self).context("serializing snapshot")?;
        fs::write(&tmp, &s).with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, &path)
            .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()))?;
        Ok(())
    }
}

pub fn snapshots_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("snapshots")
}

/// Load every valid snapshot in `dir`, sorted by device name.
/// Invalid files are skipped with a warning on stderr.
pub fn list(dir: &Path) -> Result<Vec<Snapshot>> {
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        match Snapshot::load(&path) {
            Ok(s) => out.push(s),
            Err(e) => {
                eprintln!(
                    "warning: ignoring invalid snapshot {}: {:#}",
                    path.display(),
                    e
                );
            }
        }
    }
    out.sort_by(|a, b| a.device.cmp(&b.device));
    Ok(out)
}

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_manifest(device: &str) -> Manifest {
        let mut m = Manifest::new(device);
        m.update_entry(
            "music",
            "a.mp3",
            crate::manifest::FileEntry {
                hash: "abc".into(),
                mtime: 100,
                size: 4096,
                last_sync: 101,
            },
        );
        m
    }

    #[test]
    fn save_then_load_roundtrip() {
        let dir = TempDir::new().unwrap();
        let snap = Snapshot::new(sample_manifest("materia"), "tardis");
        snap.save(dir.path()).unwrap();

        let loaded = Snapshot::load(&dir.path().join("materia.toml")).unwrap();
        assert_eq!(loaded.device, "materia");
        assert_eq!(loaded.last_synced_with, "tardis");
        assert_eq!(loaded.manifest, snap.manifest);
    }

    #[test]
    fn save_creates_dir() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("a/b/snapshots");
        let snap = Snapshot::new(sample_manifest("materia"), "tardis");
        snap.save(&nested).unwrap();
        assert!(nested.join("materia.toml").exists());
    }

    #[test]
    fn list_returns_sorted_snapshots() {
        let dir = TempDir::new().unwrap();
        Snapshot::new(sample_manifest("zeta"), "tardis")
            .save(dir.path())
            .unwrap();
        Snapshot::new(sample_manifest("alpha"), "tardis")
            .save(dir.path())
            .unwrap();
        Snapshot::new(sample_manifest("materia"), "tardis")
            .save(dir.path())
            .unwrap();

        let snaps = list(dir.path()).unwrap();
        let names: Vec<_> = snaps.iter().map(|s| s.device.as_str()).collect();
        assert_eq!(names, vec!["alpha", "materia", "zeta"]);
    }

    #[test]
    fn list_on_missing_dir_returns_empty() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("does-not-exist");
        let snaps = list(&missing).unwrap();
        assert!(snaps.is_empty());
    }

    #[test]
    fn load_rejects_wrong_version() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("materia.toml");
        let bad = r#"
version = 999
device = "materia"
last_synced_at = 0
last_synced_with = "tardis"

[manifest]
version = 1
device = "materia"
"#;
        fs::write(&path, bad).unwrap();
        let err = Snapshot::load(&path).unwrap_err();
        assert!(err.to_string().contains("version mismatch"));
    }

    #[test]
    fn save_overwrites_existing() {
        let dir = TempDir::new().unwrap();
        Snapshot::new(sample_manifest("materia"), "tardis")
            .save(dir.path())
            .unwrap();

        let mut m2 = sample_manifest("materia");
        m2.update_entry(
            "music",
            "b.mp3",
            crate::manifest::FileEntry {
                hash: "xyz".into(),
                mtime: 200,
                size: 8192,
                last_sync: 201,
            },
        );
        Snapshot::new(m2.clone(), "hal9000")
            .save(dir.path())
            .unwrap();

        let loaded = Snapshot::load(&dir.path().join("materia.toml")).unwrap();
        assert_eq!(loaded.last_synced_with, "hal9000");
        assert!(loaded.manifest.get_entry("music", "b.mp3").is_some());
    }
}
