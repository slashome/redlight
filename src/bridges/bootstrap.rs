//! Bootstrap the `.redlight/` structure on a passive device.
//!
//! When the daemon encounters a passive device for the first time, the
//! device's storage may not yet contain `/.redlight/manifest.toml` or
//! `/.redlight/sync_log.toml`. [`ensure_bootstrap`] creates them via the
//! bridge if they're missing. It is **idempotent**: existing files are
//! left untouched.

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::manifest::Manifest;

use super::Bridge;

const REDLIGHT_DIR: &str = "/.redlight";
const MANIFEST_FILE: &str = "manifest.toml";
const SYNC_LOG_FILE: &str = "sync_log.toml";

fn redlight_path(filename: &str) -> PathBuf {
    PathBuf::from(REDLIGHT_DIR).join(filename)
}

/// Ensure `device_name`'s passive-device area is initialized. The manifest
/// is seeded with the current schema version and `device_name`; the log
/// is an empty TOML.
pub fn ensure_bootstrap(bridge: &dyn Bridge, device_name: &str) -> Result<()> {
    // TODO(v0.0.2): treating *any* metadata error as "file missing" also
    // swallows permission errors. Either add a `Bridge::exists()` method
    // or surface `io::ErrorKind` so we can distinguish missing from
    // permission-denied and act differently (the latter shouldn't lead us
    // to attempt a write that will also fail).
    let manifest_path = redlight_path(MANIFEST_FILE);
    if bridge.get_metadata(&manifest_path).is_err() {
        let m = Manifest::new(device_name);
        let s = toml::to_string_pretty(&m).context("serializing initial manifest")?;
        bridge.write_text(&manifest_path, &s)?;
    }

    let log_path = redlight_path(SYNC_LOG_FILE);
    if bridge.get_metadata(&log_path).is_err() {
        bridge.write_text(&log_path, "")?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::TempDir;

    use super::*;
    use crate::bridges::FsBridge;

    #[test]
    fn bootstrap_creates_manifest_and_log_on_empty_device() {
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());

        ensure_bootstrap(&bridge, "jarvis").unwrap();

        let manifest = tmp.path().join(".redlight/manifest.toml");
        let log = tmp.path().join(".redlight/sync_log.toml");
        assert!(manifest.exists());
        assert!(log.exists());
    }

    #[test]
    fn bootstrap_seeds_manifest_with_device_name() {
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());

        ensure_bootstrap(&bridge, "jarvis").unwrap();

        let loaded = Manifest::load(&tmp.path().join(".redlight/manifest.toml")).unwrap();
        assert_eq!(loaded.device, "jarvis");
        assert!(loaded.items.is_empty());
    }

    #[test]
    fn bootstrap_is_idempotent_no_overwrite() {
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());

        // First run creates the files.
        ensure_bootstrap(&bridge, "jarvis").unwrap();

        // Mutate the manifest, then re-run bootstrap.
        let manifest_path = tmp.path().join(".redlight/manifest.toml");
        let mut m = Manifest::load(&manifest_path).unwrap();
        m.update_entry(
            "music",
            "a.mp3",
            crate::manifest::FileEntry {
                hash: "abc".into(),
                mtime: 100,
                size: 1,
                last_sync: 100,
            },
        );
        m.save(&manifest_path).unwrap();

        ensure_bootstrap(&bridge, "jarvis").unwrap();

        // The mutation should still be there: bootstrap didn't overwrite.
        let reloaded = Manifest::load(&manifest_path).unwrap();
        assert!(reloaded.get_entry("music", "a.mp3").is_some());
    }

    #[test]
    fn bootstrap_creates_redlight_directory() {
        let tmp = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(tmp.path());

        ensure_bootstrap(&bridge, "x").unwrap();

        assert!(tmp.path().join(".redlight").is_dir());
    }

    #[test]
    fn redlight_path_uses_device_root() {
        assert_eq!(
            redlight_path("manifest.toml"),
            Path::new("/.redlight/manifest.toml")
        );
    }
}
