//! Polling watcher: scan mount-root directories at a fixed cadence,
//! diff against the previous scan to derive Connected / Disconnected
//! events.
//!
//! Honest about what it is — a stand-in for the event-driven
//! DiskArbitration / udev backends. The latency is bounded by
//! [`PollWatcher::poll_interval`] (default 2s), which is fine for
//! human-scale plug events but burns a tiny amount of CPU every cycle.

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::{DeviceEvent, DeviceIdentity, DeviceKind, Watcher};

/// How often to re-scan the mount roots when [`Watcher::next_event`]
/// is called repeatedly.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(2);

pub struct PollWatcher {
    scan_dirs: Vec<PathBuf>,
    /// Last known mounts: path → directory name (used as volume label).
    /// BTreeMap so diff iteration is deterministic.
    known: BTreeMap<PathBuf, String>,
    pending: VecDeque<DeviceEvent>,
    poll_interval: Duration,
    last_scan: Option<Instant>,
}

impl PollWatcher {
    /// Watcher with the default scan directories for the current OS.
    pub fn for_os() -> Self {
        let scan_dirs = if cfg!(target_os = "macos") {
            vec![PathBuf::from("/Volumes")]
        } else {
            vec![PathBuf::from("/media"), PathBuf::from("/mnt")]
        };
        Self::new(scan_dirs, DEFAULT_POLL_INTERVAL)
    }

    pub fn new(scan_dirs: Vec<PathBuf>, poll_interval: Duration) -> Self {
        Self {
            scan_dirs,
            known: BTreeMap::new(),
            pending: VecDeque::new(),
            poll_interval,
            last_scan: None,
        }
    }

    /// Walk the scan dirs and queue events for any change since the
    /// previous scan.
    fn scan(&mut self) {
        let current = current_mounts(&self.scan_dirs);

        // Disconnections.
        for (path, label) in &self.known {
            if !current.contains_key(path) {
                self.pending.push_back(DeviceEvent::Disconnected {
                    identity: DeviceIdentity {
                        volume_label: Some(label.clone()),
                        ..Default::default()
                    },
                });
            }
        }

        // Connections.
        for (path, label) in &current {
            if !self.known.contains_key(path) {
                self.pending.push_back(DeviceEvent::Connected {
                    kind: DeviceKind::Drive,
                    identity: DeviceIdentity {
                        volume_label: Some(label.clone()),
                        ..Default::default()
                    },
                    mount_point: Some(path.clone()),
                });
            }
        }

        self.known = current;
    }
}

fn current_mounts(scan_dirs: &[PathBuf]) -> BTreeMap<PathBuf, String> {
    let mut out = BTreeMap::new();
    for dir in scan_dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            // Missing or unreadable scan dir is a no-op.
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()).map(String::from) else {
                continue;
            };
            if name.starts_with('.') {
                continue; // Hidden / system entries.
            }
            // entry.metadata() follows symlinks, which is what we want:
            // /Volumes/Macintosh HD is a symlink to / on macOS.
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_dir() {
                continue;
            }
            out.insert(path, name);
        }
    }
    out
}

impl Watcher for PollWatcher {
    fn next_event(&mut self, timeout: Duration) -> Option<DeviceEvent> {
        if let Some(e) = self.pending.pop_front() {
            return Some(e);
        }
        let should_scan = self
            .last_scan
            .is_none_or(|t| t.elapsed() >= self.poll_interval);
        if should_scan {
            self.scan();
            self.last_scan = Some(Instant::now());
            if let Some(e) = self.pending.pop_front() {
                return Some(e);
            }
        }
        // Nothing changed; sleep so we don't busy-loop. Cap at the
        // poll interval so the daemon stop signal is checked often
        // enough.
        std::thread::sleep(timeout.min(self.poll_interval));
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs as stdfs;
    use std::path::Path;

    use tempfile::TempDir;

    fn make_watcher(dir: &Path) -> PollWatcher {
        PollWatcher::new(vec![dir.to_path_buf()], Duration::from_millis(0))
    }

    #[test]
    fn new_mount_emits_connected_event() {
        let dir = TempDir::new().unwrap();
        let mut w = make_watcher(dir.path());

        stdfs::create_dir(dir.path().join("MATERIA")).unwrap();

        let event = w.next_event(Duration::from_millis(0)).unwrap();
        match event {
            DeviceEvent::Connected {
                kind,
                identity,
                mount_point,
            } => {
                assert_eq!(kind, DeviceKind::Drive);
                assert_eq!(identity.volume_label.as_deref(), Some("MATERIA"));
                assert_eq!(mount_point, Some(dir.path().join("MATERIA")));
            }
            other => panic!("expected Connected, got {other:?}"),
        }
    }

    #[test]
    fn removed_mount_emits_disconnected_event() {
        let dir = TempDir::new().unwrap();
        let mut w = make_watcher(dir.path());

        stdfs::create_dir(dir.path().join("MATERIA")).unwrap();
        // First call: Connected.
        assert!(matches!(
            w.next_event(Duration::from_millis(0)).unwrap(),
            DeviceEvent::Connected { .. }
        ));

        // Remove the "mount".
        stdfs::remove_dir(dir.path().join("MATERIA")).unwrap();
        // Force a re-scan by resetting last_scan.
        w.last_scan = None;

        let event = w.next_event(Duration::from_millis(0)).unwrap();
        match event {
            DeviceEvent::Disconnected { identity } => {
                assert_eq!(identity.volume_label.as_deref(), Some("MATERIA"));
            }
            other => panic!("expected Disconnected, got {other:?}"),
        }
    }

    #[test]
    fn two_polls_with_no_change_yield_no_events() {
        let dir = TempDir::new().unwrap();
        let mut w = make_watcher(dir.path());
        stdfs::create_dir(dir.path().join("MATERIA")).unwrap();

        // First scan emits Connected.
        assert!(matches!(
            w.next_event(Duration::from_millis(0)).unwrap(),
            DeviceEvent::Connected { .. }
        ));
        w.last_scan = None;

        // Second scan: nothing changed.
        assert!(w.next_event(Duration::from_millis(0)).is_none());
    }

    #[test]
    fn multiple_new_mounts_yield_separate_events() {
        let dir = TempDir::new().unwrap();
        let mut w = make_watcher(dir.path());

        stdfs::create_dir(dir.path().join("MATERIA")).unwrap();
        stdfs::create_dir(dir.path().join("XFILES")).unwrap();

        let e1 = w.next_event(Duration::from_millis(0)).unwrap();
        let e2 = w.next_event(Duration::from_millis(0)).unwrap();
        let mut labels: Vec<_> = [e1, e2]
            .into_iter()
            .map(|e| match e {
                DeviceEvent::Connected { identity, .. } => identity.volume_label.unwrap(),
                _ => panic!("expected Connected"),
            })
            .collect();
        labels.sort();
        assert_eq!(labels, vec!["MATERIA", "XFILES"]);

        // Queue drained.
        assert!(w.next_event(Duration::from_millis(0)).is_none());
    }

    #[test]
    fn hidden_entries_are_ignored() {
        let dir = TempDir::new().unwrap();
        let mut w = make_watcher(dir.path());
        stdfs::create_dir(dir.path().join(".Trashes")).unwrap();
        stdfs::create_dir(dir.path().join(".Spotlight-V100")).unwrap();
        stdfs::create_dir(dir.path().join("MATERIA")).unwrap();

        let event = w.next_event(Duration::from_millis(0)).unwrap();
        assert!(matches!(event, DeviceEvent::Connected { ref identity, .. }
            if identity.volume_label.as_deref() == Some("MATERIA")));
        assert!(w.next_event(Duration::from_millis(0)).is_none());
    }

    #[test]
    fn nonexistent_scan_dir_is_silently_ignored() {
        let mut w = PollWatcher::new(
            vec![PathBuf::from("/this/should/not/exist/at/all")],
            Duration::from_millis(0),
        );
        // Scanning a missing dir yields no events and doesn't panic.
        assert!(w.next_event(Duration::from_millis(0)).is_none());
    }

    #[test]
    fn files_are_not_reported_as_mounts() {
        let dir = TempDir::new().unwrap();
        let mut w = make_watcher(dir.path());
        stdfs::write(dir.path().join("not-a-mount.txt"), "x").unwrap();
        assert!(w.next_event(Duration::from_millis(0)).is_none());
    }
}
