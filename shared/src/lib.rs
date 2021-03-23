//! TODO(doc): @quake

// num_cpus is used in proc_macro
pub mod shared;
pub mod shared_builder;

pub use ckb_snapshot::{Snapshot, SnapshotMgr};
use p2p::SessionId;
pub use shared::Shared;
pub use shared_builder::{SharedBuilder, SharedPackage};

/// Alias session id
pub(crate) type PeerIndex = SessionId;
