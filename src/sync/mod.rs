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
//!
//! Stage 3 — transfer execution: run each [`SyncAction`] through the
//! corresponding bridges, hash transferred content, and update both
//! manifests in-memory. Errors per action are surfaced in a
//! [`TransferReport`] so a single bad file doesn't abort the run.

pub mod diff;
mod reconcile;
mod transfer;

pub use diff::{Diff, FileChange, compute_diff};
pub use reconcile::{SyncAction, reconcile};
pub use transfer::{SideRef, TransferReport, execute_action, execute_actions};
