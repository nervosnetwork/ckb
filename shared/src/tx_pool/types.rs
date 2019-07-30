//! The primary module containing the implementations of the transaction pool
//! and its top-level members.

use crate::tx_pool::get_transaction_virtual_bytes;
use ckb_core::cell::UnresolvableError;
use ckb_core::transaction::{ProposalShortId, Transaction};
use ckb_core::Capacity;
use ckb_core::Cycle;
use ckb_verification::TransactionError;
use failure::Fail;
use serde_derive::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};

/// Transaction pool configuration
#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
pub struct TxPoolConfig {
    // Keep the transaction pool below <max_mem_size> mb
    pub max_mem_size: usize,
    // Keep the transaction pool below <max_cycles> cycles
    pub max_cycles: Cycle,
    // tx verify cache capacity
    pub max_verify_cache_size: usize,
    // conflict tx cache capacity
    pub max_conflict_cache_size: usize,
    // committed transactions hash cache capacity
    pub max_committed_txs_hash_cache_size: usize,
}

impl Default for TxPoolConfig {
    fn default() -> Self {
        TxPoolConfig {
            max_mem_size: 20_000_000, // 20mb
            max_cycles: 200_000_000_000,
            max_verify_cache_size: 100_000,
            max_conflict_cache_size: 1_000,
            max_committed_txs_hash_cache_size: 100_000,
        }
    }
}

// TODO document this enum more accurately
/// Enum of errors
#[derive(Debug, Clone, PartialEq, Fail)]
pub enum PoolError {
    /// Unresolvable CellStatus
    UnresolvableTransaction(UnresolvableError),
    /// An invalid pool entry caused by underlying tx validation error
    InvalidTx(TransactionError),
    /// Transaction pool reach limit, can't accept more transactions
    LimitReached,
    /// TimeOut
    TimeOut,
    /// BlockNumber is not right
    InvalidBlockNumber,
    /// Duplicate tx
    Duplicate,
    /// tx fee
    TxFee,
}

impl PoolError {
    /// Transaction error may be caused by different tip between peers if this method return false,
    /// Otherwise we consider the Bad Tx is constructed intendedly.
    pub fn is_bad_tx(&self) -> bool {
        match self {
            PoolError::InvalidTx(err) => err.is_bad_tx(),
            _ => false,
        }
    }
}

impl fmt::Display for PoolError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self, f)
    }
}

/// An defect entry (conflict or orphan) in the transaction pool.
#[derive(Debug, Clone)]
pub struct DefectEntry {
    /// Transaction
    pub transaction: Transaction,
    /// refs count
    pub refs_count: usize,
    /// Cycles
    pub cycles: Option<Cycle>,
    /// tx size
    pub size: usize,
}

impl DefectEntry {
    /// Create new transaction pool entry
    pub fn new(
        tx: Transaction,
        refs_count: usize,
        cycles: Option<Cycle>,
        size: usize,
    ) -> DefectEntry {
        DefectEntry {
            transaction: tx,
            refs_count,
            cycles,
            size,
        }
    }
}

/// An entry in the transaction pool.
#[derive(Debug, Clone)]
pub struct PendingEntry {
    /// Transaction
    pub transaction: Transaction,
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
}

impl PendingEntry {
    /// Create new transaction pool entry
    pub fn new(tx: Transaction, cycles: Cycle, fee: Capacity, size: usize) -> PendingEntry {
        PendingEntry {
            transaction: tx,
            cycles,
            size,
            fee,
            ancestors_size: size,
            ancestors_fee: fee,
            ancestors_cycles: cycles,
            ancestors_count: 1,
        }
    }
}

impl From<&PendingEntry> for AncestorsScoreSortKey {
    fn from(entry: &PendingEntry) -> Self {
        let vbytes = get_transaction_virtual_bytes(entry.size, entry.cycles);
        let ancestors_vbytes =
            get_transaction_virtual_bytes(entry.ancestors_size, entry.ancestors_cycles);
        AncestorsScoreSortKey {
            fee: entry.fee,
            vbytes,
            id: entry.transaction.proposal_short_id(),
            ancestors_fee: entry.ancestors_fee,
            ancestors_vbytes,
        }
    }
}

/// An entry in the transaction pool.
#[derive(Debug, Clone, Eq)]
pub struct ProposedEntry {
    /// Transaction
    pub transaction: Transaction,
    /// refs count
    pub refs_count: usize,
    /// Cycles
    pub cycles: Cycle,
    /// fee
    pub fee: Capacity,
    /// tx size
    pub size: usize,
    /// ancestors txs size
    pub ancestors_size: usize,
    /// ancestors txs fee
    pub ancestors_fee: Capacity,
    /// ancestors txs cycles
    pub ancestors_cycles: Cycle,
    /// ancestors txs count
    pub ancestors_count: usize,
}

impl ProposedEntry {
    /// Create new transaction pool entry
    pub fn new(
        tx: Transaction,
        refs_count: usize,
        cycles: Cycle,
        fee: Capacity,
        size: usize,
    ) -> ProposedEntry {
        ProposedEntry {
            transaction: tx,
            refs_count,
            cycles,
            fee,
            size,
            ancestors_size: size,
            ancestors_cycles: cycles,
            ancestors_fee: fee,
            ancestors_count: 1,
        }
    }

    /// Virtual bytes(aka vbytes) is a concept to unify the size and cycles of a transaction,
    /// tx_pool use vbytes to estimate transaction fee rate.
    pub fn virtual_bytes(&self) -> u64 {
        get_transaction_virtual_bytes(self.size, self.cycles)
    }
}

impl Hash for ProposedEntry {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Hash::hash(&self.transaction, state);
    }
}

impl PartialEq for ProposedEntry {
    fn eq(&self, other: &ProposedEntry) -> bool {
        self.transaction == other.transaction
    }
}

impl From<&ProposedEntry> for AncestorsScoreSortKey {
    fn from(entry: &ProposedEntry) -> Self {
        AncestorsScoreSortKey {
            fee: entry.fee,
            vbytes: entry.virtual_bytes(),
            id: entry.transaction.proposal_short_id(),
            ancestors_fee: entry.ancestors_fee,
            ancestors_vbytes: get_transaction_virtual_bytes(
                entry.ancestors_size,
                entry.ancestors_cycles,
            ),
        }
    }
}

/// A struct to use as a sorted key
#[derive(Eq, PartialEq, Clone, Debug)]
pub struct AncestorsScoreSortKey {
    pub fee: Capacity,
    pub vbytes: u64,
    pub id: ProposalShortId,
    pub ancestors_fee: Capacity,
    pub ancestors_vbytes: u64,
}

impl AncestorsScoreSortKey {
    /// compare tx fee rate with ancestors fee rate and return the min one
    fn min_fee_and_vbytes(&self) -> (Capacity, u64) {
        // avoid division a_fee/a_vbytes > b_fee/b_vbytes
        let tx_weight = self.fee.as_u64() * self.ancestors_vbytes;
        let ancestors_weight = self.ancestors_fee.as_u64() * self.vbytes;

        if tx_weight < ancestors_weight {
            (self.fee, self.vbytes)
        } else {
            (self.ancestors_fee, self.ancestors_vbytes)
        }
    }
}

impl PartialOrd for AncestorsScoreSortKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for AncestorsScoreSortKey {
    fn cmp(&self, other: &Self) -> Ordering {
        // avoid division a_fee/a_vbytes > b_fee/b_vbytes
        let (fee, vbytes) = self.min_fee_and_vbytes();
        let (other_fee, other_vbytes) = other.min_fee_and_vbytes();
        let self_weight = fee.as_u64() * other_vbytes;
        let other_weight = other_fee.as_u64() * vbytes;
        if self_weight == other_weight {
            // if fee rate weight is same, then compare with ancestor vbytes
            if self.ancestors_vbytes == other.ancestors_vbytes {
                self.id.cmp(&other.id)
            } else {
                self.ancestors_vbytes.cmp(&other.ancestors_vbytes)
            }
        } else {
            self_weight.cmp(&other_weight)
        }
    }
}

#[derive(Default, Debug, Clone)]
pub struct TxLink {
    pub parents: HashSet<ProposalShortId>,
    pub children: HashSet<ProposalShortId>,
}

#[derive(Clone, Copy)]
enum Relation {
    Parents,
    Children,
}

impl TxLink {
    fn get_direct_ids(&self, r: Relation) -> &HashSet<ProposalShortId> {
        match r {
            Relation::Parents => &self.parents,
            Relation::Children => &self.children,
        }
    }

    fn get_relative_ids(
        links: &HashMap<ProposalShortId, TxLink>,
        tx_short_id: &ProposalShortId,
        relation: Relation,
    ) -> HashSet<ProposalShortId> {
        let mut family_txs = links
            .get(tx_short_id)
            .map(|link| link.get_direct_ids(relation).clone())
            .unwrap_or_default();
        let mut relative_txs = HashSet::with_capacity(family_txs.len());
        while !family_txs.is_empty() {
            let id = family_txs
                .iter()
                .next()
                .map(ToOwned::to_owned)
                .expect("exists");
            relative_txs.insert(id);
            family_txs.remove(&id);

            // check parents recursively
            for id in links
                .get(&id)
                .map(|link| link.get_direct_ids(relation).clone())
                .unwrap_or_default()
            {
                if !relative_txs.contains(&id) {
                    family_txs.insert(id);
                }
            }
        }
        relative_txs
    }

    pub fn get_ancestors(
        links: &HashMap<ProposalShortId, TxLink>,
        tx_short_id: &ProposalShortId,
    ) -> HashSet<ProposalShortId> {
        TxLink::get_relative_ids(links, tx_short_id, Relation::Parents)
    }

    pub fn get_descendants(
        links: &HashMap<ProposalShortId, TxLink>,
        tx_short_id: &ProposalShortId,
    ) -> HashSet<ProposalShortId> {
        TxLink::get_relative_ids(links, tx_short_id, Relation::Children)
    }
}

pub struct TxEntryContainer {
    entries: HashMap<ProposalShortId, ProposedEntry>,
    sort_index: BTreeSet<AncestorsScoreSortKey>,
}

impl Default for TxEntryContainer {
    fn default() -> Self {
        TxEntryContainer {
            entries: HashMap::default(),
            sort_index: BTreeSet::default(),
        }
    }
}

impl TxEntryContainer {
    pub fn with_sorted_by_score_iter<F, Ret>(&self, func: F) -> Ret
    where
        F: FnOnce(&mut dyn Iterator<Item = &ProposedEntry>) -> Ret,
    {
        let mut iter = self
            .sort_index
            .iter()
            .rev()
            .map(|key| self.entries.get(&key.id).expect("must be consistent"));
        func(&mut iter)
    }

    pub fn get(&self, id: &ProposalShortId) -> Option<&ProposedEntry> {
        self.entries.get(id)
    }

    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.entries.contains_key(id)
    }

    pub fn insert(&mut self, entry: ProposedEntry) {
        let key = AncestorsScoreSortKey::from(&entry);
        let short_id = entry.transaction.proposal_short_id();
        self.entries.insert(short_id, entry);
        self.sort_index.insert(key);
    }

    pub fn remove(&mut self, id: &ProposalShortId) -> Option<ProposedEntry> {
        self.entries.remove(id).map(|entry| {
            self.sort_index.remove(&(&entry).into());
            entry
        })
    }
}
