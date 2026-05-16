//! In-memory mock watcher for tests and CI.
//!
//! Events are pushed synchronously via [`MockWatcher::push`]; the
//! daemon loop pops them via [`Watcher::next_event`]. No OS plumbing,
//! no threads — deterministic and fast.

use std::collections::VecDeque;
use std::time::Duration;

use super::{DeviceEvent, Watcher};

#[derive(Debug, Default)]
pub struct MockWatcher {
    queue: VecDeque<DeviceEvent>,
}

impl MockWatcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueue an event that the next call to `next_event` will return.
    pub fn push(&mut self, event: DeviceEvent) {
        self.queue.push_back(event);
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

impl Watcher for MockWatcher {
    fn next_event(&mut self, _timeout: Duration) -> Option<DeviceEvent> {
        self.queue.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watcher::{DeviceIdentity, DeviceKind};

    fn drive_connected() -> DeviceEvent {
        DeviceEvent::Connected {
            kind: DeviceKind::Drive,
            identity: DeviceIdentity {
                volume_label: Some("MATERIA".into()),
                ..Default::default()
            },
            mount_point: Some("/Volumes/MATERIA".into()),
        }
    }

    fn drive_disconnected() -> DeviceEvent {
        DeviceEvent::Disconnected {
            identity: DeviceIdentity {
                volume_label: Some("MATERIA".into()),
                ..Default::default()
            },
        }
    }

    #[test]
    fn pops_events_fifo() {
        let mut w = MockWatcher::new();
        w.push(drive_connected());
        w.push(drive_disconnected());

        assert!(matches!(
            w.next_event(Duration::from_secs(0)).unwrap(),
            DeviceEvent::Connected { .. }
        ));
        assert!(matches!(
            w.next_event(Duration::from_secs(0)).unwrap(),
            DeviceEvent::Disconnected { .. }
        ));
        assert!(w.next_event(Duration::from_secs(0)).is_none());
    }

    #[test]
    fn empty_watcher_returns_none() {
        let mut w = MockWatcher::new();
        assert!(w.next_event(Duration::from_secs(0)).is_none());
        assert!(w.is_empty());
        assert_eq!(w.len(), 0);
    }

    #[test]
    fn len_reflects_pending_count() {
        let mut w = MockWatcher::new();
        assert_eq!(w.len(), 0);
        w.push(drive_connected());
        w.push(drive_disconnected());
        assert_eq!(w.len(), 2);
        w.next_event(Duration::from_secs(0));
        assert_eq!(w.len(), 1);
    }
}
