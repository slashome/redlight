//! Sync engine.
//!
//! Stage 1 (this phase) — [`diff`]: compare what the device currently
//! holds (via its [`Bridge`](crate::bridges::Bridge)) against the local
//! [`Manifest`](crate::manifest::Manifest) for a given binding, and
//! produce a list of [`FileChange`]s.

pub mod diff;

pub use diff::{Diff, FileChange, compute_diff};
