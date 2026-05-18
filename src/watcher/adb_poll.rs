//! Polling watcher for Android phones over ADB.
//!
//! Periodically shells out to `adb devices`, diffs against the
//! previous result, and emits Connected/Disconnected events for
//! serials that appear or disappear. Only phones in the "device" state
//! (ready, authorized) count; "unauthorized" / "offline" / "no
//! permissions" rows are ignored — the user can't sync with them yet.
//!
//! Skipped silently if `adb` is not installed (the watcher's `new()`
//! returns an Err).
//!
//! Identity carries only `serial`. Match against the user's config's
//! `match.serial` field — vendor/product aren't available via `adb
//! devices` output, only via the slower `adb shell getprop`.

use std::collections::{BTreeSet, VecDeque};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use super::{DeviceEvent, DeviceIdentity, DeviceKind, Watcher};

pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(3);

pub struct AdbPollWatcher {
    known: BTreeSet<String>,
    pending: VecDeque<DeviceEvent>,
    poll_interval: Duration,
    last_scan: Option<Instant>,
}

impl AdbPollWatcher {
    /// Construct a watcher. Errors if `adb` isn't available on PATH —
    /// the caller decides whether to surface that or include only the
    /// FS watcher.
    pub fn new() -> Result<Self> {
        check_adb()?;
        Ok(Self::with_interval(DEFAULT_POLL_INTERVAL))
    }

    fn with_interval(poll_interval: Duration) -> Self {
        Self {
            known: BTreeSet::new(),
            pending: VecDeque::new(),
            poll_interval,
            last_scan: None,
        }
    }

    fn scan(&mut self) {
        let Ok(current) = list_ready_devices() else {
            // Transient adb failures: just skip this scan.
            return;
        };

        // Disconnections.
        for serial in &self.known {
            if !current.contains(serial) {
                self.pending.push_back(DeviceEvent::Disconnected {
                    identity: DeviceIdentity {
                        serial: Some(serial.clone()),
                        ..Default::default()
                    },
                });
            }
        }

        // Connections.
        for serial in &current {
            if !self.known.contains(serial) {
                self.pending.push_back(DeviceEvent::Connected {
                    kind: DeviceKind::Phone,
                    identity: DeviceIdentity {
                        serial: Some(serial.clone()),
                        ..Default::default()
                    },
                    mount_point: None,
                });
            }
        }

        self.known = current;
    }
}

fn check_adb() -> Result<()> {
    Command::new("adb")
        .arg("--version")
        .output()
        .map_err(|e| anyhow::anyhow!("adb not in PATH ({e})"))?;
    Ok(())
}

fn list_ready_devices() -> Result<BTreeSet<String>> {
    let output = Command::new("adb")
        .arg("devices")
        .output()
        .context("running adb devices")?;
    if !output.status.success() {
        bail!(
            "adb devices failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_adb_devices(&stdout))
}

/// Parse the output of `adb devices` into the set of serials whose
/// state is `device` (authorized + online). Unauthorized / offline /
/// daemon-warning lines are ignored.
fn parse_adb_devices(output: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("List of devices") {
            continue; // Header.
        }
        if line.starts_with('*') {
            continue; // adb daemon startup chatter.
        }
        let mut parts = line.split_whitespace();
        let Some(serial) = parts.next() else { continue };
        let Some(state) = parts.next() else { continue };
        if state == "device" {
            out.insert(serial.to_string());
        }
    }
    out
}

impl Watcher for AdbPollWatcher {
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
        std::thread::sleep(timeout.min(self.poll_interval));
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_typical_two_device_output() {
        let s = "\
List of devices attached
ABC123\tdevice
DEF456\tdevice
";
        let got = parse_adb_devices(s);
        assert_eq!(got.len(), 2);
        assert!(got.contains("ABC123"));
        assert!(got.contains("DEF456"));
    }

    #[test]
    fn skips_unauthorized_devices() {
        let s = "\
List of devices attached
ABC123\tunauthorized
DEF456\tdevice
";
        let got = parse_adb_devices(s);
        assert_eq!(got.len(), 1);
        assert!(got.contains("DEF456"));
    }

    #[test]
    fn skips_offline_devices() {
        let s = "\
List of devices attached
ABC123\toffline
";
        assert!(parse_adb_devices(s).is_empty());
    }

    #[test]
    fn handles_no_devices_attached() {
        let s = "List of devices attached\n\n";
        assert!(parse_adb_devices(s).is_empty());
    }

    #[test]
    fn skips_adb_daemon_chatter() {
        let s = "\
* daemon not running; starting now at tcp:5037
* daemon started successfully
List of devices attached
ABC123\tdevice
";
        let got = parse_adb_devices(s);
        assert_eq!(got.len(), 1);
        assert!(got.contains("ABC123"));
    }

    #[test]
    fn handles_extended_format_with_product_model() {
        // `adb devices -l` output, with extra key=value pairs after state
        let s = "\
List of devices attached
ABC123\tdevice usb:1.7.3 product:husky model:Pixel_8_Pro
";
        let got = parse_adb_devices(s);
        assert_eq!(got.len(), 1);
        assert!(got.contains("ABC123"));
    }
}
