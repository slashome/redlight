//! Map a configured [`Device`](crate::config::Device) to a usable
//! [`Bridge`] instance.
//!
//! This is the single place that knows how to translate
//! `(device_type, bridge)` from the user's TOML into an actual transport.
//! Schema validation (in [`crate::config`]) guarantees the combinations
//! reached here are legal; the catch-all arm is defensive.

use std::path::Path;

use anyhow::{Result, bail};

use crate::config::{Bridge as BridgeKind, Device, DeviceType};

use super::{AdbBridge, Bridge, FsBridge, MtpBridge, MtpDeviceMatcher};

/// Construct a usable bridge for `device`.
///
/// `drive_mount` is required for drives — the daemon's volume watcher
/// discovers the current mount point at plug-in time and passes it in.
/// For host and phone devices, it is ignored.
///
/// Side effects:
/// - Phone + MTP: spawns `jmtpfs` to mount the device (may take a few
///   seconds; returns a [`crate::bridges::MountedMtpBridge`] that
///   unmounts on drop).
/// - Phone + ADB: verifies `adb` is installed and surfaces an install
///   hint if not.
pub fn build_bridge(device: &Device, drive_mount: Option<&Path>) -> Result<Box<dyn Bridge>> {
    match (device.device_type, device.bridge) {
        (DeviceType::Host, BridgeKind::Fs) => Ok(Box::new(FsBridge::host())),

        (DeviceType::Drive, BridgeKind::Fs) => {
            let mount = drive_mount.ok_or_else(|| {
                anyhow::anyhow!(
                    "device '{}' (drive) requires a mount point — pass it via `drive_mount`",
                    device.name
                )
            })?;
            Ok(Box::new(FsBridge::at_mount(mount)))
        }

        (DeviceType::Phone, BridgeKind::Mtp) => {
            // TODO(v0.0.2): jmtpfs currently picks the first connected device
            // so vendor/product are ignored — hence the unwrap_or_default.
            // When multi-device disambiguation lands, require those fields
            // and bail out cleanly instead of defaulting to empty strings.
            // TODO(v0.0.2): this eagerly mounts. If the caller never uses
            // the bridge, the mount/unmount cycle is wasted. Consider lazy
            // mount on first op if it becomes a measurable issue.
            let matcher = MtpDeviceMatcher {
                vendor_id: device.matcher.vendor_id.clone().unwrap_or_default(),
                product_id: device.matcher.product_id.clone().unwrap_or_default(),
                serial: device.matcher.serial.clone(),
            };
            let mounted = MtpBridge::new(matcher).mount()?;
            Ok(Box::new(mounted))
        }

        (DeviceType::Phone, BridgeKind::Adb) => {
            AdbBridge::check_prerequisites()?;
            Ok(Box::new(AdbBridge::new(device.matcher.serial.clone())))
        }

        (t, b) => bail!(
            "device '{}': bridge {} incompatible with type {} \
             (should have been caught by config validation)",
            device.name,
            b,
            t
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::TempDir;

    use super::*;
    use crate::config::{Bridge as BridgeKind, Device, DeviceMatch, DeviceType};

    fn make_device(name: &str, t: DeviceType, b: BridgeKind, matcher: DeviceMatch) -> Device {
        Device {
            name: name.into(),
            device_type: t,
            bridge: b,
            matcher,
            description: None,
        }
    }

    #[test]
    fn host_device_returns_usable_bridge() {
        let d = make_device(
            "tardis",
            DeviceType::Host,
            BridgeKind::Fs,
            DeviceMatch::default(),
        );
        let bridge = build_bridge(&d, None).unwrap();
        // Functional check: write a temp file via the bridge, read it back.
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("probe.txt");
        bridge.write_text(&path, "hi").unwrap();
        assert_eq!(bridge.read_text(&path).unwrap(), "hi");
    }

    #[test]
    fn drive_device_uses_supplied_mount() {
        let tmp = TempDir::new().unwrap();
        let d = make_device(
            "materia",
            DeviceType::Drive,
            BridgeKind::Fs,
            DeviceMatch {
                volume_label: Some("MATERIA".into()),
                ..Default::default()
            },
        );
        let bridge = build_bridge(&d, Some(tmp.path())).unwrap();
        // The bridge should resolve "/probe.txt" against the mount.
        bridge.write_text(Path::new("/probe.txt"), "ok").unwrap();
        assert!(tmp.path().join("probe.txt").exists());
    }

    #[test]
    fn drive_device_without_mount_errors() {
        let d = make_device(
            "materia",
            DeviceType::Drive,
            BridgeKind::Fs,
            DeviceMatch {
                volume_label: Some("MATERIA".into()),
                ..Default::default()
            },
        );
        let err = build_bridge(&d, None).unwrap_err().to_string();
        assert!(err.contains("mount point"));
    }

    #[test]
    fn defensive_arm_rejects_incompatible_combo() {
        // This combo would be rejected by config validation; we still
        // refuse it here defensively.
        let d = make_device(
            "tardis",
            DeviceType::Host,
            BridgeKind::Mtp,
            DeviceMatch::default(),
        );
        let err = build_bridge(&d, None).unwrap_err().to_string();
        assert!(err.contains("incompatible"));
    }

    // MTP and ADB cases require system tooling (jmtpfs / adb) and a real
    // device — exercised via examples/{mtp,adb}_probe.rs.
}
