//! Device watcher abstraction.
//!
//! A [`Watcher`] emits [`DeviceEvent`]s as devices are plugged in or
//! unplugged. The daemon main loop (Phase 4.4) polls the watcher's
//! `next_event` method; the actual OS plumbing (IOKit, udev) lives
//! behind the trait.
//!
//! Implementations:
//! - [`MockWatcher`] (this module) — in-memory queue for tests.
//! - macOS backend (Phase 4.2) — IOKit + DiskArbitration.
//! - Linux backend (Phase 4.3) — udev.

use std::path::PathBuf;
use std::time::Duration;

pub mod mock;

pub use mock::MockWatcher;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceEvent {
    Connected {
        kind: DeviceKind,
        identity: DeviceIdentity,
        /// For drives: the mount point reported by the OS (e.g.
        /// `/Volumes/MATERIA`). For phones: `None` — the daemon mounts
        /// on demand via the bridge.
        mount_point: Option<PathBuf>,
    },
    Disconnected {
        identity: DeviceIdentity,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Phone,
    Drive,
}

/// What the OS tells us about a (dis)connected device. A subset of
/// [`crate::config::DeviceMatch`]; used to match real events against
/// the user's configured devices.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceIdentity {
    pub vendor_id: Option<String>,
    pub product_id: Option<String>,
    pub serial: Option<String>,
    pub volume_label: Option<String>,
    pub volume_uuid: Option<String>,
}

impl DeviceIdentity {
    /// Does this identity satisfy `device`'s matcher?
    ///
    /// Each matcher field that is `Some` must equal the corresponding
    /// identity field. An empty matcher (all `None`) matches nothing —
    /// otherwise hosts (which have no matcher) would match every USB
    /// event spuriously.
    pub fn matches(&self, device: &crate::config::Device) -> bool {
        let m = &device.matcher;
        let any_constraint = m.vendor_id.is_some()
            || m.product_id.is_some()
            || m.serial.is_some()
            || m.volume_label.is_some()
            || m.volume_uuid.is_some();
        if !any_constraint {
            return false;
        }
        macro_rules! constrain {
            ($field:ident) => {
                if let Some(expected) = &m.$field
                    && self.$field.as_deref() != Some(expected.as_str())
                {
                    return false;
                }
            };
        }
        constrain!(vendor_id);
        constrain!(product_id);
        constrain!(serial);
        constrain!(volume_label);
        constrain!(volume_uuid);
        true
    }
}

pub trait Watcher: Send {
    /// Poll for the next event. Blocks up to `timeout`; returns `None`
    /// if no event arrived before the deadline. Returning `None` lets
    /// the daemon loop check its stop signal between events.
    fn next_event(&mut self, timeout: Duration) -> Option<DeviceEvent>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Bridge, Device, DeviceMatch, DeviceType};

    fn make_phone() -> Device {
        Device {
            name: "jarvis".into(),
            device_type: DeviceType::Phone,
            bridge: Bridge::Mtp,
            matcher: DeviceMatch {
                vendor_id: Some("18d1".into()),
                product_id: Some("4ee7".into()),
                serial: Some("ABC123".into()),
                ..Default::default()
            },
            description: None,
        }
    }

    fn make_drive() -> Device {
        Device {
            name: "materia".into(),
            device_type: DeviceType::Drive,
            bridge: Bridge::Fs,
            matcher: DeviceMatch {
                volume_label: Some("MATERIA".into()),
                ..Default::default()
            },
            description: None,
        }
    }

    #[test]
    fn phone_match_with_all_fields() {
        let id = DeviceIdentity {
            vendor_id: Some("18d1".into()),
            product_id: Some("4ee7".into()),
            serial: Some("ABC123".into()),
            ..Default::default()
        };
        assert!(id.matches(&make_phone()));
    }

    #[test]
    fn phone_mismatch_on_serial() {
        let id = DeviceIdentity {
            vendor_id: Some("18d1".into()),
            product_id: Some("4ee7".into()),
            serial: Some("WRONG".into()),
            ..Default::default()
        };
        assert!(!id.matches(&make_phone()));
    }

    #[test]
    fn phone_missing_required_serial_doesnt_match() {
        let id = DeviceIdentity {
            vendor_id: Some("18d1".into()),
            product_id: Some("4ee7".into()),
            serial: None,
            ..Default::default()
        };
        assert!(!id.matches(&make_phone()));
    }

    #[test]
    fn drive_match_by_label() {
        let id = DeviceIdentity {
            volume_label: Some("MATERIA".into()),
            ..Default::default()
        };
        assert!(id.matches(&make_drive()));
    }

    #[test]
    fn drive_wrong_label() {
        let id = DeviceIdentity {
            volume_label: Some("OTHER".into()),
            ..Default::default()
        };
        assert!(!id.matches(&make_drive()));
    }

    #[test]
    fn empty_matcher_never_matches() {
        // A host device has no matcher fields — it must NOT match
        // arbitrary USB events.
        let host = Device {
            name: "tardis".into(),
            device_type: DeviceType::Host,
            bridge: Bridge::Fs,
            matcher: DeviceMatch::default(),
            description: None,
        };
        let id = DeviceIdentity {
            volume_label: Some("WHATEVER".into()),
            ..Default::default()
        };
        assert!(!id.matches(&host));
    }

    #[test]
    fn extra_unmatched_identity_fields_are_ignored() {
        // Drive matcher only constrains volume_label; an identity that
        // carries extra vendor_id should still match.
        let id = DeviceIdentity {
            vendor_id: Some("0000".into()),
            volume_label: Some("MATERIA".into()),
            ..Default::default()
        };
        assert!(id.matches(&make_drive()));
    }
}
