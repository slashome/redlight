//! Pairwise reconciliation: turn two per-side diffs into a list of
//! transfer / delete operations that brings both devices into agreement.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::config::{Binding, Role};

use super::diff::{Diff, FileChange};

/// A single concrete operation produced by reconciliation. The transfer
/// step (Phase 3.3) will execute these and update the involved manifests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncAction {
    /// Copy `path` from `from` to `to`. After execution, both manifests
    /// should record `(size, mtime)` for this path.
    Push {
        from: String,
        to: String,
        path: PathBuf,
        size: u64,
        mtime: i64,
    },
    /// Remove `path` from `device`. After execution, both manifests
    /// should clear the entry.
    Delete { device: String, path: PathBuf },
}

impl SyncAction {
    pub fn path(&self) -> &Path {
        match self {
            Self::Push { path, .. } | Self::Delete { path, .. } => path,
        }
    }
}

/// Reconcile two diffs into a sequence of [`SyncAction`]s.
///
/// **Conflict policy**
/// - Both sides modified → most recent `mtime` wins; the older copy is
///   overwritten.
/// - Modification vs deletion → modification wins (conservative: never
///   silently destroy a freshly-edited file).
/// - Both deleted → no-op.
///
/// **Role policy**
/// - A `read_only` binding contributes no intent: that device may
///   receive updates but never pushes outward. Its diff entries are
///   ignored.
///
/// **Caller's responsibility**: `diff_a` and `diff_b` must describe the
/// same item, and the bindings must correspond to that item on devices
/// `diff_a.device` / `diff_b.device` respectively.
pub fn reconcile(
    diff_a: &Diff,
    binding_a: &Binding,
    diff_b: &Diff,
    binding_b: &Binding,
) -> Vec<SyncAction> {
    let intents_a = collect_intents(diff_a, binding_a.role);
    let intents_b = collect_intents(diff_b, binding_b.role);

    let mut paths: BTreeSet<PathBuf> = BTreeSet::new();
    paths.extend(intents_a.keys().cloned());
    paths.extend(intents_b.keys().cloned());

    let mut actions = Vec::new();
    for path in paths {
        let a = intents_a.get(&path).copied();
        let b = intents_b.get(&path).copied();
        if let Some(action) = classify(a, b, &diff_a.device, &diff_b.device, &path) {
            actions.push(action);
        }
    }
    actions
}

fn collect_intents(diff: &Diff, role: Role) -> BTreeMap<PathBuf, &FileChange> {
    if role == Role::ReadOnly {
        BTreeMap::new()
    } else {
        diff.changes
            .iter()
            .map(|c| (c.path().to_path_buf(), c))
            .collect()
    }
}

fn classify(
    a: Option<&FileChange>,
    b: Option<&FileChange>,
    device_a: &str,
    device_b: &str,
    path: &Path,
) -> Option<SyncAction> {
    use FileChange::{Deleted, Modified, New};

    match (a, b) {
        (None, None) => None,

        // Only one side has anything to say.
        (Some(New { size, mtime, .. } | Modified { size, mtime, .. }), None) => {
            Some(push(device_a, device_b, path, *size, *mtime))
        }
        (None, Some(New { size, mtime, .. } | Modified { size, mtime, .. })) => {
            Some(push(device_b, device_a, path, *size, *mtime))
        }
        (Some(Deleted { .. }), None) => Some(delete(device_b, path)),
        (None, Some(Deleted { .. })) => Some(delete(device_a, path)),

        // Both deleted: nothing to do (caller's manifest update step will
        // also clean up, but no transfer is needed).
        (Some(Deleted { .. }), Some(Deleted { .. })) => None,

        // Modification vs deletion: modification wins.
        (Some(Deleted { .. }), Some(c_b)) => {
            let (sz, mt) = size_mtime(c_b);
            Some(push(device_b, device_a, path, sz, mt))
        }
        (Some(c_a), Some(Deleted { .. })) => {
            let (sz, mt) = size_mtime(c_a);
            Some(push(device_a, device_b, path, sz, mt))
        }

        // Both have content: most recent mtime wins.
        (Some(c_a), Some(c_b)) => {
            let (sz_a, mt_a) = size_mtime(c_a);
            let (sz_b, mt_b) = size_mtime(c_b);
            if mt_a >= mt_b {
                Some(push(device_a, device_b, path, sz_a, mt_a))
            } else {
                Some(push(device_b, device_a, path, sz_b, mt_b))
            }
        }
    }
}

fn push(from: &str, to: &str, path: &Path, size: u64, mtime: i64) -> SyncAction {
    SyncAction::Push {
        from: from.into(),
        to: to.into(),
        path: path.to_path_buf(),
        size,
        mtime,
    }
}

fn delete(device: &str, path: &Path) -> SyncAction {
    SyncAction::Delete {
        device: device.into(),
        path: path.to_path_buf(),
    }
}

fn size_mtime(c: &FileChange) -> (u64, i64) {
    match c {
        FileChange::New { size, mtime, .. } | FileChange::Modified { size, mtime, .. } => {
            (*size, *mtime)
        }
        FileChange::Deleted { .. } => (0, i64::MIN),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::{Binding, Role};

    // -- helpers --

    fn binding(device: &str, role: Role) -> Binding {
        Binding {
            item: "music".into(),
            device: device.into(),
            path: None,
            role,
        }
    }

    fn diff(device: &str, changes: Vec<FileChange>) -> Diff {
        Diff {
            item: "music".into(),
            device: device.into(),
            changes,
        }
    }

    fn new_(path: &str, size: u64, mtime: i64) -> FileChange {
        FileChange::New {
            path: path.into(),
            size,
            mtime,
        }
    }
    fn modified(path: &str, size: u64, mtime: i64) -> FileChange {
        FileChange::Modified {
            path: path.into(),
            size,
            mtime,
        }
    }
    fn del(path: &str) -> FileChange {
        FileChange::Deleted { path: path.into() }
    }

    fn run(a: Vec<FileChange>, role_a: Role, b: Vec<FileChange>, role_b: Role) -> Vec<SyncAction> {
        reconcile(
            &diff("a", a),
            &binding("a", role_a),
            &diff("b", b),
            &binding("b", role_b),
        )
    }

    // -- one side only --

    #[test]
    fn new_on_a_pushes_a_to_b() {
        let actions = run(
            vec![new_("x", 10, 100)],
            Role::ReadWrite,
            vec![],
            Role::ReadWrite,
        );
        assert_eq!(
            actions,
            vec![SyncAction::Push {
                from: "a".into(),
                to: "b".into(),
                path: "x".into(),
                size: 10,
                mtime: 100,
            }]
        );
    }

    #[test]
    fn modified_on_a_pushes_a_to_b() {
        let actions = run(
            vec![modified("x", 20, 200)],
            Role::ReadWrite,
            vec![],
            Role::ReadWrite,
        );
        assert!(matches!(
            actions[0],
            SyncAction::Push { ref from, ref to, .. } if from == "a" && to == "b"
        ));
    }

    #[test]
    fn deleted_on_a_propagates_delete_to_b() {
        let actions = run(vec![del("x")], Role::ReadWrite, vec![], Role::ReadWrite);
        assert_eq!(
            actions,
            vec![SyncAction::Delete {
                device: "b".into(),
                path: "x".into()
            }]
        );
    }

    #[test]
    fn new_on_b_pushes_b_to_a() {
        let actions = run(
            vec![],
            Role::ReadWrite,
            vec![new_("y", 5, 50)],
            Role::ReadWrite,
        );
        assert!(matches!(
            actions[0],
            SyncAction::Push { ref from, ref to, .. } if from == "b" && to == "a"
        ));
    }

    #[test]
    fn deleted_on_b_propagates_delete_to_a() {
        let actions = run(vec![], Role::ReadWrite, vec![del("y")], Role::ReadWrite);
        assert_eq!(
            actions,
            vec![SyncAction::Delete {
                device: "a".into(),
                path: "y".into()
            }]
        );
    }

    // -- both sides --

    #[test]
    fn both_modified_most_recent_wins_a() {
        let actions = run(
            vec![modified("x", 10, 200)],
            Role::ReadWrite,
            vec![modified("x", 9, 100)],
            Role::ReadWrite,
        );
        assert!(matches!(
            actions[0],
            SyncAction::Push { ref from, ref to, mtime: 200, .. } if from == "a" && to == "b"
        ));
    }

    #[test]
    fn both_modified_most_recent_wins_b() {
        let actions = run(
            vec![modified("x", 10, 100)],
            Role::ReadWrite,
            vec![modified("x", 9, 200)],
            Role::ReadWrite,
        );
        assert!(matches!(
            actions[0],
            SyncAction::Push { ref from, ref to, mtime: 200, .. } if from == "b" && to == "a"
        ));
    }

    #[test]
    fn both_deleted_yields_no_action() {
        let actions = run(
            vec![del("x")],
            Role::ReadWrite,
            vec![del("x")],
            Role::ReadWrite,
        );
        assert!(actions.is_empty());
    }

    #[test]
    fn modified_beats_deleted_a_wins() {
        let actions = run(
            vec![modified("x", 10, 100)],
            Role::ReadWrite,
            vec![del("x")],
            Role::ReadWrite,
        );
        assert!(matches!(
            actions[0],
            SyncAction::Push { ref from, ref to, .. } if from == "a" && to == "b"
        ));
    }

    #[test]
    fn modified_beats_deleted_b_wins() {
        let actions = run(
            vec![del("x")],
            Role::ReadWrite,
            vec![modified("x", 10, 100)],
            Role::ReadWrite,
        );
        assert!(matches!(
            actions[0],
            SyncAction::Push { ref from, ref to, .. } if from == "b" && to == "a"
        ));
    }

    #[test]
    fn new_beats_deleted() {
        let actions = run(
            vec![new_("x", 10, 100)],
            Role::ReadWrite,
            vec![del("x")],
            Role::ReadWrite,
        );
        assert!(matches!(
            actions[0],
            SyncAction::Push { ref from, ref to, .. } if from == "a" && to == "b"
        ));
    }

    // -- roles --

    #[test]
    fn read_only_a_suppresses_a_push() {
        // A would push, but it's read-only → no action.
        let actions = run(
            vec![new_("x", 10, 100)],
            Role::ReadOnly,
            vec![],
            Role::ReadWrite,
        );
        assert!(actions.is_empty());
    }

    #[test]
    fn read_only_a_still_receives_from_b() {
        // B pushes new file, A is read-only (can receive) → push B→A.
        let actions = run(
            vec![],
            Role::ReadOnly,
            vec![new_("x", 10, 100)],
            Role::ReadWrite,
        );
        assert!(matches!(
            actions[0],
            SyncAction::Push { ref from, ref to, .. } if from == "b" && to == "a"
        ));
    }

    #[test]
    fn read_only_b_suppresses_b_push() {
        let actions = run(
            vec![],
            Role::ReadWrite,
            vec![new_("x", 10, 100)],
            Role::ReadOnly,
        );
        assert!(actions.is_empty());
    }

    #[test]
    fn both_read_only_no_action() {
        // Even if both sides have intents, neither can push.
        let actions = run(
            vec![new_("x", 10, 100)],
            Role::ReadOnly,
            vec![new_("x", 10, 100)],
            Role::ReadOnly,
        );
        assert!(actions.is_empty());
    }

    #[test]
    fn read_only_a_with_conflict_b_wins_automatically() {
        // A modified locally (suppressed), B modified → B's version
        // pushes to A; A's local change is discarded silently.
        let actions = run(
            vec![modified("x", 10, 999)],
            Role::ReadOnly,
            vec![modified("x", 10, 100)],
            Role::ReadWrite,
        );
        assert!(matches!(
            actions[0],
            SyncAction::Push { ref from, ref to, mtime: 100, .. } if from == "b" && to == "a"
        ));
    }

    // -- ordering & multiple paths --

    #[test]
    fn actions_sorted_by_path() {
        let actions = run(
            vec![new_("z.mp3", 1, 1), new_("a.mp3", 1, 1)],
            Role::ReadWrite,
            vec![],
            Role::ReadWrite,
        );
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].path(), Path::new("a.mp3"));
        assert_eq!(actions[1].path(), Path::new("z.mp3"));
    }

    #[test]
    fn empty_both_sides_no_actions() {
        let actions = run(vec![], Role::ReadWrite, vec![], Role::ReadWrite);
        assert!(actions.is_empty());
    }
}
