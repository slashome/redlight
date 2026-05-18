//! Compose multiple watchers behind the same trait.
//!
//! The daemon polls a single [`Watcher`]; in v0.0.1 we have two
//! polling-based sources (filesystem mounts via [`super::PollWatcher`],
//! ADB devices via [`super::AdbPollWatcher`]). [`MultiWatcher`] folds
//! them together so the daemon doesn't need to know how many sources
//! are active.
//!
//! `next_event` round-robins through sub-watchers with the timeout
//! split evenly. First sub-watcher to produce an event wins; if all
//! return `None`, the call returns `None` after at most `timeout`
//! elapsed.

use std::time::Duration;

use super::{DeviceEvent, Watcher};

pub struct MultiWatcher {
    watchers: Vec<Box<dyn Watcher>>,
}

impl MultiWatcher {
    pub fn new() -> Self {
        Self {
            watchers: Vec::new(),
        }
    }

    /// Builder-style: add a sub-watcher.
    pub fn with(mut self, watcher: Box<dyn Watcher>) -> Self {
        self.watchers.push(watcher);
        self
    }

    /// Mutating: add a sub-watcher.
    pub fn add(&mut self, watcher: Box<dyn Watcher>) {
        self.watchers.push(watcher);
    }

    pub fn len(&self) -> usize {
        self.watchers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.watchers.is_empty()
    }
}

impl Default for MultiWatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Watcher for MultiWatcher {
    fn next_event(&mut self, timeout: Duration) -> Option<DeviceEvent> {
        if self.watchers.is_empty() {
            std::thread::sleep(timeout);
            return None;
        }
        let per_watcher = timeout / (self.watchers.len() as u32);
        for w in &mut self.watchers {
            if let Some(e) = w.next_event(per_watcher) {
                return Some(e);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watcher::{DeviceIdentity, DeviceKind, MockWatcher};

    fn evt(label: &str) -> DeviceEvent {
        DeviceEvent::Connected {
            kind: DeviceKind::Drive,
            identity: DeviceIdentity {
                volume_label: Some(label.into()),
                ..Default::default()
            },
            mount_point: None,
        }
    }

    #[test]
    fn empty_multi_returns_none() {
        let mut m = MultiWatcher::new();
        assert!(m.next_event(Duration::from_millis(0)).is_none());
        assert_eq!(m.len(), 0);
        assert!(m.is_empty());
    }

    #[test]
    fn events_from_first_watcher_are_drained_before_second() {
        let mut a = MockWatcher::new();
        a.push(evt("A1"));
        a.push(evt("A2"));
        let mut b = MockWatcher::new();
        b.push(evt("B1"));

        let mut m = MultiWatcher::new().with(Box::new(a)).with(Box::new(b));

        // First call drains the first event from A.
        match m.next_event(Duration::from_millis(0)) {
            Some(DeviceEvent::Connected { identity, .. }) => {
                assert_eq!(identity.volume_label.as_deref(), Some("A1"));
            }
            _ => panic!("expected event"),
        }
        // Second call: A still has A2 (we round-robin, A first).
        match m.next_event(Duration::from_millis(0)) {
            Some(DeviceEvent::Connected { identity, .. }) => {
                assert_eq!(identity.volume_label.as_deref(), Some("A2"));
            }
            _ => panic!("expected event"),
        }
        // Third call: A empty, B yields B1.
        match m.next_event(Duration::from_millis(0)) {
            Some(DeviceEvent::Connected { identity, .. }) => {
                assert_eq!(identity.volume_label.as_deref(), Some("B1"));
            }
            _ => panic!("expected event"),
        }
        // Everything drained.
        assert!(m.next_event(Duration::from_millis(0)).is_none());
    }

    #[test]
    fn skips_to_next_watcher_when_first_is_empty() {
        let a = MockWatcher::new(); // empty
        let mut b = MockWatcher::new();
        b.push(evt("B1"));

        let mut m = MultiWatcher::new().with(Box::new(a)).with(Box::new(b));

        match m.next_event(Duration::from_millis(0)) {
            Some(DeviceEvent::Connected { identity, .. }) => {
                assert_eq!(identity.volume_label.as_deref(), Some("B1"));
            }
            _ => panic!("expected event"),
        }
    }
}
