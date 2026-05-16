//! Sync engine.
//!
//! Stage 1 — [`diff`]: compare what the device currently holds (via its
//! [`Bridge`](crate::bridges::Bridge)) against the local
//! [`Manifest`](crate::manifest::Manifest) for a given binding, and
//! produce a list of [`FileChange`]s.
//!
//! Stage 2 — pairwise reconciliation: given two diffs (one per device)
//! plus their bindings (for role enforcement), produce a list of
//! [`SyncAction`]s describing the transfers and deletions needed to
//! bring both sides into agreement.

pub mod diff;
mod reconcile;

pub use diff::{Diff, FileChange, compute_diff};
pub use reconcile::{SyncAction, reconcile};
