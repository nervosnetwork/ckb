use crate::component::sort_key::{AncestorsScoreSortKey, EvictKey};
use ckb_systemtime::unix_time_as_millis;
use ckb_types::{
    core::{
        Capacity, Cycle, FeeRate, TransactionView,
        cell::ResolvedTransaction,
        tx_pool::{TxEntryInfo, get_transaction_weight},
    },
    packed::{OutPoint, ProposalShortId},
};
use std::cmp::Ordering;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// An entry in the transaction pool.
#[derive(Debug, Clone, Eq)]
pub struct TxEntry {
    /// Transaction
    pub rtx: Arc<ResolvedTransaction>,
    /// Cycles
    pub cycles: Cycle,
    /// tx size
    pub size: usize,
    /// fee
    pub fee: Capacity,
    /// ancestors txs size
    pub ancestors_size: usize,
    /// ancestors txs fee
    pub ancestors_fee: Capacity,
    /// ancestors txs cycles
    pub ancestors_cycles: Cycle,
    /// ancestors txs count
    pub ancestors_count: usize,
    /// descendants txs fee
    pub descendants_fee: Capacity,
    /// descendants txs size
    pub descendants_size: usize,
    /// descendants txs cycles
    pub descendants_cycles: Cycle,
    /// descendants txs count
    pub descendants_count: usize,
    /// The unix timestamp when entering the Txpool, unit: Millisecond
    pub timestamp: u64,
}

/// Immutable callback payload detached from resolved-cell ownership.
///
/// A pool entry retains complete resolved inputs for DAO accounting and a
/// compact liveness-only dep representation. Stable-state callbacks need
/// neither; they only need the transaction and accounting snapshot.
/// Keeping this compact type in the effect journal prevents a stalled callback
/// from extending the lifetime of arbitrarily large resolved metadata after
/// the authoritative pool entry has been removed.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct TxEntrySnapshot {
    /// Transaction.
    pub transaction: TransactionView,
    /// Cycles.
    pub cycles: Cycle,
    /// Serialized transaction size.
    pub size: usize,
    /// Fee.
    pub fee: Capacity,
    /// Ancestor transaction size.
    pub ancestors_size: usize,
    /// Ancestor transaction fee.
    pub ancestors_fee: Capacity,
    /// Ancestor transaction cycles.
    pub ancestors_cycles: Cycle,
    /// Ancestor count.
    pub ancestors_count: usize,
    /// Descendant transaction fee.
    pub descendants_fee: Capacity,
    /// Descendant transaction size.
    pub descendants_size: usize,
    /// Descendant transaction cycles.
    pub descendants_cycles: Cycle,
    /// Descendant count.
    pub descendants_count: usize,
    /// Unix timestamp when the transaction entered the pool, in milliseconds.
    pub timestamp: u64,
}

impl TxEntrySnapshot {
    /// Return the immutable transaction view.
    pub fn transaction(&self) -> &TransactionView {
        &self.transaction
    }

    /// Convert the snapshot to the public entry-info representation.
    pub fn to_info(&self) -> TxEntryInfo {
        TxEntryInfo {
            cycles: self.cycles,
            size: self.size as u64,
            fee: self.fee,
            ancestors_size: self.ancestors_size as u64,
            ancestors_cycles: self.ancestors_cycles,
            descendants_size: self.descendants_size as u64,
            descendants_cycles: self.descendants_cycles,
            ancestors_count: self.ancestors_count as u64,
            timestamp: self.timestamp,
        }
    }
}

impl From<TxEntry> for TxEntrySnapshot {
    fn from(entry: TxEntry) -> Self {
        Self {
            transaction: entry.rtx.transaction.clone(),
            cycles: entry.cycles,
            size: entry.size,
            fee: entry.fee,
            ancestors_size: entry.ancestors_size,
            ancestors_fee: entry.ancestors_fee,
            ancestors_cycles: entry.ancestors_cycles,
            ancestors_count: entry.ancestors_count,
            descendants_fee: entry.descendants_fee,
            descendants_size: entry.descendants_size,
            descendants_cycles: entry.descendants_cycles,
            descendants_count: entry.descendants_count,
            timestamp: entry.timestamp,
        }
    }
}

impl TxEntry {
    /// Create new transaction pool entry
    pub fn new(rtx: Arc<ResolvedTransaction>, cycles: Cycle, fee: Capacity, size: usize) -> Self {
        Self::new_with_timestamp(rtx, cycles, fee, size, unix_time_as_millis())
    }

    /// Create new transaction pool entry with specified timestamp
    pub fn new_with_timestamp(
        rtx: Arc<ResolvedTransaction>,
        cycles: Cycle,
        fee: Capacity,
        size: usize,
        timestamp: u64,
    ) -> Self {
        TxEntry {
            rtx,
            cycles,
            size,
            fee,
            timestamp,
            ancestors_size: size,
            ancestors_fee: fee,
            ancestors_cycles: cycles,
            descendants_fee: fee,
            descendants_size: size,
            descendants_cycles: cycles,
            descendants_count: 1,
            ancestors_count: 1,
        }
    }

    /// Create dummy entry from tx, skip resolve
    pub fn dummy_resolve(tx: TransactionView, cycles: Cycle, fee: Capacity, size: usize) -> Self {
        let rtx = ResolvedTransaction::dummy_resolve(tx);
        TxEntry::new(Arc::new(rtx), cycles, fee, size)
    }

    /// Return related dep out_points
    pub fn related_dep_out_points(&self) -> impl Iterator<Item = &OutPoint> {
        self.rtx.related_dep_out_points()
    }

    /// Return reference of transaction
    pub fn transaction(&self) -> &TransactionView {
        &self.rtx.transaction
    }

    /// Converts a Entry into a TransactionView
    /// This consumes the Entry
    pub fn into_transaction(self) -> TransactionView {
        self.rtx.transaction.clone()
    }

    /// Return proposal_short_id of transaction
    pub fn proposal_short_id(&self) -> ProposalShortId {
        self.transaction().proposal_short_id()
    }

    /// Returns a sorted_key
    pub fn as_score_key(&self) -> AncestorsScoreSortKey {
        AncestorsScoreSortKey::from(self)
    }

    /// Returns a evict_key
    pub fn as_evict_key(&self) -> EvictKey {
        EvictKey::from(self)
    }

    /// Returns fee rate
    pub fn fee_rate(&self) -> FeeRate {
        let weight = get_transaction_weight(self.size, self.cycles);
        FeeRate::calculate(self.fee, weight)
    }

    /// Reset ancestor state by remove
    pub fn reset_statistic_state(&mut self) {
        self.ancestors_count = 1;
        self.ancestors_size = self.size;
        self.ancestors_cycles = self.cycles;
        self.ancestors_fee = self.fee;

        self.descendants_count = 1;
        self.descendants_size = self.size;
        self.descendants_cycles = self.cycles;
        self.descendants_fee = self.fee;
    }

    /// Converts entry to a `TxEntryInfo`.
    pub fn to_info(&self) -> TxEntryInfo {
        TxEntryInfo {
            cycles: self.cycles,
            size: self.size as u64,
            fee: self.fee,
            ancestors_size: self.ancestors_size as u64,
            ancestors_cycles: self.ancestors_cycles,
            descendants_size: self.descendants_size as u64,
            descendants_cycles: self.descendants_cycles,
            ancestors_count: self.ancestors_count as u64,
            timestamp: self.timestamp,
        }
    }
}

impl Hash for TxEntry {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Hash::hash(self.transaction(), state);
    }
}

impl PartialEq for TxEntry {
    fn eq(&self, other: &TxEntry) -> bool {
        self.rtx.transaction == other.rtx.transaction
    }
}

impl PartialOrd for TxEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TxEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_score_key().cmp(&other.as_score_key())
    }
}

impl From<&TxEntry> for AncestorsScoreSortKey {
    fn from(entry: &TxEntry) -> Self {
        let weight = get_transaction_weight(entry.size, entry.cycles);
        let ancestors_weight = get_transaction_weight(entry.ancestors_size, entry.ancestors_cycles);
        AncestorsScoreSortKey {
            fee: entry.fee,
            weight,
            ancestors_fee: entry.ancestors_fee,
            ancestors_weight,
        }
    }
}

impl From<&TxEntry> for EvictKey {
    fn from(entry: &TxEntry) -> Self {
        let weight = get_transaction_weight(entry.size, entry.cycles);
        let descendants_weight =
            get_transaction_weight(entry.descendants_size, entry.descendants_cycles);

        let descendants_feerate = FeeRate::calculate(entry.descendants_fee, descendants_weight);
        let feerate = FeeRate::calculate(entry.fee, weight);
        EvictKey {
            fee_rate: descendants_feerate.max(feerate),
            timestamp: entry.timestamp,
            descendants_count: entry.descendants_count,
            id: entry.proposal_short_id(),
        }
    }
}
