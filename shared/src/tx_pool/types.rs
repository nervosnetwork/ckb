//! The primary module containing the implementations of the transaction pool
//! and its top-level members.

use ckb_core::cell::CellProvider;
use ckb_core::cell::CellStatus;
use ckb_core::transaction::{CellOutput, OutPoint, ProposalShortId, Transaction};
use ckb_core::Cycle;
use ckb_verification::TransactionError;
use failure::Fail;
use fnv::{FnvHashMap, FnvHashSet};
use linked_hash_map::LinkedHashMap;
use occupied_capacity::OccupiedCapacity;
use serde_derive::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::iter::ExactSizeIterator;

pub const MIN_TXS_VERIFY_CACHE_SIZE: usize = 100;

/// Transaction pool configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TxPoolConfig {
    /// Maximum capacity of the pool in number of transactions
    pub max_pool_size: usize,
    pub max_orphan_size: usize,
    pub max_proposal_size: usize,
    pub max_cache_size: usize,
    pub max_pending_size: usize,
    pub trace: Option<usize>,
    pub txs_verify_cache_size: usize,
}

impl Default for TxPoolConfig {
    fn default() -> Self {
        TxPoolConfig {
            max_pool_size: 10000,
            max_orphan_size: 10000,
            max_proposal_size: 10000,
            max_cache_size: 1000,
            max_pending_size: 10000,
            trace: Some(100),
            txs_verify_cache_size: MIN_TXS_VERIFY_CACHE_SIZE,
        }
    }
}

impl TxPoolConfig {
    pub fn trace_enable(&self) -> bool {
        self.trace.is_some()
    }
}

#[derive(Debug, PartialEq, Clone, Eq)]
pub enum StagingTxResult {
    Normal(Cycle),
    Orphan,
    Proposed,
    Unknown,
}

// TODO document this enum more accurately
/// Enum of errors
#[derive(Debug, Clone, PartialEq, Fail)]
pub enum PoolError {
    /// An invalid pool entry caused by underlying tx validation error
    InvalidTx(TransactionError),
    /// An entry already in the pool
    AlreadyInPool,
    /// CellStatus Conflict
    Conflict,
    /// Transaction pool is over capacity, can't accept more transactions
    OverCapacity,
    /// A duplicate output
    DuplicateOutput,
    /// tx_pool don't accept cellbase-like tx
    Cellbase,
    /// TimeOut
    TimeOut,
    /// BlockNumber is not right
    InvalidBlockNumber,
    /// Duplicate tx
    Duplicate,
}

impl fmt::Display for PoolError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self, f)
    }
}

/// An entry in the transaction pool.
#[derive(Debug, Clone)]
pub struct PoolEntry {
    /// Transaction
    pub transaction: Transaction,
    /// refs count
    pub refs_count: usize,
    /// Bytes size
    pub bytes_size: usize,
    /// Cycles
    pub cycles: Option<Cycle>,
}

impl PoolEntry {
    /// Create new transaction pool entry
    pub fn new(tx: Transaction, count: usize, cycles: Option<Cycle>) -> PoolEntry {
        PoolEntry {
            bytes_size: tx.occupied_capacity(),
            transaction: tx,
            refs_count: count,
            cycles,
        }
    }
}

impl Hash for PoolEntry {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Hash::hash(&self.transaction, state);
    }
}

impl PartialEq for PoolEntry {
    fn eq(&self, other: &PoolEntry) -> bool {
        self.transaction == other.transaction
    }
}

#[derive(Default, Debug, Clone)]
pub struct Edges<K: Hash + Eq, V: Copy + Eq + Hash> {
    inner: FnvHashMap<K, Option<V>>,
    outer: FnvHashMap<K, Option<V>>,
    deps: FnvHashMap<K, FnvHashSet<V>>,
}

impl<K: Hash + Eq, V: Copy + Eq + Hash> Edges<K, V> {
    pub fn inner_len(&self) -> usize {
        self.inner.len()
    }

    pub fn outer_len(&self) -> usize {
        self.outer.len()
    }

    pub fn insert_outer(&mut self, key: K, value: V) {
        self.outer.insert(key, Some(value));
    }

    pub fn insert_inner(&mut self, key: K, value: V) {
        self.inner.insert(key, Some(value));
    }

    pub fn remove_outer(&mut self, key: &K) -> Option<V> {
        self.outer.remove(key).unwrap_or(None)
    }

    pub fn remove_inner(&mut self, key: &K) -> Option<V> {
        self.inner.remove(key).unwrap_or(None)
    }

    pub fn mark_inpool(&mut self, key: K) {
        self.inner.insert(key, None);
    }

    pub fn get_inner(&self, key: &K) -> Option<&Option<V>> {
        self.inner.get(key)
    }

    pub fn get_outer(&self, key: &K) -> Option<&Option<V>> {
        self.outer.get(key)
    }

    pub fn get_inner_mut(&mut self, key: &K) -> Option<&mut Option<V>> {
        self.inner.get_mut(key)
    }

    pub fn remove_edge(&mut self, key: &K) -> Option<V> {
        self.inner.remove(key).unwrap_or(None)
    }

    pub fn get_deps(&self, key: &K) -> Option<&FnvHashSet<V>> {
        self.deps.get(key)
    }

    pub fn remove_deps(&mut self, key: &K) -> Option<FnvHashSet<V>> {
        self.deps.remove(key)
    }

    pub fn contains_key(&self, key: &K) -> bool {
        self.inner.contains_key(&key)
    }

    pub fn insert_deps(&mut self, key: K, value: V) {
        let e = self.deps.entry(key).or_insert_with(FnvHashSet::default);
        e.insert(value);
    }

    pub fn delete_value_in_deps(&mut self, key: &K, value: &V) {
        let mut empty = false;

        if let Some(x) = self.deps.get_mut(key) {
            x.remove(value);
            empty = x.is_empty();
        }

        if empty {
            self.deps.remove(key);
        }
    }
}

#[derive(Default, Debug, Clone)]
pub struct StagingPool {
    pub vertices: LinkedHashMap<ProposalShortId, PoolEntry>,
    pub edges: Edges<OutPoint, ProposalShortId>,
}

impl CellProvider for StagingPool {
    fn cell(&self, o: &OutPoint) -> CellStatus {
        if let Some(x) = self.edges.get_inner(o) {
            if x.is_some() {
                CellStatus::Dead
            } else {
                CellStatus::Live(self.get_output(o).unwrap())
            }
        } else if self.edges.get_outer(o).is_some() {
            CellStatus::Dead
        } else {
            CellStatus::Unknown
        }
    }
}

impl StagingPool {
    pub fn new() -> Self {
        StagingPool::default()
    }

    pub fn capacity(&self) -> usize {
        self.vertices.len()
    }

    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.vertices.contains_key(id)
    }

    pub fn get(&self, id: &ProposalShortId) -> Option<&PoolEntry> {
        self.vertices.get(id)
    }

    pub fn get_tx(&self, id: &ProposalShortId) -> Option<&Transaction> {
        self.get(id).map(|x| &x.transaction)
    }

    pub fn get_output(&self, o: &OutPoint) -> Option<CellOutput> {
        self.vertices
            .get(&ProposalShortId::from_h256(&o.hash))
            .and_then(|x| x.transaction.get_output(o.index as usize))
    }

    pub fn remove_vertex(&mut self, id: &ProposalShortId, rtxs: &mut Vec<PoolEntry>) {
        if let Some(x) = self.vertices.remove(id) {
            let tx = &x.transaction;
            let inputs = tx.input_pts();
            let outputs = tx.output_pts();
            let deps = tx.dep_pts();

            rtxs.push(x);

            for i in inputs {
                if let Some(x) = self.edges.inner.get_mut(&i) {
                    *x = None;
                } else {
                    self.edges.outer.remove(&i);
                }
            }

            for d in deps {
                self.edges.delete_value_in_deps(&d, id);
            }

            for o in outputs {
                if let Some(cid) = self.edges.remove_inner(&o) {
                    self.remove_vertex(&cid, rtxs);
                }

                if let Some(ids) = self.edges.remove_deps(&o) {
                    for cid in ids {
                        self.remove_vertex(&cid, rtxs);
                    }
                }
            }
        }
    }

    pub fn remove(&mut self, id: &ProposalShortId) -> Option<Vec<PoolEntry>> {
        let mut rtxs = Vec::new();

        self.remove_vertex(id, &mut rtxs);

        if rtxs.is_empty() {
            None
        } else {
            Some(rtxs)
        }
    }

    /// Readd a verified transaction which is rolled back from chain. Since the rolled back
    /// transaction should depend on any transaction in the pool, it is safe to skip some checking.
    pub fn readd_tx(&mut self, tx: &Transaction, cycles: Cycle) {
        let inputs = tx.input_pts();
        let outputs = tx.output_pts();
        let deps = tx.dep_pts();
        let id = tx.proposal_short_id();

        self.vertices.insert_front(
            tx.proposal_short_id(),
            PoolEntry::new(tx.clone(), 0, Some(cycles)),
        );

        for i in inputs {
            self.edges.insert_outer(i, id);
        }

        for d in deps {
            self.edges.insert_deps(d, id);
        }

        for o in outputs {
            if let Some(id) = self.edges.remove_outer(&o) {
                self.inc_ref(&id);
                self.edges.insert_inner(o.clone(), id);
            } else {
                self.edges.mark_inpool(o.clone());
            }

            if let Some(cids) = { self.edges.get_deps(&o).cloned() } {
                for cid in cids {
                    self.inc_ref(&cid);
                }
            }
        }
    }

    pub fn add_tx(&mut self, mut entry: PoolEntry) {
        let tx = &entry.transaction;
        let inputs = tx.input_pts();
        let outputs = tx.output_pts();
        let deps = tx.dep_pts();

        let id = tx.proposal_short_id();

        let mut count: usize = 0;

        for i in inputs {
            let mut flag = true;
            if let Some(x) = self.edges.get_inner_mut(&i) {
                *x = Some(id);
                count += 1;
                flag = false;
            }

            if flag {
                self.edges.insert_outer(i, id);
            }
        }

        for d in deps {
            if self.edges.contains_key(&d) {
                count += 1;
            }
            self.edges.insert_deps(d, id);
        }

        for o in outputs {
            self.edges.mark_inpool(o);
        }

        entry.refs_count = count;
        self.vertices.insert(id, entry);
    }

    pub fn commit_tx(&mut self, tx: &Transaction) {
        let outputs = tx.output_pts();
        let inputs = tx.input_pts();
        let deps = tx.dep_pts();
        let id = tx.proposal_short_id();

        if self.vertices.remove(&id).is_some() {
            for o in outputs {
                if let Some(cid) = self.edges.remove_inner(&o) {
                    self.dec_ref(&cid);
                    self.edges.insert_outer(o.clone(), cid);
                }

                if let Some(x) = { self.edges.get_deps(&o).cloned() } {
                    for cid in x {
                        self.dec_ref(&cid);
                    }
                }
            }

            for i in inputs {
                self.edges.remove_outer(&i);
            }

            for d in deps {
                self.edges.delete_value_in_deps(&d, &id)
            }
        } else {
            self.resolve_conflict(tx);
        }
    }

    pub fn resolve_conflict(&mut self, tx: &Transaction) {
        let inputs = tx.input_pts();

        for i in inputs {
            if let Some(id) = self.edges.remove_outer(&i) {
                self.remove(&id);
            }

            if let Some(x) = self.edges.remove_deps(&i) {
                for id in x {
                    self.remove(&id);
                }
            }
        }
    }

    /// Get n transactions in topology
    pub fn get_txs(&self, n: usize) -> Vec<PoolEntry> {
        self.vertices
            .front_n(n)
            .iter()
            .map(|x| x.1.clone())
            .collect()
    }

    pub fn txs_iter(&self) -> impl Iterator<Item = &PoolEntry> {
        self.vertices.values()
    }

    pub fn inc_ref(&mut self, id: &ProposalShortId) {
        if let Some(x) = self.vertices.get_mut(&id) {
            x.refs_count += 1;
        }
    }

    pub fn dec_ref(&mut self, id: &ProposalShortId) {
        if let Some(x) = self.vertices.get_mut(&id) {
            x.refs_count -= 1;
        }
    }
}

///not verified, may contain conflict transactions
#[derive(Default, Debug, Clone)]
pub struct OrphanPool {
    pub vertices: FnvHashMap<ProposalShortId, PoolEntry>,
    pub edges: FnvHashMap<OutPoint, Vec<ProposalShortId>>,
}

impl OrphanPool {
    pub fn new() -> Self {
        OrphanPool::default()
    }

    pub fn capacity(&self) -> usize {
        self.vertices.len()
    }

    pub fn get(&self, id: &ProposalShortId) -> Option<&PoolEntry> {
        self.vertices.get(id)
    }

    pub fn get_tx(&self, id: &ProposalShortId) -> Option<&Transaction> {
        self.get(id).map(|x| &x.transaction)
    }

    pub fn contains(&self, tx: &Transaction) -> bool {
        self.vertices.contains_key(&tx.proposal_short_id())
    }

    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.vertices.contains_key(id)
    }

    /// add orphan transaction
    pub fn add_tx(
        &mut self,
        mut entry: PoolEntry,
        unknown: impl ExactSizeIterator<Item = OutPoint>,
    ) {
        let short_id = entry.transaction.proposal_short_id();
        let len = unknown.len();
        for out_point in unknown {
            let edge = self.edges.entry(out_point).or_insert_with(Vec::new);
            edge.push(short_id);
        }
        entry.refs_count = len;
        self.vertices.insert(short_id, entry);
    }

    pub fn remove(&mut self, id: &ProposalShortId) -> Option<PoolEntry> {
        self.vertices.remove(id)
    }

    pub fn recursion_remove(&mut self, id: &ProposalShortId) -> VecDeque<PoolEntry> {
        let mut removed = VecDeque::new();

        let mut queue: VecDeque<&ProposalShortId> = VecDeque::new();
        queue.push_back(id);
        while let Some(id) = queue.pop_front() {
            if let Some(entry) = self.vertices.remove(id) {
                for outpoint in entry.transaction.output_pts() {
                    if let Some(ids) = self.edges.remove(&outpoint) {
                        if let Some(entries) = ids
                            .iter()
                            .map(|id| self.vertices.remove(id))
                            .collect::<Option<Vec<PoolEntry>>>()
                        {
                            removed.extend(entries);
                        }
                    }
                }
                removed.push_back(entry);
            }
        }
        removed
    }

    pub fn remove_by_ancestor(&mut self, tx: &Transaction) -> Vec<PoolEntry> {
        let mut txs = Vec::new();
        let mut queue = VecDeque::new();

        self.remove_conflict(tx);

        queue.push_back(tx.output_pts());
        while let Some(outputs) = queue.pop_front() {
            for o in outputs {
                if let Some(ids) = self.edges.remove(&o) {
                    for cid in ids {
                        if let Some(mut x) = self.vertices.remove(&cid) {
                            x.refs_count -= 1;
                            if x.refs_count == 0 {
                                queue.push_back(x.transaction.output_pts());
                                txs.push(x);
                            } else {
                                self.vertices.insert(cid, x);
                            }
                        }
                    }
                }
            }
        }
        txs
    }

    pub fn remove_conflict(&mut self, tx: &Transaction) {
        let inputs = tx.input_pts();

        for i in inputs {
            if let Some(ids) = self.edges.remove(&i) {
                for cid in ids {
                    self.recursion_remove(&cid);
                }
            }
        }
    }
}

#[derive(Default, Debug, Clone)]
pub struct PendingQueue {
    inner: FnvHashMap<ProposalShortId, PoolEntry>,
}

impl PendingQueue {
    pub fn new() -> Self {
        PendingQueue {
            inner: FnvHashMap::default(),
        }
    }

    pub fn size(&self) -> usize {
        self.inner.len()
    }

    pub fn insert(&mut self, id: ProposalShortId, tx: PoolEntry) -> Option<PoolEntry> {
        self.inner.insert(id, tx)
    }

    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.inner.contains_key(id)
    }

    pub fn get(&self, id: &ProposalShortId) -> Option<&PoolEntry> {
        self.inner.get(id)
    }

    pub fn get_tx(&self, id: &ProposalShortId) -> Option<&Transaction> {
        self.get(id).map(|x| &x.transaction)
    }

    pub fn remove(&mut self, id: &ProposalShortId) -> Option<PoolEntry> {
        self.inner.remove(id)
    }

    pub fn fetch(&self, n: usize) -> Vec<ProposalShortId> {
        self.inner.keys().take(n).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_core::script::Script;
    use ckb_core::transaction::{CellInput, CellOutput, Transaction, TransactionBuilder};
    use numext_fixed_hash::H256;

    fn build_tx(inputs: Vec<(H256, u32)>, outputs_len: usize) -> PoolEntry {
        let tx = TransactionBuilder::default()
            .inputs(
                inputs
                    .into_iter()
                    .map(|(txid, index)| {
                        CellInput::new(OutPoint::new(txid, index), Default::default())
                    })
                    .collect(),
            )
            .outputs(
                (0..outputs_len)
                    .map(|i| CellOutput::new((i + 1) as u64, Vec::new(), Script::default(), None))
                    .collect(),
            )
            .build();

        PoolEntry::new(tx, 0, None)
    }

    #[test]
    fn test_add_entry() {
        let tx1 = build_tx(vec![(H256::zero(), 1), (H256::zero(), 2)], 1);
        let tx1_hash = tx1.transaction.hash().clone();
        let tx2 = build_tx(vec![(tx1_hash, 0)], 1);

        let mut pool = StagingPool::new();
        let id1 = tx1.transaction.proposal_short_id();
        let id2 = tx2.transaction.proposal_short_id();

        pool.add_tx(tx1.clone());
        pool.add_tx(tx2.clone());

        assert_eq!(pool.vertices.len(), 2);
        assert_eq!(pool.edges.inner_len(), 2);
        assert_eq!(pool.edges.outer_len(), 2);

        assert_eq!(pool.get(&id1).unwrap().refs_count, 0);
        assert_eq!(pool.get(&id2).unwrap().refs_count, 1);

        pool.commit_tx(&tx1.transaction);
        assert_eq!(pool.edges.inner_len(), 1);
        assert_eq!(pool.edges.outer_len(), 1);

        assert_eq!(pool.get(&id2).unwrap().refs_count, 0);
    }

    #[test]
    fn test_add_roots() {
        let tx1 = build_tx(vec![(H256::zero(), 1), (H256::zero(), 2)], 1);
        let tx2 = build_tx(
            vec![
                (H256::from_trimmed_hex_str("2").unwrap(), 1),
                (H256::from_trimmed_hex_str("3").unwrap(), 2),
            ],
            3,
        );

        let mut pool = StagingPool::new();

        let id1 = tx1.transaction.proposal_short_id();
        let id2 = tx2.transaction.proposal_short_id();

        pool.add_tx(tx1.clone());
        pool.add_tx(tx2.clone());

        assert_eq!(pool.get(&id1).unwrap().refs_count, 0);
        assert_eq!(pool.get(&id2).unwrap().refs_count, 0);
        assert_eq!(pool.edges.inner_len(), 4);
        assert_eq!(pool.edges.outer_len(), 4);

        let mut mineable = pool.get_txs(0);
        assert_eq!(0, mineable.len());

        mineable = pool.get_txs(1);
        assert_eq!(1, mineable.len());
        assert!(mineable.contains(&tx1));

        mineable = pool.get_txs(2);
        assert_eq!(2, mineable.len());
        assert!(mineable.contains(&tx1) && mineable.contains(&tx2));

        mineable = pool.get_txs(3);
        assert_eq!(2, mineable.len());
        assert!(mineable.contains(&tx1) && mineable.contains(&tx2));

        pool.commit_tx(&tx1.transaction);

        assert_eq!(pool.edges.inner_len(), 3);
        assert_eq!(pool.edges.outer_len(), 2);
    }

    #[test]
    fn test_pending_queue() {
        let mut pending = PendingQueue::new();

        for i in 0..20 {
            let tx = build_tx(vec![(H256::zero(), i), (H256::zero(), i + 20)], 2);

            pending.insert(tx.transaction.proposal_short_id(), tx);
        }

        assert_eq!(pending.size(), 20);
    }

    #[test]
    #[allow(clippy::cyclomatic_complexity)]
    fn test_add_no_roots() {
        let tx1 = build_tx(vec![(H256::zero(), 1)], 3);
        let tx2 = build_tx(vec![], 4);
        let tx1_hash = tx1.transaction.hash().clone();
        let tx2_hash = tx2.transaction.hash().clone();

        let tx3 = build_tx(vec![(tx1_hash.clone(), 0), (H256::zero(), 2)], 2);
        let tx4 = build_tx(vec![(tx1_hash.clone(), 1), (tx2_hash.clone(), 0)], 2);

        let tx3_hash = tx3.transaction.hash().clone();
        let tx5 = build_tx(vec![(tx1_hash.clone(), 2), (tx3_hash.clone(), 0)], 2);

        let id1 = tx1.transaction.proposal_short_id();
        let id3 = tx3.transaction.proposal_short_id();
        let id5 = tx5.transaction.proposal_short_id();

        let mut pool = StagingPool::new();

        pool.add_tx(tx1.clone());
        pool.add_tx(tx2.clone());
        pool.add_tx(tx3.clone());
        pool.add_tx(tx4.clone());
        pool.add_tx(tx5.clone());

        assert_eq!(pool.get(&id1).unwrap().refs_count, 0);
        assert_eq!(pool.get(&id3).unwrap().refs_count, 1);
        assert_eq!(pool.get(&id5).unwrap().refs_count, 2);
        assert_eq!(pool.edges.inner_len(), 13);
        assert_eq!(pool.edges.outer_len(), 2);

        let mut mineable: Vec<Transaction> =
            pool.get_txs(0).into_iter().map(|x| x.transaction).collect();
        assert_eq!(0, mineable.len());

        mineable = pool.get_txs(1).into_iter().map(|x| x.transaction).collect();
        assert_eq!(1, mineable.len());
        assert!(mineable.contains(&tx1.transaction));

        mineable = pool.get_txs(2).into_iter().map(|x| x.transaction).collect();
        assert_eq!(2, mineable.len());
        assert!(mineable.contains(&tx1.transaction) && mineable.contains(&tx2.transaction));

        mineable = pool.get_txs(3).into_iter().map(|x| x.transaction).collect();
        assert_eq!(3, mineable.len());

        assert!(
            mineable.contains(&tx1.transaction)
                && mineable.contains(&tx2.transaction)
                && mineable.contains(&tx3.transaction)
        );

        mineable = pool.get_txs(4).into_iter().map(|x| x.transaction).collect();
        assert_eq!(4, mineable.len());
        assert!(mineable.contains(&tx1.transaction) && mineable.contains(&tx2.transaction));
        assert!(mineable.contains(&tx3.transaction) && mineable.contains(&tx4.transaction));

        mineable = pool.get_txs(5).into_iter().map(|x| x.transaction).collect();
        assert_eq!(5, mineable.len());
        assert!(mineable.contains(&tx1.transaction) && mineable.contains(&tx2.transaction));
        assert!(mineable.contains(&tx3.transaction) && mineable.contains(&tx4.transaction));
        assert!(mineable.contains(&tx5.transaction));

        mineable = pool.get_txs(6).into_iter().map(|x| x.transaction).collect();
        assert_eq!(5, mineable.len());
        assert!(mineable.contains(&tx1.transaction) && mineable.contains(&tx2.transaction));
        assert!(mineable.contains(&tx3.transaction) && mineable.contains(&tx4.transaction));
        assert!(mineable.contains(&tx5.transaction));

        pool.commit_tx(&tx1.transaction);

        assert_eq!(pool.get(&id3).unwrap().refs_count, 0);
        assert_eq!(pool.get(&id5).unwrap().refs_count, 1);
        assert_eq!(pool.edges.inner_len(), 10);
        assert_eq!(pool.edges.outer_len(), 4);

        mineable = pool.get_txs(1).into_iter().map(|x| x.transaction).collect();
        assert_eq!(1, mineable.len());
        assert!(mineable.contains(&tx2.transaction));

        mineable = pool.get_txs(2).into_iter().map(|x| x.transaction).collect();
        assert_eq!(2, mineable.len());
        assert!(mineable.contains(&tx2.transaction) && mineable.contains(&tx3.transaction));

        mineable = pool.get_txs(3).into_iter().map(|x| x.transaction).collect();
        assert_eq!(3, mineable.len());
        assert!(mineable.contains(&tx2.transaction) && mineable.contains(&tx3.transaction));
        assert!(mineable.contains(&tx4.transaction));

        mineable = pool.get_txs(4).into_iter().map(|x| x.transaction).collect();
        assert_eq!(4, mineable.len());
        assert!(mineable.contains(&tx2.transaction) && mineable.contains(&tx3.transaction));
        assert!(mineable.contains(&tx4.transaction) && mineable.contains(&tx5.transaction));

        mineable = pool.get_txs(5).into_iter().map(|x| x.transaction).collect();
        assert_eq!(4, mineable.len());
    }
}
