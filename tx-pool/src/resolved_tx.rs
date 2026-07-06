//! Types for the tx-pool resolve stage.
//!
//! The resolve stage turns a raw [`TransactionView`] into a [`ResolvedTx`] by
//! resolving inputs/cell_deps against the current chain snapshot and the
//! in-pool cell overlay.  It runs as a single ordered worker so that dependent
//! transactions are resolved in the order they arrive.

use crate::process::TxStatus;
use ckb_network::PeerIndex;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{Capacity, Cycle, TransactionView, cell::ResolvedTransaction},
    packed::Byte32,
};
use std::sync::Arc;

/// A job submitted to the resolve queue.
#[derive(Debug, Clone)]
pub struct ResolveJob {
    /// The raw transaction to resolve.
    pub tx: TransactionView,
    /// Declared cycles and source peer for remote transactions.
    pub remote: Option<(Cycle, PeerIndex)>,
    /// Whether this transaction came from a block proposal.
    pub is_proposal_tx: bool,
    /// Number of times this local transaction has been retried because its
    /// inputs were not yet available. Used to bound retries for orphans that
    /// are not satisfiable.
    pub attempts: u8,
}

/// A transaction that has been resolved and is ready for verification.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTx {
    /// The raw transaction.
    pub tx: TransactionView,
    /// The resolved transaction.
    pub rtx: Arc<ResolvedTransaction>,
    /// Pool status derived at resolve time.
    pub status: TxStatus,
    /// Transaction fee calculated at resolve time.
    pub fee: Capacity,
    /// Serialized transaction size.
    pub tx_size: usize,
    /// Tip hash at resolve time; used to detect snapshot drift before submit.
    pub pre_resolve_tip: Byte32,
    /// Snapshot used during resolve; reused for verification.
    pub snapshot: Arc<Snapshot>,
    /// Declared cycles and source peer for remote transactions.
    pub remote: Option<(Cycle, PeerIndex)>,
    /// Whether this transaction came from a block proposal and should be
    /// prioritized in the verify queue.
    pub is_proposal_tx: bool,
}
