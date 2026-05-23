//! MTP bridge — Android over USB via `jmtpfs` (FUSE).
//!
//! `MtpBridge` declares the intent to talk to an MTP device.
//! [`MtpBridge::mount`] spawns `jmtpfs` to mount the device into a fresh
//! temp directory and returns a [`MountedMtpBridge`], which delegates all
//! file ops to [`FsBridge`] and unmounts on drop.
//!
//! Requires `jmtpfs` (plus its `libmtp` + FUSE dependencies) installed on
//! the host. There's no homebrew formula for `jmtpfs` on macOS — users
//! install macFUSE via cask and then build `jmtpfs` from source; on
//! Linux it's a regular distro package.
//!
//! Known limitation (v0.0.1): when multiple MTP devices are plugged in,
//! `jmtpfs` selects the first one. The `matcher` field is stored but not
//! yet used to disambiguate.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::{Context, Result, bail};

use super::fs::FsBridge;
use super::{Bridge, FileMeta};

/// Identification criteria for an MTP device. Mirrors the relevant subset
/// of `config::DeviceMatch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MtpDeviceMatcher {
    pub vendor_id: String,
    pub product_id: String,
    pub serial: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MtpBridge {
    matcher: MtpDeviceMatcher,
}

impl MtpBridge {
    pub fn new(matcher: MtpDeviceMatcher) -> Self {
        Self { matcher }
    }

    /// Identification criteria this bridge will look for.
    pub fn matcher(&self) -> &MtpDeviceMatcher {
        &self.matcher
    }

    /// Verify that `jmtpfs` is available. Call before [`Self::mount`] for a
    /// clear error message if the prerequisite is missing.
    pub fn check_prerequisites() -> Result<()> {
        Command::new("jmtpfs")
            .arg("--version")
            .output()
            .map_err(|e| {
                anyhow::anyhow!(
                    "jmtpfs not found in PATH ({e}).\n  \
                     macOS:  no homebrew formula — build from source\n          \
                             (https://github.com/dechamps/jmtpfs)\n          \
                             after `brew install --cask macfuse`\n  \
                     Linux:  `apt install jmtpfs` / `dnf install jmtpfs`"
                )
            })?;
        Ok(())
    }

    /// Mount the device into a fresh temp directory and return a bridge to it.
    pub fn mount(&self) -> Result<MountedMtpBridge> {
        Self::check_prerequisites()?;
        let mount_point = make_mount_point()?;
        let status = Command::new("jmtpfs")
            .arg(&mount_point)
            .status()
            .with_context(|| format!("running jmtpfs on {}", mount_point.display()))?;
        if !status.success() {
            // Clean up the empty mount dir before bailing.
            let _ = std::fs::remove_dir(&mount_point);
            bail!(
                "jmtpfs failed to mount any MTP device at {} (exit status {})",
                mount_point.display(),
                status
            );
        }
        Ok(MountedMtpBridge {
            inner: FsBridge::at_mount(mount_point.clone()),
            mount_point,
        })
    }
}

/// A successfully-mounted MTP device. Implements [`Bridge`] by delegating
/// to an inner [`FsBridge`] pointed at the mount. Unmounts on drop.
#[derive(Debug)]
pub struct MountedMtpBridge {
    inner: FsBridge,
    mount_point: PathBuf,
}

impl MountedMtpBridge {
    pub fn mount_point(&self) -> &Path {
        &self.mount_point
    }
}

impl Drop for MountedMtpBridge {
    fn drop(&mut self) {
        // Best-effort: ignore errors, we're in Drop.
        let _ = unmount(&self.mount_point);
        let _ = std::fs::remove_dir(&self.mount_point);
    }
}

fn make_mount_point() -> Result<PathBuf> {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    let pid = std::process::id();
    let path = std::env::temp_dir().join(format!("redlight-mtp-{pid}-{id}"));
    std::fs::create_dir_all(&path).with_context(|| format!("creating {}", path.display()))?;
    Ok(path)
}

fn unmount(path: &Path) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(s) = Command::new("fusermount").arg("-u").arg(path).status()
            && s.success()
        {
            return Ok(());
        }
    }
    let s = Command::new("umount")
        .arg(path)
        .status()
        .with_context(|| format!("running umount on {}", path.display()))?;
    if !s.success() {
        bail!("umount returned non-zero for {}", path.display());
    }
    Ok(())
}

impl Bridge for MountedMtpBridge {
    fn list_files(&self, start: &Path) -> Result<Vec<FileMeta>> {
        self.inner.list_files(start)
    }

    fn get_file(&self, remote: &Path, local: &Path) -> Result<()> {
        self.inner.get_file(remote, local)
    }

    fn put_file(&self, local: &Path, remote: &Path) -> Result<()> {
        self.inner.put_file(local, remote)
    }

    fn delete_file(&self, path: &Path) -> Result<()> {
        self.inner.delete_file(path)
    }

    fn make_dir(&self, path: &Path) -> Result<()> {
        self.inner.make_dir(path)
    }

    fn get_metadata(&self, path: &Path) -> Result<FileMeta> {
        self.inner.get_metadata(path)
    }

    fn read_text(&self, path: &Path) -> Result<String> {
        self.inner.read_text(path)
    }

    fn write_text(&self, path: &Path, content: &str) -> Result<()> {
        self.inner.write_text(path, content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_mount_point_creates_unique_dirs() {
        let a = make_mount_point().unwrap();
        let b = make_mount_point().unwrap();
        assert_ne!(a, b);
        assert!(a.exists());
        assert!(b.exists());
        std::fs::remove_dir(&a).ok();
        std::fs::remove_dir(&b).ok();
    }

    #[test]
    fn matcher_is_stored() {
        let m = MtpDeviceMatcher {
            vendor_id: "18d1".into(),
            product_id: "4ee7".into(),
            serial: Some("ABC123".into()),
        };
        let bridge = MtpBridge::new(m.clone());
        assert_eq!(bridge.matcher(), &m);
    }

    // Behavior tests (check_prerequisites, mount, unmount) require either
    // jmtpfs installed or a connected device — exercised via
    // `cargo run --example mtp_probe` on the host.
}
