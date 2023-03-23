use crate::component::container::AncestorsScoreSortKey;
use ckb_systemtime::unix_time_as_millis;
use ckb_types::{
    core::{
        cell::ResolvedTransaction,
        tx_pool::{get_transaction_weight, TxEntryInfo},
        Capacity, Cycle, FeeRate, TransactionView,
    },
    packed::{OutPoint, ProposalShortId},
};
use std::cmp::Ordering;
use std::hash::{Hash, Hasher};

/// An entry in the transaction pool.
#[derive(Debug, Clone, Eq)]
pub struct TxEntry {
    /// Transaction
    pub rtx: ResolvedTransaction,
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
    /// The unix timestamp when entering the Txpool, unit: Millisecond
    pub timestamp: u64,
}

impl TxEntry {
    /// Create new transaction pool entry
    pub fn new(rtx: ResolvedTransaction, cycles: Cycle, fee: Capacity, size: usize) -> Self {
        Self::new_with_timestamp(rtx, cycles, fee, size, unix_time_as_millis())
    }

    /// Create new transaction pool entry with specified timestamp
    pub fn new_with_timestamp(
        rtx: ResolvedTransaction,
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
            ancestors_count: 1,
        }
    }

    /// Create dummy entry from tx, skip resolve
    pub fn dummy_resolve(tx: TransactionView, cycles: Cycle, fee: Capacity, size: usize) -> Self {
        let rtx = ResolvedTransaction::dummy_resolve(tx);
        TxEntry::new(rtx, cycles, fee, size)
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
        self.rtx.transaction
    }

    /// Return proposal_short_id of transaction
    pub fn proposal_short_id(&self) -> ProposalShortId {
        self.transaction().proposal_short_id()
    }

    /// Returns a sorted_key
    pub fn as_sorted_key(&self) -> AncestorsScoreSortKey {
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

    /// Update ancestor state for add an entry
    pub fn add_entry_weight(&mut self, entry: &TxEntry) {
        self.ancestors_count = self.ancestors_count.saturating_add(1);
        self.ancestors_size = self.ancestors_size.saturating_add(entry.size);
        self.ancestors_cycles = self.ancestors_cycles.saturating_add(entry.cycles);
        self.ancestors_fee = Capacity::shannons(
            self.ancestors_fee
                .as_u64()
                .saturating_add(entry.fee.as_u64()),
        );
    }
    /// Update ancestor state for remove an entry
    pub fn sub_entry_weight(&mut self, entry: &TxEntry) {
        self.ancestors_count = self.ancestors_count.saturating_sub(1);
        self.ancestors_size = self.ancestors_size.saturating_sub(entry.size);
        self.ancestors_cycles = self.ancestors_cycles.saturating_sub(entry.cycles);
        self.ancestors_fee = Capacity::shannons(
            self.ancestors_fee
                .as_u64()
                .saturating_sub(entry.fee.as_u64()),
        );
    }

    /// Reset ancestor state by remove
    pub fn reset_ancestors_state(&mut self) {
        self.ancestors_count = 1;
        self.ancestors_size = self.size;
        self.ancestors_cycles = self.cycles;
        self.ancestors_fee = self.fee;
    }

    /// Converts entry to a `TxEntryInfo`.
    pub fn to_info(&self) -> TxEntryInfo {
        TxEntryInfo {
            cycles: self.cycles,
            size: self.size as u64,
            fee: self.fee,
            ancestors_size: self.ancestors_size as u64,
            ancestors_cycles: self.ancestors_cycles,
            ancestors_count: self.ancestors_count as u64,
            timestamp: self.timestamp,
        }
    }
}

impl From<&TxEntry> for AncestorsScoreSortKey {
    fn from(entry: &TxEntry) -> Self {
        let weight = get_transaction_weight(entry.size, entry.cycles);
        let ancestors_weight = get_transaction_weight(entry.ancestors_size, entry.ancestors_cycles);
        AncestorsScoreSortKey {
            fee: entry.fee,
            weight,
            id: entry.proposal_short_id(),
            ancestors_fee: entry.ancestors_fee,
            ancestors_size: entry.ancestors_size,
            ancestors_weight,
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
        self.as_sorted_key().cmp(&other.as_sorted_key())
    }
}

/// Currently we do not have trace descendants,
/// so first take the simplest strategy,
/// first compare fee_rate, select the smallest fee_rate,
/// and then select the latest timestamp, for eviction,
/// the latest timestamp which also means that the fewer descendants may exist.
#[derive(Eq, PartialEq, Clone, Debug)]
pub struct EvictKey {
    fee_rate: FeeRate,
    timestamp: u64,
}

impl From<&TxEntry> for EvictKey {
    fn from(entry: &TxEntry) -> Self {
        EvictKey {
            fee_rate: entry.fee_rate(),
            timestamp: entry.timestamp,
        }
    }
}

impl PartialOrd for EvictKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for EvictKey {
    fn cmp(&self, other: &Self) -> Ordering {
        if self.fee_rate == other.fee_rate {
            self.timestamp.cmp(&other.timestamp).reverse()
        } else {
            self.fee_rate.cmp(&other.fee_rate)
        }
    }
}
