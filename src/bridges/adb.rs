//! ADB bridge — Android over USB via `adb` (Android Debug Bridge).
//!
//! Talks to the phone using `adb push`, `adb pull`, and `adb shell`.
//! Faster and more reliable than MTP, at the cost of one-time setup:
//! the user must enable Developer Options + USB Debugging on the phone
//! and authorize the host on first connection.
//!
//! Requires `adb` (Android Platform Tools) on the host. Not a hard
//! dependency of Redlight — installed only when a device's config
//! declares `bridge = "adb"`. See [`AdbBridge::check_prerequisites`].

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use super::{Bridge, FileMeta};

#[derive(Debug, Clone)]
pub struct AdbBridge {
    /// Device serial for `adb -s <serial>`. If `None`, adb picks the only
    /// connected device or errors when several are present.
    serial: Option<String>,
}

impl AdbBridge {
    pub fn new(serial: Option<String>) -> Self {
        Self { serial }
    }

    pub fn serial(&self) -> Option<&str> {
        self.serial.as_deref()
    }

    /// Verify that `adb` is available on the host.
    pub fn check_prerequisites() -> Result<()> {
        Command::new("adb").arg("--version").output().map_err(|e| {
            anyhow::anyhow!(
                "adb not found in PATH ({e}). The bridge is opt-in for devices that declare \
                 `bridge = \"adb\"`. Install manually with `brew install android-platform-tools` \
                 (macOS) or `apt install android-tools-adb` (Debian/Ubuntu)."
            )
        })?;
        Ok(())
    }

    fn cmd(&self) -> Command {
        let mut c = Command::new("adb");
        if let Some(s) = &self.serial {
            c.arg("-s").arg(s);
        }
        c
    }

    /// Run `adb shell <cmd>`, returning stdout on success.
    fn shell(&self, cmd: &str) -> Result<String> {
        let out = self
            .cmd()
            .arg("shell")
            .arg(cmd)
            .output()
            .with_context(|| format!("running adb shell `{cmd}`"))?;
        if !out.status.success() {
            bail!(
                "adb shell `{}` failed: {}",
                cmd,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// Wrap a path in single quotes for safe interpolation into an adb shell
/// command. Any embedded single quote is closed, escaped, and reopened.
fn shell_quote(path: &Path) -> String {
    let s = path.display().to_string();
    format!("'{}'", s.replace('\'', "'\\''"))
}

impl Bridge for AdbBridge {
    fn list_files(&self, start: &Path) -> Result<Vec<FileMeta>> {
        // Toybox `find` (modern Android) supports GNU-style -printf.
        // %P = path relative to the search root; %s = size; %T@ = mtime as epoch.frac
        let cmd = format!(
            "find {} -type f -printf '%P\\t%s\\t%T@\\n' 2>/dev/null",
            shell_quote(start)
        );
        let out = self.shell(&cmd)?;
        let mut files = Vec::new();
        for line in out.lines() {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() != 3 {
                continue;
            }
            let path = PathBuf::from(parts[0]);
            let size: u64 = parts[1].parse().unwrap_or(0);
            let mtime: i64 = parts[2].parse::<f64>().map(|f| f as i64).unwrap_or(0);
            files.push(FileMeta { path, size, mtime });
        }
        Ok(files)
    }

    fn get_file(&self, remote: &Path, local: &Path) -> Result<()> {
        if let Some(parent) = local.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let status = self
            .cmd()
            .arg("pull")
            .arg(remote)
            .arg(local)
            .status()
            .with_context(|| {
                format!("running adb pull {} {}", remote.display(), local.display())
            })?;
        if !status.success() {
            bail!(
                "adb pull {} -> {} failed (exit status {})",
                remote.display(),
                local.display(),
                status
            );
        }
        Ok(())
    }

    fn put_file(&self, local: &Path, remote: &Path) -> Result<()> {
        if let Some(parent) = remote.parent()
            && !parent.as_os_str().is_empty()
        {
            self.shell(&format!("mkdir -p {}", shell_quote(parent)))?;
        }
        let status = self
            .cmd()
            .arg("push")
            .arg(local)
            .arg(remote)
            .status()
            .with_context(|| {
                format!("running adb push {} {}", local.display(), remote.display())
            })?;
        if !status.success() {
            bail!(
                "adb push {} -> {} failed (exit status {})",
                local.display(),
                remote.display(),
                status
            );
        }
        Ok(())
    }

    fn delete_file(&self, path: &Path) -> Result<()> {
        self.shell(&format!("rm {}", shell_quote(path)))?;
        Ok(())
    }

    fn make_dir(&self, path: &Path) -> Result<()> {
        self.shell(&format!("mkdir -p {}", shell_quote(path)))?;
        Ok(())
    }

    fn get_metadata(&self, path: &Path) -> Result<FileMeta> {
        // `stat -c '%s %Y'` → "<size> <mtime>"
        let out = self.shell(&format!("stat -c '%s %Y' {}", shell_quote(path)))?;
        let parts: Vec<&str> = out.split_whitespace().collect();
        if parts.len() != 2 {
            bail!("unexpected stat output for {}: {:?}", path.display(), out);
        }
        let size: u64 = parts[0]
            .parse()
            .with_context(|| format!("parsing size from `{out}`"))?;
        let mtime: i64 = parts[1]
            .parse()
            .with_context(|| format!("parsing mtime from `{out}`"))?;
        Ok(FileMeta {
            path: path.to_path_buf(),
            size,
            mtime,
        })
    }

    fn read_text(&self, path: &Path) -> Result<String> {
        self.shell(&format!("cat {}", shell_quote(path)))
    }

    fn write_text(&self, path: &Path, content: &str) -> Result<()> {
        // adb has no atomic "write inline" — stage to a local temp, then push.
        let tmp = std::env::temp_dir().join(format!(
            "redlight-adb-{}-{}.tmp",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::write(&tmp, content).with_context(|| format!("staging temp {}", tmp.display()))?;
        let result = self.put_file(&tmp, path);
        let _ = std::fs::remove_file(&tmp);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_without_serial_has_no_s_flag() {
        let b = AdbBridge::new(None);
        let cmd = b.cmd();
        let args: Vec<_> = cmd.get_args().collect();
        assert!(args.is_empty());
    }

    #[test]
    fn cmd_with_serial_adds_s_flag() {
        let b = AdbBridge::new(Some("ABCD1234".into()));
        let cmd = b.cmd();
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec!["-s".to_string(), "ABCD1234".to_string()]);
    }

    #[test]
    fn shell_quote_wraps_in_single_quotes() {
        assert_eq!(shell_quote(Path::new("/sdcard/Music")), "'/sdcard/Music'");
    }

    #[test]
    fn shell_quote_escapes_embedded_quote() {
        assert_eq!(shell_quote(Path::new("/sd/it's.mp3")), "'/sd/it'\\''s.mp3'");
    }

    #[test]
    fn serial_round_trip() {
        let b = AdbBridge::new(Some("XYZ".into()));
        assert_eq!(b.serial(), Some("XYZ"));
        let b = AdbBridge::new(None);
        assert_eq!(b.serial(), None);
    }

    // Behavior tests (check_prerequisites, list_files, push/pull, etc.)
    // require adb installed and a connected, authorized device — exercised
    // via `cargo run --example adb_probe`.
}
