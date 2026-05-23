//! High-level orchestration: walk the config, sync each present
//! host ↔ drive pair for each shared item.
//!
//! Scope (v0.0.1):
//! - Host is always present.
//! - Drives are "present" iff their mount point is reachable (either
//!   supplied via [`SyncOpts::drive_mounts`] or auto-discovered at
//!   `/Volumes/<label>` or `/media/<label>`).
//! - Phones are skipped — automatic detection comes with the daemon
//!   watcher in Phase 4.
//!
//! Topology: only host ↔ drive pairs run. We don't do direct
//! drive ↔ drive: when both drives are bound to the same item *and*
//! the host is bound to it too, transitivity through the host keeps
//! them in agreement. The edge case "two drives share an item with
//! no host binding" is out of scope — see PLAN.md "Deferred".

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use colored::Colorize;

use crate::bridges::{Bridge, FsBridge, ensure_bootstrap};
use crate::config::{Binding, Config, Device, DeviceType, Item};
use crate::manifest::Manifest;
use crate::sync_log::SyncLog;

use super::diff::compute_diff;
use super::reconcile::{SyncAction, reconcile};
use super::transfer::{SideRef, execute_actions};

/// Where a passive device's manifest lives, relative to its root.
const PASSIVE_MANIFEST_PATH: &str = "/.redlight/manifest.toml";

#[derive(Debug, Clone, Default)]
pub struct SyncOpts {
    pub dry_run: bool,
    /// Override drive mount points: `device_name -> mount path`. When
    /// absent for a device, the orchestrator falls back to
    /// `/Volumes/<label>` (macOS) or `/media/<label>` (Linux).
    pub drive_mounts: HashMap<String, PathBuf>,
}

#[derive(Debug, Default)]
pub struct SyncSummary {
    pub pairs: u32,
    pub successes: u32,
    pub failures: u32,
    pub skipped_devices: Vec<String>,
}

/// Drive each available host↔drive pair through diff → reconcile →
/// transfer. Returns aggregate counters. Per-pair / per-action output
/// is printed inline.
///
/// Each executed action is appended to `log_path` (the host-side
/// [`SyncLog`]). v0.0.1 logs only on the host; mirroring to passive
/// devices is deferred.
pub fn run_sync(
    config: &Config,
    host_manifest_path: &Path,
    log_path: &Path,
    opts: &SyncOpts,
) -> Result<SyncSummary> {
    let host = pick_host(config)?;

    let mut summary = SyncSummary::default();
    let mut log = SyncLog::open(log_path)
        .with_context(|| format!("opening sync log at {}", log_path.display()))?;

    for device in config.devices.values() {
        match device.device_type {
            DeviceType::Host => continue,
            DeviceType::Phone => {
                summary.skipped_devices.push(format!(
                    "{} (phone — auto-detection in Phase 4)",
                    device.name
                ));
            }
            DeviceType::Drive => match drive_mount(device, opts) {
                Some(mount) => {
                    sync_host_with_drive(
                        host,
                        device,
                        &mount,
                        config,
                        host_manifest_path,
                        opts,
                        &mut summary,
                        &mut log,
                    )?;
                }
                None => {
                    summary
                        .skipped_devices
                        .push(format!("{} (drive not mounted)", device.name));
                }
            },
        }
    }

    Ok(summary)
}

/// Pick the host device that represents this running machine.
///
/// Rules:
/// 1. If a host's `match.hostname` equals the current OS hostname → use it.
/// 2. Else, if exactly one host is declared → use it (back-compat for
///    single-machine configs).
/// 3. Else (multiple hosts, none matches hostname) → bail with a clear
///    error pointing the user at `match.hostname`.
fn pick_host(config: &Config) -> Result<&Device> {
    let hosts: Vec<&Device> = config
        .devices
        .values()
        .filter(|d| d.device_type == DeviceType::Host)
        .collect();
    if hosts.is_empty() {
        bail!("no host device declared in config");
    }
    let current = current_hostname();
    if let Some(h) = hosts
        .iter()
        .find(|d| d.matcher.hostname.as_deref() == Some(current.as_str()))
    {
        return Ok(*h);
    }
    if hosts.len() == 1 {
        return Ok(hosts[0]);
    }
    bail!(
        "multiple host devices declared but none matches this machine's hostname '{}'. \
         Set `match.hostname` on the relevant host in devices.toml.",
        current
    );
}

fn current_hostname() -> String {
    gethostname::gethostname().to_string_lossy().into_owned()
}

fn drive_mount(device: &Device, opts: &SyncOpts) -> Option<PathBuf> {
    if let Some(p) = opts.drive_mounts.get(&device.name) {
        return Some(p.clone());
    }
    let label = device.matcher.volume_label.as_deref()?;
    for prefix in ["/Volumes", "/media"] {
        let p = PathBuf::from(prefix).join(label);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn sync_host_with_drive(
    host: &Device,
    drive: &Device,
    mount: &Path,
    config: &Config,
    host_manifest_path: &Path,
    opts: &SyncOpts,
    summary: &mut SyncSummary,
    log: &mut SyncLog,
) -> Result<()> {
    let host_bridge = FsBridge::host();
    let drive_bridge = FsBridge::at_mount(mount);

    ensure_bootstrap(&drive_bridge, &drive.name)?;

    let mut host_manifest = if host_manifest_path.exists() {
        Manifest::load(host_manifest_path)?
    } else {
        Manifest::new(&host.name)
    };

    let drive_manifest_raw = drive_bridge.read_text(Path::new(PASSIVE_MANIFEST_PATH))?;
    let mut drive_manifest: Manifest =
        toml::from_str(&drive_manifest_raw).context("parsing drive manifest")?;

    for (item_name, item) in &config.items {
        let host_binding = config
            .bindings
            .iter()
            .find(|b| &b.item == item_name && b.device == host.name);
        let drive_binding = config
            .bindings
            .iter()
            .find(|b| &b.item == item_name && b.device == drive.name);

        let (Some(hb), Some(db)) = (host_binding, drive_binding) else {
            continue;
        };

        sync_item(
            item,
            hb,
            host,
            &host_bridge,
            &mut host_manifest,
            db,
            drive,
            &drive_bridge,
            &mut drive_manifest,
            opts,
            summary,
            log,
        )?;
    }

    if !opts.dry_run {
        host_manifest
            .save(host_manifest_path)
            .with_context(|| format!("saving host manifest to {}", host_manifest_path.display()))?;
        let s = toml::to_string_pretty(&drive_manifest).context("serializing drive manifest")?;
        drive_bridge
            .write_text(Path::new(PASSIVE_MANIFEST_PATH), &s)
            .context("saving drive manifest")?;
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn sync_item(
    item: &Item,
    host_binding: &Binding,
    host: &Device,
    host_bridge: &dyn Bridge,
    host_manifest: &mut Manifest,
    drive_binding: &Binding,
    drive: &Device,
    drive_bridge: &dyn Bridge,
    drive_manifest: &mut Manifest,
    opts: &SyncOpts,
    summary: &mut SyncSummary,
    log: &mut SyncLog,
) -> Result<()> {
    println!(
        "{} {} {} ↔ {}",
        item.name.bold(),
        "·".dimmed(),
        host.name.dimmed(),
        drive.name.dimmed()
    );

    let diff_h = compute_diff(host_bridge, host_binding, item, host, host_manifest)?;
    let diff_d = compute_diff(drive_bridge, drive_binding, item, drive, drive_manifest)?;
    let actions = reconcile(&diff_h, host_binding, &diff_d, drive_binding);

    summary.pairs += 1;

    if actions.is_empty() {
        println!("  {}", "rien à faire".dimmed());
        return Ok(());
    }

    if opts.dry_run {
        for a in &actions {
            println!("  {} {}", "→".yellow(), describe(a));
        }
        return Ok(());
    }

    let host_root = host_binding.resolved_path(host);
    let drive_root = drive_binding.resolved_path(drive);
    let side_h = SideRef {
        name: &host.name,
        bridge: host_bridge,
        binding_root: &host_root,
    };
    let side_d = SideRef {
        name: &drive.name,
        bridge: drive_bridge,
        binding_root: &drive_root,
    };

    let report = execute_actions(
        actions,
        &item.name,
        &side_h,
        host_manifest,
        &side_d,
        drive_manifest,
        log,
    );

    for action in &report.successes {
        println!("  {} {}", "✓".green(), describe(action));
    }
    for (action, err) in &report.failures {
        println!(
            "  {} {}: {}",
            "✗".red(),
            describe(action),
            err.to_string().red()
        );
    }

    summary.successes += report.successes.len() as u32;
    summary.failures += report.failures.len() as u32;

    Ok(())
}

fn describe(action: &SyncAction) -> String {
    match action {
        SyncAction::Push { from, to, path, .. } => {
            format!("push {} ({} → {})", path.display(), from, to)
        }
        SyncAction::Delete { device, path } => {
            format!("delete {} on {}", path.display(), device)
        }
    }
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
                path: None, // = drive root
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
    fn sync_pushes_new_file_from_host_to_drive() {
        let host_dir = TempDir::new().unwrap();
        let drive_dir = TempDir::new().unwrap();
        let config_dir = TempDir::new().unwrap();

        touch(&host_dir.path().join("song.mp3"), "hello");

        let cfg = make_config(host_dir.path(), "MATERIA");
        let opts = SyncOpts {
            dry_run: false,
            drive_mounts: HashMap::from([("materia".into(), drive_dir.path().to_path_buf())]),
        };
        let summary = run_sync(
            &cfg,
            &config_dir.path().join("manifest.toml"),
            &config_dir.path().join("sync_log.toml"),
            &opts,
        )
        .unwrap();

        assert_eq!(summary.pairs, 1);
        assert_eq!(summary.successes, 1);
        assert_eq!(summary.failures, 0);
        assert_eq!(
            stdfs::read_to_string(drive_dir.path().join("song.mp3")).unwrap(),
            "hello"
        );
    }

    #[test]
    fn sync_appends_to_host_log() {
        use crate::sync_log::{LogLevel, Operation, SyncLog};

        let host_dir = TempDir::new().unwrap();
        let drive_dir = TempDir::new().unwrap();
        let config_dir = TempDir::new().unwrap();

        touch(&host_dir.path().join("song.mp3"), "hello");

        let cfg = make_config(host_dir.path(), "MATERIA");
        let opts = SyncOpts {
            dry_run: false,
            drive_mounts: HashMap::from([("materia".into(), drive_dir.path().to_path_buf())]),
        };
        let log_path = config_dir.path().join("sync_log.toml");

        run_sync(
            &cfg,
            &config_dir.path().join("manifest.toml"),
            &log_path,
            &opts,
        )
        .unwrap();

        let log = SyncLog::open(&log_path).unwrap();
        assert_eq!(log.len(), 1);
        let entry = &log.entries()[0];
        assert_eq!(entry.level, LogLevel::Info);
        assert_eq!(entry.device, "materia");
        assert_eq!(entry.item, "music");
        assert_eq!(entry.operation, Operation::Create);
        assert_eq!(entry.file, "song.mp3");
        assert!(entry.error.is_none());
    }

    #[test]
    fn dry_run_doesnt_transfer_or_write_manifests() {
        let host_dir = TempDir::new().unwrap();
        let drive_dir = TempDir::new().unwrap();
        let config_dir = TempDir::new().unwrap();

        touch(&host_dir.path().join("song.mp3"), "hello");

        let cfg = make_config(host_dir.path(), "MATERIA");
        let opts = SyncOpts {
            dry_run: true,
            drive_mounts: HashMap::from([("materia".into(), drive_dir.path().to_path_buf())]),
        };
        let host_manifest_path = config_dir.path().join("manifest.toml");
        let summary = run_sync(
            &cfg,
            &host_manifest_path,
            &config_dir.path().join("sync_log.toml"),
            &opts,
        )
        .unwrap();

        assert_eq!(summary.pairs, 1);
        assert_eq!(summary.successes, 0);
        assert!(!drive_dir.path().join("song.mp3").exists());
        assert!(!host_manifest_path.exists());
    }

    #[test]
    fn second_sync_run_is_a_no_op() {
        let host_dir = TempDir::new().unwrap();
        let drive_dir = TempDir::new().unwrap();
        let config_dir = TempDir::new().unwrap();

        touch(&host_dir.path().join("song.mp3"), "hello");

        let cfg = make_config(host_dir.path(), "MATERIA");
        let opts = SyncOpts {
            dry_run: false,
            drive_mounts: HashMap::from([("materia".into(), drive_dir.path().to_path_buf())]),
        };
        let host_manifest_path = config_dir.path().join("manifest.toml");

        // First sync: 1 transfer.
        let s1 = run_sync(
            &cfg,
            &host_manifest_path,
            &config_dir.path().join("sync_log.toml"),
            &opts,
        )
        .unwrap();
        assert_eq!(s1.successes, 1);

        // Second sync with nothing changed: should be 0 transfers.
        let s2 = run_sync(
            &cfg,
            &host_manifest_path,
            &config_dir.path().join("sync_log.toml"),
            &opts,
        )
        .unwrap();
        assert_eq!(s2.successes, 0);
        assert_eq!(s2.failures, 0);
    }

    #[test]
    fn pulls_file_added_on_drive_back_to_host() {
        let host_dir = TempDir::new().unwrap();
        let drive_dir = TempDir::new().unwrap();
        let config_dir = TempDir::new().unwrap();

        touch(&drive_dir.path().join("only-on-drive.mp3"), "from drive");

        let cfg = make_config(host_dir.path(), "MATERIA");
        let opts = SyncOpts {
            dry_run: false,
            drive_mounts: HashMap::from([("materia".into(), drive_dir.path().to_path_buf())]),
        };
        run_sync(
            &cfg,
            &config_dir.path().join("manifest.toml"),
            &config_dir.path().join("sync_log.toml"),
            &opts,
        )
        .unwrap();

        assert_eq!(
            stdfs::read_to_string(host_dir.path().join("only-on-drive.mp3")).unwrap(),
            "from drive"
        );
    }

    #[test]
    fn skips_drive_when_mount_absent_and_no_override() {
        let host_dir = TempDir::new().unwrap();
        let config_dir = TempDir::new().unwrap();

        let cfg = make_config(host_dir.path(), "NONEXISTENT-LABEL-FOR-TEST");
        let opts = SyncOpts {
            dry_run: false,
            drive_mounts: HashMap::new(),
        };
        let summary = run_sync(
            &cfg,
            &config_dir.path().join("manifest.toml"),
            &config_dir.path().join("sync_log.toml"),
            &opts,
        )
        .unwrap();
        assert_eq!(summary.pairs, 0);
        assert!(
            summary
                .skipped_devices
                .iter()
                .any(|s| s.contains("materia"))
        );
    }

    #[test]
    fn errors_without_a_host_device() {
        let host_dir = TempDir::new().unwrap();
        let config_dir = TempDir::new().unwrap();

        let mut cfg = make_config(host_dir.path(), "MATERIA");
        cfg.devices.remove("tardis");

        let opts = SyncOpts::default();
        let err = run_sync(
            &cfg,
            &config_dir.path().join("manifest.toml"),
            &config_dir.path().join("sync_log.toml"),
            &opts,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("no host"));
    }
}
