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

/// A signed delta used to update ancestor/descendant weight statistics.
#[derive(Clone, Copy, Default)]
pub(crate) struct WeightDelta {
    count: usize,
    size: usize,
    cycles: Cycle,
    fee: u64,
}

impl WeightDelta {
    fn from_entry(entry: &TxEntry) -> Self {
        Self {
            count: 1,
            size: entry.size,
            cycles: entry.cycles,
            fee: entry.fee.as_u64(),
        }
    }

    /// Accumulate another entry's weight into this delta, so a whole set of
    /// entries can be applied with a single `apply_*_delta` call.
    ///
    /// Plain (non-saturating) addition is safe here: the totals are bounded
    /// by the pool's own limits (counts by `max_ancestors_count`, sizes and
    /// cycles by block limits, fees by total issuance), so the sum cannot
    /// overflow. Applying the aggregated delta is equivalent to applying
    /// each entry's delta in sequence: all arithmetic downstream is
    /// saturating, and both forms clamp at zero on over-subtraction.
    pub(crate) fn add_entry(&mut self, entry: &TxEntry) {
        self.count += 1;
        self.size += entry.size;
        self.cycles += entry.cycles;
        self.fee += entry.fee.as_u64();
    }
}

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

    /// Update ancestor state for add an entry
    pub fn add_descendant_weight(&mut self, entry: &TxEntry) {
        self.apply_descendant_delta(WeightDelta::from_entry(entry), true);
    }

    /// Update ancestor state for remove an entry
    pub fn sub_descendant_weight(&mut self, entry: &TxEntry) {
        self.apply_descendant_delta(WeightDelta::from_entry(entry), false);
    }

    /// Update ancestor state for add an entry
    pub fn add_ancestor_weight(&mut self, entry: &TxEntry) {
        self.apply_ancestor_delta(WeightDelta::from_entry(entry), true);
    }

    /// Update ancestor state for remove an entry
    pub fn sub_ancestor_weight(&mut self, entry: &TxEntry) {
        self.apply_ancestor_delta(WeightDelta::from_entry(entry), false);
    }

    /// Update ancestor state for removing several entries at once.
    ///
    /// Equivalent to calling `sub_ancestor_weight` for each of them (see
    /// [`WeightDelta::add_entry`]); used by the block-template selector to
    /// adjust shared descendants with one aggregate subtraction instead of
    /// one subtraction per committed ancestor.
    pub(crate) fn sub_ancestors_weight(&mut self, delta: WeightDelta) {
        self.apply_ancestor_delta(delta, false);
    }

    /// Update descendant state for adding several entries at once.
    ///
    /// Equivalent to calling `add_descendant_weight` for each of them (see
    /// [`WeightDelta::add_entry`]); used when linking an entry to children
    /// that entered the pool before it.
    pub(crate) fn add_descendants_weight(&mut self, delta: WeightDelta) {
        self.apply_descendant_delta(delta, true);
    }

    fn apply_ancestor_delta(&mut self, delta: WeightDelta, add: bool) {
        if add {
            self.ancestors_count = self.ancestors_count.saturating_add(delta.count);
            self.ancestors_size = self.ancestors_size.saturating_add(delta.size);
            self.ancestors_cycles = self.ancestors_cycles.saturating_add(delta.cycles);
            self.ancestors_fee =
                Capacity::shannons(self.ancestors_fee.as_u64().saturating_add(delta.fee));
        } else {
            self.ancestors_count = self.ancestors_count.saturating_sub(delta.count);
            self.ancestors_size = self.ancestors_size.saturating_sub(delta.size);
            self.ancestors_cycles = self.ancestors_cycles.saturating_sub(delta.cycles);
            self.ancestors_fee =
                Capacity::shannons(self.ancestors_fee.as_u64().saturating_sub(delta.fee));
        }
    }

    fn apply_descendant_delta(&mut self, delta: WeightDelta, add: bool) {
        if add {
            self.descendants_count = self.descendants_count.saturating_add(delta.count);
            self.descendants_size = self.descendants_size.saturating_add(delta.size);
            self.descendants_cycles = self.descendants_cycles.saturating_add(delta.cycles);
            self.descendants_fee =
                Capacity::shannons(self.descendants_fee.as_u64().saturating_add(delta.fee));
        } else {
            self.descendants_count = self.descendants_count.saturating_sub(delta.count);
            self.descendants_size = self.descendants_size.saturating_sub(delta.size);
            self.descendants_cycles = self.descendants_cycles.saturating_sub(delta.cycles);
            self.descendants_fee =
                Capacity::shannons(self.descendants_fee.as_u64().saturating_sub(delta.fee));
        }
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
        }
    }
}
