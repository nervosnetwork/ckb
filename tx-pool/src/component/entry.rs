use crate::component::container::AncestorsScoreSortKey;
use crate::component::get_transaction_virtual_bytes;
use ckb_types::{
    core::{cell::ResolvedTransaction, tx_pool::TxEntryInfo, Capacity, Cycle, TransactionView},
    packed::{OutPoint, ProposalShortId},
};
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
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
    // /// related out points (cell deps includes cell group itself)
    // pub related_out_points: Vec<OutPoint>,
}

impl TxEntry {
    /// Create new transaction pool entry
    pub fn new(rtx: ResolvedTransaction, cycles: Cycle, fee: Capacity, size: usize) -> Self {
        TxEntry {
            rtx,
            cycles,
            size,
            fee,
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

    /// Return proposal_short_id of transaction
    pub fn proposal_short_id(&self) -> ProposalShortId {
        self.transaction().proposal_short_id()
    }

    /// Returns a sorted_key
    pub fn as_sorted_key(&self) -> AncestorsScoreSortKey {
        AncestorsScoreSortKey::from(self)
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

    /// Update ancestors to add it as a descendant transaction.
    pub fn add_ancestors_weight(&mut self, entry: &TxEntry) {
        self.ancestors_count = self.ancestors_count.saturating_add(entry.ancestors_count);
        self.ancestors_size = self.ancestors_size.saturating_add(entry.ancestors_size);
        self.ancestors_cycles = self.ancestors_cycles.saturating_add(entry.ancestors_cycles);
        self.ancestors_fee = Capacity::shannons(
            self.ancestors_fee
                .as_u64()
                .saturating_add(entry.ancestors_fee.as_u64()),
        );
    }
    /// Update ancestors to remove it.
    pub fn sub_ancestors_weight(&mut self, entry: &TxEntry) {
        self.ancestors_count = self.ancestors_count.saturating_sub(entry.ancestors_count);
        self.ancestors_size = self.ancestors_size.saturating_sub(entry.ancestors_size);
        self.ancestors_cycles = self.ancestors_cycles.saturating_sub(entry.ancestors_cycles);
        self.ancestors_fee = Capacity::shannons(
            self.ancestors_fee
                .as_u64()
                .saturating_sub(entry.ancestors_fee.as_u64()),
        );
    }

    /// Converts entry to a `TxEntryInfo`.
    pub fn to_info(&self) -> TxEntryInfo {
        TxEntryInfo {
            cycles: self.cycles,
            size: self.size as u64,
            fee: self.fee,
            ancestors_size: self.ancestors_size as u64,
            ancestors_cycles: self.ancestors_cycles as u64,
            ancestors_count: self.ancestors_count as u64,
        }
    }
}

impl From<&TxEntry> for AncestorsScoreSortKey {
    fn from(entry: &TxEntry) -> Self {
        let vbytes = get_transaction_virtual_bytes(entry.size, entry.cycles);
        let ancestors_vbytes =
            get_transaction_virtual_bytes(entry.ancestors_size, entry.ancestors_cycles);
        AncestorsScoreSortKey {
            fee: entry.fee,
            vbytes,
            id: entry.proposal_short_id(),
            ancestors_fee: entry.ancestors_fee,
            ancestors_size: entry.ancestors_size,
            ancestors_vbytes,
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

// A template data struct used to store modified entries when package txs
pub struct TxModifiedEntries {
    entries: HashMap<ProposalShortId, TxEntry>,
    sort_index: BTreeSet<AncestorsScoreSortKey>,
}

impl Default for TxModifiedEntries {
    fn default() -> Self {
        TxModifiedEntries {
            entries: HashMap::default(),
            sort_index: BTreeSet::default(),
        }
    }
}

impl TxModifiedEntries {
    pub fn with_sorted_by_score_iter<F, Ret>(&self, func: F) -> Ret
    where
        F: FnOnce(&mut dyn Iterator<Item = &TxEntry>) -> Ret,
    {
        let mut iter = self
            .sort_index
            .iter()
            .rev()
            .map(|key| self.entries.get(&key.id).expect("must be consistent"));
        func(&mut iter)
    }

    pub fn get(&self, id: &ProposalShortId) -> Option<&TxEntry> {
        self.entries.get(id)
    }

    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.entries.contains_key(id)
    }

    pub fn insert(&mut self, entry: TxEntry) {
        let key = AncestorsScoreSortKey::from(&entry);
        let short_id = entry.proposal_short_id();
        self.entries.insert(short_id, entry);
        self.sort_index.insert(key);
    }

    pub fn remove(&mut self, id: &ProposalShortId) -> Option<TxEntry> {
        self.entries.remove(id).map(|entry| {
            self.sort_index.remove(&(&entry).into());
            entry
        })
    }
}
