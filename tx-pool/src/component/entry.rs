use crate::component::container::AncestorsScoreSortKey;
use crate::component::get_transaction_virtual_bytes;
use ckb_types::{
    core::{Capacity, Cycle, TransactionView},
    packed::{OutPoint, ProposalShortId},
};
use ckb_verification::cache::CacheEntry;
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::hash::{Hash, Hasher};

/// An defect entry (conflict or orphan) in the transaction pool.
#[derive(Debug, Clone)]
pub struct DefectEntry {
    /// Transaction
    pub transaction: TransactionView,
    /// refs count
    pub refs_count: usize,
    /// Cycles and fee
    pub cache_entry: Option<CacheEntry>,
    /// tx size
    pub size: usize,
    // timestamp
    pub timestamp: u64,
}

impl DefectEntry {
    /// Create new transaction pool entry
    pub fn new(
        tx: TransactionView,
        refs_count: usize,
        cache_entry: Option<CacheEntry>,
        size: usize,
    ) -> DefectEntry {
        DefectEntry {
            transaction: tx,
            refs_count,
            cache_entry,
            size,
            timestamp: faketime::unix_time().as_secs(),
        }
    }
}

/// An entry in the transaction pool.
#[derive(Debug, Clone, Eq)]
pub struct TxEntry {
    /// Transaction
    pub transaction: TransactionView,
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
    /// related out points (cell deps includes cell group itself)
    pub related_out_points: Vec<OutPoint>,
}

impl TxEntry {
    /// Create new transaction pool entry
    pub fn new(
        tx: TransactionView,
        cycles: Cycle,
        fee: Capacity,
        size: usize,
        related_out_points: Vec<OutPoint>,
    ) -> Self {
        TxEntry {
            transaction: tx,
            cycles,
            size,
            fee,
            ancestors_size: size,
            ancestors_fee: fee,
            ancestors_cycles: cycles,
            ancestors_count: 1,
            related_out_points,
        }
    }

    /// TODO(doc): @zhangsoledad
    pub fn as_sorted_key(&self) -> AncestorsScoreSortKey {
        AncestorsScoreSortKey::from(self)
    }

    /// TODO(doc): @zhangsoledad
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
    /// TODO(doc): @zhangsoledad
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

    /// TODO(doc): @zhangsoledad
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
    /// TODO(doc): @zhangsoledad
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
}

impl From<&TxEntry> for AncestorsScoreSortKey {
    fn from(entry: &TxEntry) -> Self {
        let vbytes = get_transaction_virtual_bytes(entry.size, entry.cycles);
        let ancestors_vbytes =
            get_transaction_virtual_bytes(entry.ancestors_size, entry.ancestors_cycles);
        AncestorsScoreSortKey {
            fee: entry.fee,
            vbytes,
            id: entry.transaction.proposal_short_id(),
            ancestors_fee: entry.ancestors_fee,
            ancestors_size: entry.ancestors_size,
            ancestors_vbytes,
        }
    }
}

impl Hash for TxEntry {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Hash::hash(&self.transaction, state);
    }
}

impl PartialEq for TxEntry {
    fn eq(&self, other: &TxEntry) -> bool {
        self.transaction == other.transaction
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
        let short_id = entry.transaction.proposal_short_id();
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
