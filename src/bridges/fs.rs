//! Filesystem bridge — used for the host and for mounted external drives.
//!
//! For the host, paths are taken as-is (already absolute on the local
//! filesystem). For a drive, the bridge is configured with the drive's
//! current mount point on the host (e.g., `/Volumes/Materia`); device-
//! relative paths like `/Music` resolve to `<mount>/Music`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use walkdir::WalkDir;

use super::{Bridge, FileMeta};

#[derive(Debug, Clone)]
pub struct FsBridge {
    /// Empty for the host (paths used as-is). For a drive: the mount point
    /// on the host filesystem.
    root: PathBuf,
}

impl FsBridge {
    /// Bridge to the local host. Caller-supplied paths are used unchanged.
    pub fn host() -> Self {
        Self {
            root: PathBuf::new(),
        }
    }

    /// Bridge to a volume mounted at `mount`. Device-relative paths are
    /// joined with `mount` (a leading `/` on the device path is stripped).
    pub fn at_mount(mount: impl Into<PathBuf>) -> Self {
        Self { root: mount.into() }
    }

    fn resolve(&self, path: &Path) -> PathBuf {
        if self.root.as_os_str().is_empty() {
            path.to_path_buf()
        } else {
            let stripped = path.strip_prefix("/").unwrap_or(path);
            self.root.join(stripped)
        }
    }
}

fn mtime_secs(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

impl Bridge for FsBridge {
    fn list_files(&self, start: &Path) -> Result<Vec<FileMeta>> {
        let abs_start = self.resolve(start);
        let mut out = Vec::new();
        for entry in WalkDir::new(&abs_start) {
            let entry = entry.with_context(|| format!("walking {}", abs_start.display()))?;
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(&abs_start)
                .with_context(|| format!("strip prefix {}", abs_start.display()))?;
            let meta = entry
                .metadata()
                .with_context(|| format!("metadata for {}", entry.path().display()))?;
            out.push(FileMeta {
                path: rel.to_path_buf(),
                size: meta.len(),
                mtime: mtime_secs(&meta),
            });
        }
        Ok(out)
    }

    fn get_file(&self, remote: &Path, local: &Path) -> Result<()> {
        let src = self.resolve(remote);
        if let Some(parent) = local.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::copy(&src, local)
            .with_context(|| format!("copying {} -> {}", src.display(), local.display()))?;
        Ok(())
    }

    fn put_file(&self, local: &Path, remote: &Path) -> Result<()> {
        let dst = self.resolve(remote);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::copy(local, &dst)
            .with_context(|| format!("copying {} -> {}", local.display(), dst.display()))?;
        Ok(())
    }

    fn delete_file(&self, path: &Path) -> Result<()> {
        let abs = self.resolve(path);
        fs::remove_file(&abs).with_context(|| format!("deleting {}", abs.display()))?;
        Ok(())
    }

    fn make_dir(&self, path: &Path) -> Result<()> {
        let abs = self.resolve(path);
        fs::create_dir_all(&abs).with_context(|| format!("creating {}", abs.display()))?;
        Ok(())
    }

    fn get_metadata(&self, path: &Path) -> Result<FileMeta> {
        let abs = self.resolve(path);
        let meta = fs::metadata(&abs).with_context(|| format!("metadata for {}", abs.display()))?;
        Ok(FileMeta {
            path: path.to_path_buf(),
            size: meta.len(),
            mtime: mtime_secs(&meta),
        })
    }

    fn read_text(&self, path: &Path) -> Result<String> {
        let abs = self.resolve(path);
        fs::read_to_string(&abs).with_context(|| format!("reading {}", abs.display()))
    }

    fn write_text(&self, path: &Path, content: &str) -> Result<()> {
        let abs = self.resolve(path);
        if let Some(parent) = abs.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        fs::write(&abs, content).with_context(|| format!("writing {}", abs.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs as stdfs;
    use tempfile::TempDir;

    fn touch(path: &Path, content: &str) {
        if let Some(p) = path.parent() {
            stdfs::create_dir_all(p).unwrap();
        }
        stdfs::write(path, content).unwrap();
    }

    // ---- resolve ----

    #[test]
    fn host_bridge_resolve_keeps_path_as_is() {
        let b = FsBridge::host();
        assert_eq!(b.resolve(Path::new("/foo/bar")), PathBuf::from("/foo/bar"));
        assert_eq!(b.resolve(Path::new("rel")), PathBuf::from("rel"));
    }

    #[test]
    fn mounted_bridge_prepends_mount_root() {
        let b = FsBridge::at_mount("/Volumes/Materia");
        assert_eq!(
            b.resolve(Path::new("/Music")),
            PathBuf::from("/Volumes/Materia/Music")
        );
        assert_eq!(
            b.resolve(Path::new("Music")),
            PathBuf::from("/Volumes/Materia/Music")
        );
    }

    #[test]
    fn mounted_bridge_root_path_resolves_to_mount() {
        let b = FsBridge::at_mount("/Volumes/Materia");
        assert_eq!(b.resolve(Path::new("/")), PathBuf::from("/Volumes/Materia"));
    }

    // ---- list_files ----

    #[test]
    fn list_files_recurses_and_returns_relative_paths() {
        let dir = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(dir.path());
        touch(&dir.path().join("a.mp3"), "a");
        touch(&dir.path().join("Beatles/Yesterday.mp3"), "yesterday");
        touch(&dir.path().join("Beatles/Help.mp3"), "help");

        let mut files = bridge.list_files(Path::new("/")).unwrap();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let paths: Vec<_> = files.iter().map(|f| f.path.clone()).collect();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("Beatles/Help.mp3"),
                PathBuf::from("Beatles/Yesterday.mp3"),
                PathBuf::from("a.mp3"),
            ]
        );
    }

    #[test]
    fn list_files_skips_directories() {
        let dir = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(dir.path());
        stdfs::create_dir_all(dir.path().join("empty-folder")).unwrap();
        touch(&dir.path().join("file.txt"), "x");

        let files = bridge.list_files(Path::new("/")).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, PathBuf::from("file.txt"));
    }

    #[test]
    fn list_files_reports_size() {
        let dir = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(dir.path());
        touch(&dir.path().join("a.mp3"), "ABCDE");
        let files = bridge.list_files(Path::new("/")).unwrap();
        assert_eq!(files[0].size, 5);
    }

    #[test]
    fn list_files_errors_on_missing_dir() {
        let dir = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(dir.path());
        let r = bridge.list_files(Path::new("/does-not-exist"));
        assert!(r.is_err());
    }

    // ---- get / put / delete / make_dir / get_metadata ----

    #[test]
    fn get_file_copies_to_local() {
        let src_dir = TempDir::new().unwrap();
        let dst_dir = TempDir::new().unwrap();
        touch(&src_dir.path().join("a.txt"), "hello");

        let bridge = FsBridge::at_mount(src_dir.path());
        let dst = dst_dir.path().join("out/a.txt");
        bridge.get_file(Path::new("/a.txt"), &dst).unwrap();
        assert_eq!(stdfs::read_to_string(&dst).unwrap(), "hello");
    }

    #[test]
    fn put_file_copies_to_device_and_creates_parents() {
        let dev_dir = TempDir::new().unwrap();
        let local_dir = TempDir::new().unwrap();
        let local = local_dir.path().join("a.txt");
        touch(&local, "world");

        let bridge = FsBridge::at_mount(dev_dir.path());
        bridge.put_file(&local, Path::new("/sub/b.txt")).unwrap();
        let target = dev_dir.path().join("sub/b.txt");
        assert_eq!(stdfs::read_to_string(&target).unwrap(), "world");
    }

    #[test]
    fn delete_file_removes_it() {
        let dir = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(dir.path());
        let p = dir.path().join("a.txt");
        touch(&p, "x");
        bridge.delete_file(Path::new("/a.txt")).unwrap();
        assert!(!p.exists());
    }

    #[test]
    fn delete_file_errors_when_missing() {
        let dir = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(dir.path());
        let r = bridge.delete_file(Path::new("/nope.txt"));
        assert!(r.is_err());
    }

    #[test]
    fn make_dir_creates_nested() {
        let dir = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(dir.path());
        bridge.make_dir(Path::new("/a/b/c")).unwrap();
        assert!(dir.path().join("a/b/c").is_dir());
    }

    #[test]
    fn get_metadata_returns_size_and_keeps_input_path() {
        let dir = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(dir.path());
        touch(&dir.path().join("a.txt"), "hello!");
        let meta = bridge.get_metadata(Path::new("/a.txt")).unwrap();
        assert_eq!(meta.size, 6);
        assert_eq!(meta.path, PathBuf::from("/a.txt"));
    }

    // ---- read_text / write_text ----

    #[test]
    fn read_write_text_roundtrip() {
        let dir = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(dir.path());
        bridge
            .write_text(Path::new("/manifest.toml"), "hello = 1")
            .unwrap();
        let s = bridge.read_text(Path::new("/manifest.toml")).unwrap();
        assert_eq!(s, "hello = 1");
    }

    #[test]
    fn write_text_creates_parents() {
        let dir = TempDir::new().unwrap();
        let bridge = FsBridge::at_mount(dir.path());
        bridge
            .write_text(Path::new("/.redlight/manifest.toml"), "version = 1")
            .unwrap();
        assert!(dir.path().join(".redlight/manifest.toml").exists());
    }

    // ---- trait object dispatch ----

    #[test]
    fn usable_as_trait_object() {
        let dir = TempDir::new().unwrap();
        let bridge: Box<dyn Bridge> = Box::new(FsBridge::at_mount(dir.path()));
        bridge.make_dir(Path::new("/x")).unwrap();
        assert!(dir.path().join("x").is_dir());
    }
}
