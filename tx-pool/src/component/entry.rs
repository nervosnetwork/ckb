use crate::component::sort_key::{AncestorsScoreSortKey, EvictKey};
use ckb_systemtime::unix_time_as_millis;
use ckb_types::{
    core::{
        Capacity, Cycle, FeeRate, TransactionView,
        cell::ResolvedTransaction,
        tx_pool::{TxEntryInfo, get_transaction_weight},
    },
    packed::{OutPoint, ProposalShortId},
    prelude::Entity,
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
    /// These totals are bounded by pool and consensus limits. Checked
    /// arithmetic is nevertheless deliberate: a future bound regression or
    /// duplicate graph edge must fail at the invariant boundary rather than
    /// silently changing package/eviction order.
    pub(crate) fn add_entry(&mut self, entry: &TxEntry) {
        self.count = self
            .count
            .checked_add(1)
            .expect("tx-pool weight count cannot overflow");
        self.size = self
            .size
            .checked_add(entry.size)
            .expect("tx-pool weight size cannot overflow");
        self.cycles = self
            .cycles
            .checked_add(entry.cycles)
            .expect("tx-pool weight cycles cannot overflow");
        self.fee = self
            .fee
            .checked_add(entry.fee.as_u64())
            .expect("tx-pool weight fee cannot overflow");
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
    /// Conservative bytes retained by this accepted entry, including its
    /// resolved-cell payload. This is distinct from serialized tx `size`.
    pub(crate) resident_size: usize,
}

/// Conservative resident-byte charge for a resolved transaction.
///
/// This counts logical ownership, so shared `Bytes` are charged to each entry
/// that can independently extend their lifetime. Saturation turns impossible
/// arithmetic overflow into a value that every finite residency budget
/// rejects, rather than wrapping into an undercharge. Before verification this
/// includes complete dep expansion; accepted entries carry the compact
/// verified representation produced by `ResolvedTx::into_pool_candidate`.
pub(crate) fn resolved_transaction_charge_bytes(
    tx_size: usize,
    rtx: &ResolvedTransaction,
) -> usize {
    let mut bytes = std::mem::size_of::<TxEntry>()
        .saturating_add(std::mem::size_of::<ResolvedTransaction>())
        .saturating_add(tx_size)
        // `TransactionView` retains raw and witness hashes outside the packed
        // transaction backing bytes counted by `tx_size`.
        .saturating_add(64);

    for cells in [
        &rtx.resolved_inputs,
        &rtx.resolved_cell_deps,
        &rtx.resolved_dep_groups,
    ] {
        bytes = bytes.saturating_add(
            cells
                .capacity()
                .saturating_mul(std::mem::size_of::<ckb_types::core::cell::CellMeta>()),
        );
        for cell in cells {
            bytes = bytes
                .saturating_add(cell.cell_output.as_slice().len())
                .saturating_add(cell.out_point.as_slice().len())
                .saturating_add(cell.transaction_info.as_ref().map_or(0, |_| 32))
                .saturating_add(cell.mem_cell_data.as_ref().map_or(0, |data| data.len()))
                .saturating_add(cell.mem_cell_data_hash.as_ref().map_or(0, |_| 32));
        }
    }
    bytes
}

// Accepted entries live in several allocator-backed indexes in addition to
// retaining their `ResolvedTransaction`. These charges are deliberately
// conservative accounting weights rather than allocator-specific byte-exact
// measurements: each value covers the record, hash-table slack and the graph
// membership(s) created by one logical item on 64-bit targets. Keeping the
// weights explicit makes the admission budget independent of the current
// `HashMap`/`multi_index_map` implementation and prevents a compact dep-group
// from becoming an uncharged index-amplification vector.
const ACCEPTED_ENTRY_INDEX_BASE_CHARGE: usize = 1_024;
const ACCEPTED_INPUT_INDEX_CHARGE: usize = 256;
const ACCEPTED_DEP_INDEX_CHARGE: usize = 384;
const ACCEPTED_HEADER_INDEX_CHARGE: usize = 128;

/// Conservative resident-byte charge after a transaction becomes accepted.
///
/// Besides the compact verified resolved payload, an accepted entry owns four
/// `PoolEntry` indexes, an out-point index and a bidirectional dependency
/// graph node. Every input can create one spend-index record and one graph
/// relation; every expanded dep can create an out-point/set record and one
/// graph relation; header deps are retained in their own index. Counts use the
/// actual expanded resolved deps, so a tiny serialized dep-group reference is
/// charged for the persistent fanout it creates.
pub(crate) fn accepted_transaction_charge_bytes(
    tx_size: usize,
    rtx: &ResolvedTransaction,
) -> usize {
    let input_count = rtx.transaction.inputs().len();
    let dep_count = rtx.related_dep_out_points().count();
    let header_count = rtx.transaction.header_deps().len();

    resolved_transaction_charge_bytes(tx_size, rtx)
        .saturating_add(ACCEPTED_ENTRY_INDEX_BASE_CHARGE)
        .saturating_add(input_count.saturating_mul(ACCEPTED_INPUT_INDEX_CHARGE))
        .saturating_add(dep_count.saturating_mul(ACCEPTED_DEP_INDEX_CHARGE))
        .saturating_add(header_count.saturating_mul(ACCEPTED_HEADER_INDEX_CHARGE))
}

/// Immutable callback payload detached from resolved-cell ownership.
///
/// A pool entry retains complete resolved inputs for DAO accounting and a
/// compact liveness-only dep representation. Stable-state callbacks need
/// neither; they only need the transaction and accounting snapshot.
/// Keeping this compact type in the effect outbox prevents a stalled callback
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

    /// Conservative retained bytes for one queued callback snapshot.
    pub(crate) fn charge_bytes(&self) -> usize {
        // `serialized_size_in_block` covers the packed transaction backing
        // bytes. The two cached hashes own another 32 bytes each; `size_of`
        // covers the view handles and all scalar accounting fields.
        std::mem::size_of::<Self>()
            .saturating_add(self.transaction.data().serialized_size_in_block())
            .saturating_add(64)
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
        let resident_size = accepted_transaction_charge_bytes(size, &rtx);
        Self::new_with_timestamp_and_resident_size(
            rtx,
            cycles,
            fee,
            size,
            unix_time_as_millis(),
            resident_size,
        )
    }

    /// Create new transaction pool entry with specified timestamp
    pub fn new_with_timestamp(
        rtx: Arc<ResolvedTransaction>,
        cycles: Cycle,
        fee: Capacity,
        size: usize,
        timestamp: u64,
    ) -> Self {
        let resident_size = accepted_transaction_charge_bytes(size, &rtx);
        Self::new_with_timestamp_and_resident_size(rtx, cycles, fee, size, timestamp, resident_size)
    }

    /// Create an entry with a residency charge already computed at resolve.
    pub(crate) fn new_with_resident_size(
        rtx: Arc<ResolvedTransaction>,
        cycles: Cycle,
        fee: Capacity,
        size: usize,
        resident_size: usize,
    ) -> Self {
        Self::new_with_timestamp_and_resident_size(
            rtx,
            cycles,
            fee,
            size,
            unix_time_as_millis(),
            resident_size,
        )
    }

    fn new_with_timestamp_and_resident_size(
        rtx: Arc<ResolvedTransaction>,
        cycles: Cycle,
        fee: Capacity,
        size: usize,
        timestamp: u64,
        resident_size: usize,
    ) -> Self {
        TxEntry {
            rtx,
            cycles,
            size,
            fee,
            timestamp,
            resident_size,
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

    /// Return the conservative accepted-pool residency charge.
    pub(crate) fn resident_size(&self) -> usize {
        self.resident_size
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

    fn apply_ancestor_delta(&mut self, delta: WeightDelta, add: bool) {
        if add {
            self.ancestors_count = self
                .ancestors_count
                .checked_add(delta.count)
                .expect("ancestor count cannot overflow");
            self.ancestors_size = self
                .ancestors_size
                .checked_add(delta.size)
                .expect("ancestor size cannot overflow");
            self.ancestors_cycles = self
                .ancestors_cycles
                .checked_add(delta.cycles)
                .expect("ancestor cycles cannot overflow");
            self.ancestors_fee = Capacity::shannons(
                self.ancestors_fee
                    .as_u64()
                    .checked_add(delta.fee)
                    .expect("ancestor fee cannot overflow"),
            );
        } else {
            self.ancestors_count = self
                .ancestors_count
                .checked_sub(delta.count)
                .expect("ancestor count cannot underflow");
            self.ancestors_size = self
                .ancestors_size
                .checked_sub(delta.size)
                .expect("ancestor size cannot underflow");
            self.ancestors_cycles = self
                .ancestors_cycles
                .checked_sub(delta.cycles)
                .expect("ancestor cycles cannot underflow");
            self.ancestors_fee = Capacity::shannons(
                self.ancestors_fee
                    .as_u64()
                    .checked_sub(delta.fee)
                    .expect("ancestor fee cannot underflow"),
            );
        }
    }

    fn apply_descendant_delta(&mut self, delta: WeightDelta, add: bool) {
        if add {
            self.descendants_count = self
                .descendants_count
                .checked_add(delta.count)
                .expect("descendant count cannot overflow");
            self.descendants_size = self
                .descendants_size
                .checked_add(delta.size)
                .expect("descendant size cannot overflow");
            self.descendants_cycles = self
                .descendants_cycles
                .checked_add(delta.cycles)
                .expect("descendant cycles cannot overflow");
            self.descendants_fee = Capacity::shannons(
                self.descendants_fee
                    .as_u64()
                    .checked_add(delta.fee)
                    .expect("descendant fee cannot overflow"),
            );
        } else {
            self.descendants_count = self
                .descendants_count
                .checked_sub(delta.count)
                .expect("descendant count cannot underflow");
            self.descendants_size = self
                .descendants_size
                .checked_sub(delta.size)
                .expect("descendant size cannot underflow");
            self.descendants_cycles = self
                .descendants_cycles
                .checked_sub(delta.cycles)
                .expect("descendant cycles cannot underflow");
            self.descendants_fee = Capacity::shannons(
                self.descendants_fee
                    .as_u64()
                    .checked_sub(delta.fee)
                    .expect("descendant fee cannot underflow"),
            );
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
            id: entry.proposal_short_id(),
        }
    }
}
