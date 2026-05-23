//! Daemon main loop: watcher events → device sync.
//!
//! The daemon owns a [`Watcher`], polls it for events, and dispatches
//! each [`DeviceEvent`] to [`handle_event`]:
//! - Connected + matches a configured device → run [`run_sync`] with
//!   that device's current mount included in `drive_mounts`.
//! - Disconnected + matches a configured device → drop the device's
//!   mount from internal state.
//! - Anything else → log and move on.
//!
//! Concurrency: single-threaded. Devices plugged around the same time
//! queue up and process sequentially (no parallel syncs).
//!
//! Stop signal: an `Arc<AtomicBool>`. Tests set it directly; production
//! installs a `SIGTERM` handler that flips it (Phase 4.5 / 4.6).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use colored::Colorize;

use crate::config::Config;
use crate::sync::{SyncOpts, SyncSummary, run_sync};
use crate::watcher::{DeviceEvent, Watcher};

const POLL_INTERVAL: Duration = Duration::from_millis(500);

pub struct DaemonOpts {
    pub host_manifest_path: PathBuf,
    pub log_path: PathBuf,
    pub stop: Arc<AtomicBool>,
}

/// Outcome of handling one event. Returned mostly for tests; the main
/// loop discards it after the inline printing.
#[derive(Debug)]
pub enum EventOutcome {
    /// Connect of a known device → sync ran successfully.
    Synced(SyncSummary),
    /// Connect of a device not present in the config.
    UnknownDevice,
    /// Disconnect, or any other non-syncing case.
    NoSync,
}

/// Handle one event, mutating `drive_mounts` as devices plug/unplug.
/// Errors from `run_sync` are returned; the loop logs and continues.
pub fn handle_event(
    event: DeviceEvent,
    config: &Config,
    drive_mounts: &mut HashMap<String, PathBuf>,
    host_manifest_path: &Path,
    log_path: &Path,
) -> Result<EventOutcome> {
    match event {
        DeviceEvent::Connected {
            identity,
            mount_point,
            ..
        } => {
            let Some(device) = config.devices.values().find(|d| identity.matches(d)) else {
                println!("{} unknown device: {:?}", "?".dimmed(), identity);
                return Ok(EventOutcome::UnknownDevice);
            };
            println!("{} {} connected", "↑".green(), device.name.bold());
            if let Some(mp) = mount_point {
                drive_mounts.insert(device.name.clone(), mp);
            }
            let sync_opts = SyncOpts {
                dry_run: false,
                drive_mounts: drive_mounts.clone(),
            };
            let summary = run_sync(config, host_manifest_path, log_path, &sync_opts)?;
            println!(
                "  {} pair(s), {} action(s), {} failure(s)",
                summary.pairs, summary.successes, summary.failures
            );
            Ok(EventOutcome::Synced(summary))
        }
        DeviceEvent::Disconnected { identity } => {
            if let Some(device) = config.devices.values().find(|d| identity.matches(d)) {
                println!("{} {} disconnected", "↓".dimmed(), device.name.dimmed());
                drive_mounts.remove(&device.name);
            }
            Ok(EventOutcome::NoSync)
        }
    }
}

/// Run the daemon main loop until `opts.stop` becomes true.
pub fn run<W: Watcher>(config: &Config, mut watcher: W, opts: &DaemonOpts) -> Result<()> {
    println!("{}", "redlight daemon up — waiting for devices…".dimmed());
    let mut drive_mounts: HashMap<String, PathBuf> = HashMap::new();

    while !opts.stop.load(Ordering::SeqCst) {
        if let Some(event) = watcher.next_event(POLL_INTERVAL) {
            if let Err(e) = handle_event(
                event,
                config,
                &mut drive_mounts,
                &opts.host_manifest_path,
                &opts.log_path,
            ) {
                eprintln!("{} event handling failed: {e}", "✗".red());
            }
        }
    }

    println!("{}", "redlight daemon stopped.".dimmed());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeMap;
    use std::fs as stdfs;

    use tempfile::TempDir;

    use crate::config::{
        Binding, Bridge as BridgeKind, Device, DeviceMatch, DeviceType, Item, ItemKind, Role,
    };
    use crate::watcher::{DeviceIdentity, DeviceKind, MockWatcher};

    fn make_config(host_root: &Path, drive_label: &str) -> Config {
        let mut devices = BTreeMap::new();
        devices.insert(
            "tardis".into(),
            Device {
                name: "tardis".into(),
                device_type: DeviceType::Host,
                bridge: BridgeKind::Fs,
                matcher: DeviceMatch::default(),
                description: None,
            },
        );
        devices.insert(
            "materia".into(),
            Device {
                name: "materia".into(),
                device_type: DeviceType::Drive,
                bridge: BridgeKind::Fs,
                matcher: DeviceMatch {
                    volume_label: Some(drive_label.into()),
                    ..Default::default()
                },
                description: None,
            },
        );

        let mut items = BTreeMap::new();
        items.insert(
            "music".into(),
            Item {
                name: "music".into(),
                kind: ItemKind::Folder,
                category: None,
                description: None,
                include: vec![],
                exclude: vec![],
            },
        );

        let bindings = vec![
            Binding {
                item: "music".into(),
                device: "tardis".into(),
                path: Some(host_root.to_string_lossy().into_owned()),
                role: Role::ReadWrite,
                include: vec![],
                exclude: vec![],
            },
            Binding {
                item: "music".into(),
                device: "materia".into(),
                path: None,
                role: Role::ReadWrite,
                include: vec![],
                exclude: vec![],
            },
        ];

        Config {
            devices,
            items,
            bindings,
        }
    }

    fn touch(p: &Path, content: &str) {
        if let Some(parent) = p.parent() {
            stdfs::create_dir_all(parent).unwrap();
        }
        stdfs::write(p, content).unwrap();
    }

    #[test]
    fn connect_known_drive_triggers_sync_and_transfers_file() {
        let host_dir = TempDir::new().unwrap();
        let drive_dir = TempDir::new().unwrap();
        let state_dir = TempDir::new().unwrap();

        touch(&host_dir.path().join("song.mp3"), "hello");

        let cfg = make_config(host_dir.path(), "MATERIA");
        let mut drive_mounts = HashMap::new();

        let event = DeviceEvent::Connected {
            kind: DeviceKind::Drive,
            identity: DeviceIdentity {
                volume_label: Some("MATERIA".into()),
                ..Default::default()
            },
            mount_point: Some(drive_dir.path().to_path_buf()),
        };

        let outcome = handle_event(
            event,
            &cfg,
            &mut drive_mounts,
            &state_dir.path().join("manifest.toml"),
            &state_dir.path().join("sync_log.toml"),
        )
        .unwrap();

        match outcome {
            EventOutcome::Synced(s) => {
                assert_eq!(s.successes, 1);
                assert_eq!(s.failures, 0);
            }
            _ => panic!("expected Synced outcome, got {outcome:?}"),
        }
        assert_eq!(
            drive_mounts.get("materia"),
            Some(&drive_dir.path().to_path_buf())
        );
        assert!(drive_dir.path().join("song.mp3").exists());
    }

    #[test]
    fn connect_unknown_device_is_ignored() {
        let host_dir = TempDir::new().unwrap();
        let state_dir = TempDir::new().unwrap();

        let cfg = make_config(host_dir.path(), "MATERIA");
        let mut drive_mounts = HashMap::new();

        let event = DeviceEvent::Connected {
            kind: DeviceKind::Drive,
            identity: DeviceIdentity {
                volume_label: Some("STRANGER".into()),
                ..Default::default()
            },
            mount_point: Some("/some/path".into()),
        };

        let outcome = handle_event(
            event,
            &cfg,
            &mut drive_mounts,
            &state_dir.path().join("manifest.toml"),
            &state_dir.path().join("sync_log.toml"),
        )
        .unwrap();

        assert!(matches!(outcome, EventOutcome::UnknownDevice));
        assert!(drive_mounts.is_empty());
    }

    #[test]
    fn disconnect_removes_drive_from_state() {
        let host_dir = TempDir::new().unwrap();
        let state_dir = TempDir::new().unwrap();

        let cfg = make_config(host_dir.path(), "MATERIA");
        let mut drive_mounts = HashMap::new();
        drive_mounts.insert("materia".into(), PathBuf::from("/Volumes/MATERIA"));

        let event = DeviceEvent::Disconnected {
            identity: DeviceIdentity {
                volume_label: Some("MATERIA".into()),
                ..Default::default()
            },
        };

        let outcome = handle_event(
            event,
            &cfg,
            &mut drive_mounts,
            &state_dir.path().join("manifest.toml"),
            &state_dir.path().join("sync_log.toml"),
        )
        .unwrap();

        assert!(matches!(outcome, EventOutcome::NoSync));
        assert!(!drive_mounts.contains_key("materia"));
    }

    #[test]
    fn run_returns_when_stop_is_already_set() {
        // Smoke test: when stop is true at entry, run() exits without
        // touching the watcher.
        let state_dir = TempDir::new().unwrap();
        let host_dir = TempDir::new().unwrap();
        let cfg = make_config(host_dir.path(), "MATERIA");
        let watcher = MockWatcher::new();
        let opts = DaemonOpts {
            host_manifest_path: state_dir.path().join("manifest.toml"),
            log_path: state_dir.path().join("sync_log.toml"),
            stop: Arc::new(AtomicBool::new(true)),
        };
        run(&cfg, watcher, &opts).unwrap();
    }
}
