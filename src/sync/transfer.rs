//! Execute reconciled [`SyncAction`]s against two bridges and update both
//! manifests accordingly.
//!
//! Per-action flow for a Push:
//! 1. Pull source file to a host-local temp via `src.get_file`.
//! 2. Compute MD5 of the temp file.
//! 3. Push temp to destination via `dst.put_file`.
//! 4. Re-query destination metadata so its manifest reflects the
//!    actual on-disk `(size, mtime)` — these may differ from the
//!    source's values when `put_file` doesn't preserve mtime.
//! 5. Update both manifests with their respective `FileEntry`s
//!    (same hash, possibly different mtime).
//!
//! For a Delete: remove the file on the target device, then clear the
//! entry from both manifests.
//!
//! Errors are returned per-action; [`execute_actions`] collects them in
//! a [`TransferReport`] and keeps going so one bad file doesn't abort
//! the whole sync.

use std::path::Path;

use anyhow::{Context, Result, bail};
use md5::{Digest, Md5};
use tempfile::NamedTempFile;

use crate::bridges::Bridge;
use crate::manifest::{FileEntry, Manifest};
use crate::sync_log::{LogEntry, LogLevel, Operation, SyncLog, now_unix};

use super::reconcile::SyncAction;

/// Read-only handle to one device participating in the sync.
pub struct SideRef<'a> {
    pub name: &'a str,
    pub bridge: &'a dyn Bridge,
    /// Absolute path of the binding's root on this device. Action paths
    /// (which are relative to the binding root) are joined against this
    /// before being passed to the bridge.
    pub binding_root: &'a Path,
}

#[derive(Debug, Default)]
pub struct TransferReport {
    pub successes: Vec<SyncAction>,
    pub failures: Vec<(SyncAction, anyhow::Error)>,
}

impl TransferReport {
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Run a single action, mutating the two manifests in-place and
/// appending a [`LogEntry`] to `log`. The caller is responsible for
/// persisting the manifests at appropriate checkpoints; the log
/// auto-persists on each append.
#[allow(clippy::too_many_arguments)]
pub fn execute_action(
    action: &SyncAction,
    item: &str,
    side_a: &SideRef<'_>,
    manifest_a: &mut Manifest,
    side_b: &SideRef<'_>,
    manifest_b: &mut Manifest,
    log: &mut SyncLog,
) -> Result<()> {
    let key = action.path().to_string_lossy().into_owned();

    let (target_device, result, op_when_ok) = match action {
        SyncAction::Push {
            from,
            to,
            path,
            size,
            mtime,
        } => {
            // Decide Create vs Update from the destination's manifest
            // *before* the transfer mutates it.
            let was_new = destination_manifest(to, side_a, manifest_a, side_b, manifest_b)
                .map(|m| m.get_entry(item, &key).is_none())
                .unwrap_or(true);
            let res = pick_push_sides(from, to, side_a, side_b).and_then(|(src, dst)| {
                execute_push(
                    path,
                    *size,
                    *mtime,
                    item,
                    src,
                    dst,
                    manifest_a,
                    manifest_b,
                    side_a.name,
                )
            });
            let op = if was_new {
                Operation::Create
            } else {
                Operation::Update
            };
            (to.clone(), res, op)
        }
        SyncAction::Delete { device, path } => {
            let res = pick_delete_side(device, side_a, side_b)
                .and_then(|target| execute_delete(path, item, target, manifest_a, manifest_b));
            (device.clone(), res, Operation::Delete)
        }
    };

    let entry = LogEntry {
        timestamp: now_unix(),
        level: if result.is_ok() {
            LogLevel::Info
        } else {
            LogLevel::Error
        },
        device: target_device,
        item: item.to_string(),
        operation: if result.is_ok() {
            op_when_ok
        } else {
            Operation::Skip
        },
        file: key,
        error: result.as_ref().err().map(|e| e.to_string()),
    };
    // Log failures are best-effort: they shouldn't replace a real error.
    if let Err(e) = log.append(entry) {
        eprintln!("warning: sync log append failed: {e}");
    }

    result
}

fn destination_manifest<'m>(
    to: &str,
    side_a: &SideRef<'_>,
    manifest_a: &'m Manifest,
    side_b: &SideRef<'_>,
    manifest_b: &'m Manifest,
) -> Option<&'m Manifest> {
    if to == side_a.name {
        Some(manifest_a)
    } else if to == side_b.name {
        Some(manifest_b)
    } else {
        None
    }
}

/// Convenience: run every action, collecting per-action successes and
/// failures. Continues on error.
#[allow(clippy::too_many_arguments)]
pub fn execute_actions(
    actions: Vec<SyncAction>,
    item: &str,
    side_a: &SideRef<'_>,
    manifest_a: &mut Manifest,
    side_b: &SideRef<'_>,
    manifest_b: &mut Manifest,
    log: &mut SyncLog,
) -> TransferReport {
    let mut report = TransferReport::default();
    for action in actions {
        match execute_action(&action, item, side_a, manifest_a, side_b, manifest_b, log) {
            Ok(()) => report.successes.push(action),
            Err(e) => report.failures.push((action, e)),
        }
    }
    report
}

// ---------- helpers ----------

fn pick_push_sides<'a>(
    from: &str,
    to: &str,
    side_a: &'a SideRef<'a>,
    side_b: &'a SideRef<'a>,
) -> Result<(&'a SideRef<'a>, &'a SideRef<'a>)> {
    let src = match (from == side_a.name, from == side_b.name) {
        (true, _) => side_a,
        (_, true) => side_b,
        _ => bail!("push from unknown device '{from}'"),
    };
    let dst = match (to == side_a.name, to == side_b.name) {
        (true, _) => side_a,
        (_, true) => side_b,
        _ => bail!("push to unknown device '{to}'"),
    };
    if std::ptr::eq(src as *const _, dst as *const _) {
        bail!("push from '{from}' to '{to}' targets the same side");
    }
    Ok((src, dst))
}

fn pick_delete_side<'a>(
    device: &str,
    side_a: &'a SideRef<'a>,
    side_b: &'a SideRef<'a>,
) -> Result<&'a SideRef<'a>> {
    if device == side_a.name {
        Ok(side_a)
    } else if device == side_b.name {
        Ok(side_b)
    } else {
        bail!("delete on unknown device '{device}'")
    }
}

#[allow(clippy::too_many_arguments)]
fn execute_push(
    path: &Path,
    src_size: u64,
    src_mtime: i64,
    item: &str,
    src: &SideRef<'_>,
    dst: &SideRef<'_>,
    manifest_a: &mut Manifest,
    manifest_b: &mut Manifest,
    side_a_name: &str,
) -> Result<()> {
    let src_full = src.binding_root.join(path);
    let dst_full = dst.binding_root.join(path);

    let tmp = NamedTempFile::new().context("creating temp file for transfer")?;
    src.bridge
        .get_file(&src_full, tmp.path())
        .with_context(|| format!("pulling {} from {}", src_full.display(), src.name))?;

    let hash = compute_md5(tmp.path())
        .with_context(|| format!("hashing temp transfer of {}", path.display()))?;

    dst.bridge
        .put_file(tmp.path(), &dst_full)
        .with_context(|| format!("pushing {} to {}", dst_full.display(), dst.name))?;

    let dst_meta = dst
        .bridge
        .get_metadata(&dst_full)
        .with_context(|| format!("re-querying metadata on {} after push", dst.name))?;

    let now = now_unix();
    let key = path.to_string_lossy().into_owned();

    let src_entry = FileEntry {
        hash: hash.clone(),
        mtime: src_mtime,
        size: src_size,
        last_sync: now,
    };
    let dst_entry = FileEntry {
        hash,
        mtime: dst_meta.mtime,
        size: dst_meta.size,
        last_sync: now,
    };

    let (src_manifest, dst_manifest) = if src.name == side_a_name {
        (&mut *manifest_a, &mut *manifest_b)
    } else {
        (&mut *manifest_b, &mut *manifest_a)
    };
    src_manifest.update_entry(item, &key, src_entry);
    dst_manifest.update_entry(item, &key, dst_entry);

    Ok(())
}

fn execute_delete(
    path: &Path,
    item: &str,
    target: &SideRef<'_>,
    manifest_a: &mut Manifest,
    manifest_b: &mut Manifest,
) -> Result<()> {
    let full = target.binding_root.join(path);
    target
        .bridge
        .delete_file(&full)
        .with_context(|| format!("deleting {} on {}", full.display(), target.name))?;

    let key = path.to_string_lossy().into_owned();
    manifest_a.remove_entry(item, &key);
    manifest_b.remove_entry(item, &key);

    Ok(())
}

fn compute_md5(path: &Path) -> Result<String> {
    use std::fs::File;
    use std::io::Read;

    let mut f = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Md5::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f
            .read(&mut buf)
            .with_context(|| format!("reading {}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs as stdfs;

    use tempfile::TempDir;

    use crate::bridges::FsBridge;

    /// Build a SideRef-shaped tuple alongside the FsBridge so the bridge
    /// stays alive for the duration of the test.
    fn side<'a>(name: &'a str, bridge: &'a FsBridge, binding_root: &'a Path) -> SideRef<'a> {
        SideRef {
            name,
            bridge,
            binding_root,
        }
    }

    /// Open a fresh SyncLog in a TempDir. The directory is returned so
    /// it stays alive for the test's duration.
    fn fresh_log() -> (SyncLog, TempDir) {
        let dir = TempDir::new().unwrap();
        let log = SyncLog::open(dir.path().join("log.toml")).unwrap();
        (log, dir)
    }

    fn touch(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            stdfs::create_dir_all(parent).unwrap();
        }
        stdfs::write(&p, content).unwrap();
    }

    fn known_md5(content: &str) -> String {
        let mut h = Md5::new();
        h.update(content.as_bytes());
        format!("{:x}", h.finalize())
    }

    // ---- Push ----

    #[test]
    fn push_copies_file_to_destination() {
        let tmp_a = TempDir::new().unwrap();
        let tmp_b = TempDir::new().unwrap();
        let br_a = FsBridge::at_mount(tmp_a.path());
        let br_b = FsBridge::at_mount(tmp_b.path());

        // A has a file, B is empty.
        touch(tmp_a.path(), "song.mp3", "hello");

        let root = Path::new("/");
        let side_a = side("a", &br_a, root);
        let side_b = side("b", &br_b, root);
        let mut ma = Manifest::new("a");
        let mut mb = Manifest::new("b");

        let action = SyncAction::Push {
            from: "a".into(),
            to: "b".into(),
            path: "song.mp3".into(),
            size: 5,
            mtime: 100,
        };

        let (mut log, _log_dir) = fresh_log();
        execute_action(
            &action, "music", &side_a, &mut ma, &side_b, &mut mb, &mut log,
        )
        .unwrap();

        let dst = tmp_b.path().join("song.mp3");
        assert!(dst.exists());
        assert_eq!(stdfs::read_to_string(&dst).unwrap(), "hello");
    }

    #[test]
    fn push_updates_both_manifests_with_correct_hash() {
        let tmp_a = TempDir::new().unwrap();
        let tmp_b = TempDir::new().unwrap();
        let br_a = FsBridge::at_mount(tmp_a.path());
        let br_b = FsBridge::at_mount(tmp_b.path());
        touch(tmp_a.path(), "song.mp3", "hello");

        let root = Path::new("/");
        let side_a = side("a", &br_a, root);
        let side_b = side("b", &br_b, root);
        let mut ma = Manifest::new("a");
        let mut mb = Manifest::new("b");

        let action = SyncAction::Push {
            from: "a".into(),
            to: "b".into(),
            path: "song.mp3".into(),
            size: 5,
            mtime: 100,
        };
        let (mut log, _log_dir) = fresh_log();
        execute_action(
            &action, "music", &side_a, &mut ma, &side_b, &mut mb, &mut log,
        )
        .unwrap();

        let expected = known_md5("hello");
        let ea = ma.get_entry("music", "song.mp3").unwrap();
        let eb = mb.get_entry("music", "song.mp3").unwrap();
        assert_eq!(ea.hash, expected);
        assert_eq!(eb.hash, expected);
        assert_eq!(ea.size, 5);
        assert_eq!(eb.size, 5);
        // Source manifest records source's mtime; dest records its own.
        assert_eq!(ea.mtime, 100);
        assert!(eb.mtime > 0);
    }

    #[test]
    fn push_creates_parent_dirs_on_destination() {
        let tmp_a = TempDir::new().unwrap();
        let tmp_b = TempDir::new().unwrap();
        let br_a = FsBridge::at_mount(tmp_a.path());
        let br_b = FsBridge::at_mount(tmp_b.path());
        touch(tmp_a.path(), "Beatles/Yesterday.mp3", "y");

        let root = Path::new("/");
        let side_a = side("a", &br_a, root);
        let side_b = side("b", &br_b, root);
        let mut ma = Manifest::new("a");
        let mut mb = Manifest::new("b");

        let action = SyncAction::Push {
            from: "a".into(),
            to: "b".into(),
            path: "Beatles/Yesterday.mp3".into(),
            size: 1,
            mtime: 100,
        };
        let (mut log, _log_dir) = fresh_log();
        execute_action(
            &action, "music", &side_a, &mut ma, &side_b, &mut mb, &mut log,
        )
        .unwrap();
        assert!(tmp_b.path().join("Beatles/Yesterday.mp3").exists());
    }

    #[test]
    fn push_b_to_a_works_too() {
        let tmp_a = TempDir::new().unwrap();
        let tmp_b = TempDir::new().unwrap();
        let br_a = FsBridge::at_mount(tmp_a.path());
        let br_b = FsBridge::at_mount(tmp_b.path());
        touch(tmp_b.path(), "song.mp3", "world");

        let root = Path::new("/");
        let side_a = side("a", &br_a, root);
        let side_b = side("b", &br_b, root);
        let mut ma = Manifest::new("a");
        let mut mb = Manifest::new("b");

        let action = SyncAction::Push {
            from: "b".into(),
            to: "a".into(),
            path: "song.mp3".into(),
            size: 5,
            mtime: 200,
        };
        let (mut log, _log_dir) = fresh_log();
        execute_action(
            &action, "music", &side_a, &mut ma, &side_b, &mut mb, &mut log,
        )
        .unwrap();

        assert_eq!(
            stdfs::read_to_string(tmp_a.path().join("song.mp3")).unwrap(),
            "world"
        );
        let ea = ma.get_entry("music", "song.mp3").unwrap();
        let eb = mb.get_entry("music", "song.mp3").unwrap();
        assert_eq!(ea.hash, eb.hash);
    }

    // ---- Delete ----

    #[test]
    fn delete_removes_file_and_clears_entries() {
        let tmp_a = TempDir::new().unwrap();
        let tmp_b = TempDir::new().unwrap();
        let br_a = FsBridge::at_mount(tmp_a.path());
        let br_b = FsBridge::at_mount(tmp_b.path());
        touch(tmp_a.path(), "old.mp3", "x");

        let root = Path::new("/");
        let side_a = side("a", &br_a, root);
        let side_b = side("b", &br_b, root);
        let mut ma = Manifest::new("a");
        ma.update_entry(
            "music",
            "old.mp3",
            FileEntry {
                hash: "x".into(),
                size: 1,
                mtime: 50,
                last_sync: 50,
            },
        );
        let mut mb = Manifest::new("b");
        mb.update_entry(
            "music",
            "old.mp3",
            FileEntry {
                hash: "x".into(),
                size: 1,
                mtime: 50,
                last_sync: 50,
            },
        );

        let action = SyncAction::Delete {
            device: "a".into(),
            path: "old.mp3".into(),
        };
        let (mut log, _log_dir) = fresh_log();
        execute_action(
            &action, "music", &side_a, &mut ma, &side_b, &mut mb, &mut log,
        )
        .unwrap();

        assert!(!tmp_a.path().join("old.mp3").exists());
        assert!(ma.get_entry("music", "old.mp3").is_none());
        assert!(mb.get_entry("music", "old.mp3").is_none());
    }

    // ---- Error handling ----

    #[test]
    fn unknown_source_device_errors() {
        let tmp = TempDir::new().unwrap();
        let br = FsBridge::at_mount(tmp.path());
        let root = Path::new("/");
        let side_a = side("a", &br, root);
        let side_b = side("b", &br, root);
        let mut ma = Manifest::new("a");
        let mut mb = Manifest::new("b");

        let action = SyncAction::Push {
            from: "ghost".into(),
            to: "b".into(),
            path: "x".into(),
            size: 1,
            mtime: 1,
        };
        let (mut log, _log_dir) = fresh_log();
        let err = execute_action(
            &action, "music", &side_a, &mut ma, &side_b, &mut mb, &mut log,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("unknown device 'ghost'"));
    }

    #[test]
    fn execute_actions_continues_on_error() {
        let tmp_a = TempDir::new().unwrap();
        let tmp_b = TempDir::new().unwrap();
        let br_a = FsBridge::at_mount(tmp_a.path());
        let br_b = FsBridge::at_mount(tmp_b.path());
        touch(tmp_a.path(), "good.mp3", "g");

        let root = Path::new("/");
        let side_a = side("a", &br_a, root);
        let side_b = side("b", &br_b, root);
        let mut ma = Manifest::new("a");
        let mut mb = Manifest::new("b");

        let actions = vec![
            // Will fail: source file doesn't exist on A.
            SyncAction::Push {
                from: "a".into(),
                to: "b".into(),
                path: "missing.mp3".into(),
                size: 1,
                mtime: 1,
            },
            // Will succeed.
            SyncAction::Push {
                from: "a".into(),
                to: "b".into(),
                path: "good.mp3".into(),
                size: 1,
                mtime: 1,
            },
        ];
        let (mut log, _log_dir) = fresh_log();
        let report = execute_actions(
            actions, "music", &side_a, &mut ma, &side_b, &mut mb, &mut log,
        );

        assert_eq!(report.successes.len(), 1);
        assert_eq!(report.failures.len(), 1);
        assert!(!report.is_clean());
        assert!(tmp_b.path().join("good.mp3").exists());
    }

    #[test]
    fn clean_report_when_all_succeed() {
        let tmp_a = TempDir::new().unwrap();
        let tmp_b = TempDir::new().unwrap();
        let br_a = FsBridge::at_mount(tmp_a.path());
        let br_b = FsBridge::at_mount(tmp_b.path());
        touch(tmp_a.path(), "a.txt", "1");
        touch(tmp_a.path(), "b.txt", "2");

        let root = Path::new("/");
        let side_a = side("a", &br_a, root);
        let side_b = side("b", &br_b, root);
        let mut ma = Manifest::new("a");
        let mut mb = Manifest::new("b");

        let actions = vec![
            SyncAction::Push {
                from: "a".into(),
                to: "b".into(),
                path: "a.txt".into(),
                size: 1,
                mtime: 1,
            },
            SyncAction::Push {
                from: "a".into(),
                to: "b".into(),
                path: "b.txt".into(),
                size: 1,
                mtime: 1,
            },
        ];
        let (mut log, _log_dir) = fresh_log();
        let report = execute_actions(
            actions, "music", &side_a, &mut ma, &side_b, &mut mb, &mut log,
        );
        assert!(report.is_clean());
        assert_eq!(report.successes.len(), 2);
    }

    // ---- Hashing ----

    #[test]
    fn md5_matches_known_values() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("x");
        stdfs::write(&p, "hello").unwrap();
        assert_eq!(compute_md5(&p).unwrap(), "5d41402abc4b2a76b9719d911017c592");
    }

    #[test]
    fn md5_zero_byte_file() {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("empty");
        stdfs::write(&p, "").unwrap();
        assert_eq!(compute_md5(&p).unwrap(), "d41d8cd98f00b204e9800998ecf8427e");
    }
}
