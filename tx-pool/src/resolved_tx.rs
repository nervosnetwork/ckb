//! Types for the tx-pool resolve stage.
//!
//! The resolve stage turns a raw [`TransactionView`] into a [`ResolvedTx`] by
//! resolving inputs/cell_deps against the current chain snapshot and the
//! in-pool cell overlay.  It runs as a single ordered worker so that dependent
//! transactions are resolved in the order they arrive.

use crate::component::pool_map::Status;
use crate::tx_source::TxSource;
use ckb_snapshot::Snapshot;
use ckb_types::{
    core::{Capacity, TransactionView, cell::ResolvedTransaction},
    packed::Byte32,
};
use ckb_verification::cache::Completed;
use std::sync::Arc;

/// A job submitted to the resolve queue.
#[derive(Debug, Clone)]
pub struct ResolveJob {
    /// The raw transaction to resolve.
    pub tx: TransactionView,
    /// The origin of the transaction (remote, local, or proposal notification).
    pub source: TxSource,
    /// Pipeline generation in which this job was admitted.
    pub epoch: u64,
}

impl ResolveJob {
    /// Create a new resolve job in an explicit pipeline generation.
    pub(crate) fn new_at(tx: TransactionView, source: TxSource, epoch: u64) -> Self {
        Self { tx, source, epoch }
    }
}

/// A transaction that has been resolved and is ready for verification.
#[derive(Debug, Clone)]
pub struct ResolvedTx {
    /// The raw transaction.
    pub tx: TransactionView,
    /// The resolved transaction.
    pub rtx: Arc<ResolvedTransaction>,
    /// Pool status derived at resolve time.
    pub status: Status,
    /// Transaction fee calculated at resolve time.
    pub fee: Capacity,
    /// Serialized transaction size.
    pub tx_size: usize,
    /// Tip hash at resolve time; used to detect snapshot drift before submit.
    pub pre_resolve_tip: Byte32,
    /// Snapshot used during resolve; reused for verification.
    pub snapshot: Arc<Snapshot>,
    /// The origin of the transaction (remote, local, or proposal notification).
    pub source: TxSource,
    /// Pipeline generation inherited from the resolve job.
    pub(crate) epoch: u64,
    /// Completed script verification carried by authoritative RBF ownership.
    /// A `RaceLost` restore can therefore reuse the result even if the
    /// best-effort global cache-update channel was saturated.
    pub(crate) verified: Option<Completed>,
}

impl PartialEq for ResolvedTx {
    fn eq(&self, other: &Self) -> bool {
        self.tx == other.tx
            && self.rtx == other.rtx
            && self.status == other.status
            && self.fee == other.fee
            && self.tx_size == other.tx_size
            && self.pre_resolve_tip == other.pre_resolve_tip
            && self.snapshot == other.snapshot
            && self.source == other.source
            && self.epoch == other.epoch
            && self.verified == other.verified
    }
}
