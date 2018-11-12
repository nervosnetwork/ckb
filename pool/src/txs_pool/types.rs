//! The primary module containing the implementations of the transaction pool
//! and its top-level members.

use chain_spec::consensus::{TRANSACTION_PROPAGATION_TIME, TRANSACTION_PROPAGATION_TIMEOUT};
use ckb_verification::TransactionError;
use core::transaction::{CellOutput, OutPoint, ProposalShortId, Transaction};
use core::BlockNumber;
use fnv::{FnvHashMap, FnvHashSet};
use linked_hash_map::LinkedHashMap;
use std::collections::VecDeque;
use std::hash::Hash;
use std::iter::Iterator;

const BUFF_QUE_LEN: u64 = 100;

/// Transaction pool configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PoolConfig {
    /// Maximum capacity of the pool in number of transactions
    pub max_pool_size: usize,
    pub max_orphan_size: usize,
    pub max_proposal_size: usize,
    pub max_cache_size: usize,
    pub max_pending_size: usize,
}

impl Default for PoolConfig {
    fn default() -> Self {
        PoolConfig {
            max_pool_size: 10000,
            max_orphan_size: 10000,
            max_proposal_size: 10000,
            max_cache_size: 1000,
            max_pending_size: 10000,
        }
    }
}

/// This enum describes the status of a transaction's outpoint.
#[derive(Clone, Debug, PartialEq)]
pub enum TxoStatus {
    Unknown,
    InPool,
    Spent,
}

#[derive(Clone, Debug)]
pub enum InsertionResult {
    Normal,
    Orphan,
    Proposed,
    Unknown,
}

#[derive(PartialEq, Clone, Debug)]
pub enum TxStage {
    Unknown(Transaction),
    Fork(Transaction),
    Mineable(Transaction),
    TimeOut(Transaction),
    Proposed,
}

// TODO document this enum more accurately
/// Enum of errors
#[derive(Debug)]
pub enum PoolError {
    /// An invalid pool entry caused by underlying tx validation error
    InvalidTx(TransactionError),
    /// An entry already in the pool
    AlreadyInPool,
    /// A double spend
    DoubleSpent,
    /// Transaction pool is over capacity, can't accept more transactions
    OverCapacity,
    /// A duplicate output
    DuplicateOutput,
    /// Coinbase transaction
    CellBase,
    /// TimeOut
    TimeOut,
    /// Blocknumber is not right
    InvalidBlockNumber,
}

/// An entry in the transaction pool.
#[derive(Debug, PartialEq, Clone)]
pub struct PoolEntry {
    /// Transaction
    pub transaction: Transaction,
    /// refs count
    pub refs_count: usize,
    /// Size estimate
    pub size_estimate: usize,
}

impl PoolEntry {
    /// Create new transaction pool entry
    pub fn new(tx: Transaction, count: usize) -> PoolEntry {
        PoolEntry {
            size_estimate: estimate_transaction_size(&tx),
            transaction: tx,
            refs_count: count,
        }
    }
}

/// TODO guessing this needs implementing
fn estimate_transaction_size(_tx: &Transaction) -> usize {
    0
}

#[derive(Default, Debug)]
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

    pub fn is_in_pool(&self, key: &K) -> bool {
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

#[derive(Default, Debug)]
pub struct Pool {
    pub vertices: LinkedHashMap<ProposalShortId, PoolEntry>,
    pub edges: Edges<OutPoint, ProposalShortId>,
}

impl Pool {
    pub fn new() -> Self {
        Pool::default()
    }

    //TODO: size
    pub fn size(&self) -> usize {
        self.vertices.len()
    }

    pub fn contains(&self, tx: &Transaction) -> bool {
        self.vertices.contains_key(&tx.proposal_short_id())
    }

    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.vertices.contains_key(id)
    }

    pub fn txo_status(&self, o: &OutPoint) -> TxoStatus {
        if let Some(x) = self.edges.get_inner(o) {
            if x.is_some() {
                TxoStatus::Spent
            } else {
                TxoStatus::InPool
            }
        } else if self.edges.get_outer(o).is_some() {
            TxoStatus::Spent
        } else {
            TxoStatus::Unknown
        }
    }

    pub fn get_entry(&self, id: &ProposalShortId) -> Option<&PoolEntry> {
        self.vertices.get(id)
    }

    pub fn get(&self, id: &ProposalShortId) -> Option<&Transaction> {
        self.vertices.get(id).map(|x| &x.transaction)
    }

    pub fn get_transaction(&self, id: &ProposalShortId) -> Option<&Transaction> {
        self.vertices.get(id).map(|x| &x.transaction)
    }

    pub fn get_output(&self, o: &OutPoint) -> Option<CellOutput> {
        self.vertices
            .get(&ProposalShortId::from_h256(&o.hash))
            .and_then(|x| x.transaction.get_output(o.index as usize))
    }

    pub fn remove_vertex(&mut self, id: &ProposalShortId, rtxs: &mut Vec<Transaction>) {
        if let Some(x) = self.vertices.remove(id) {
            let tx = x.transaction;
            let inputs = tx.input_pts();
            let outputs = tx.output_pts();
            let deps = tx.dep_pts();

            rtxs.push(tx);

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

    pub fn remove(&mut self, id: &ProposalShortId) -> Option<Vec<Transaction>> {
        let mut rtxs = Vec::new();

        self.remove_vertex(id, &mut rtxs);

        if rtxs.is_empty() {
            None
        } else {
            Some(rtxs)
        }
    }

    /// Add a verified transaction.
    pub fn add_transaction(&mut self, tx: Transaction) {
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
            if self.edges.is_in_pool(&d) {
                count += 1;
            }
            self.edges.insert_deps(d, id);
        }

        for o in outputs {
            self.edges.mark_inpool(o);
        }

        self.vertices.insert(id, PoolEntry::new(tx, count));
    }

    /// Readd a verified transaction which is rolled back from chain. Since the rolled back
    /// transaction should depend on any transaction in the pool, it is safe to skip some checking.
    pub fn readd_transaction(&mut self, tx: &Transaction) {
        let inputs = tx.input_pts();
        let outputs = tx.output_pts();
        let deps = tx.dep_pts();
        let id = tx.proposal_short_id();

        self.vertices
            .insert_front(tx.proposal_short_id(), PoolEntry::new(tx.clone(), 0));

        for i in inputs {
            self.edges.insert_outer(i, id);
        }

        for d in deps {
            self.edges.insert_deps(d, id);
        }

        for o in outputs {
            if let Some(id) = self.edges.remove_outer(&o) {
                self.inc_ref(&id);
                self.edges.insert_inner(o, id);
            } else {
                self.edges.mark_inpool(o);
            }

            if let Some(cids) = { self.edges.get_deps(&o).cloned() } {
                for cid in cids {
                    self.inc_ref(&cid);
                }
            }
        }
    }

    ///Commit proposed transaction
    pub fn commit_transaction(&mut self, tx: &Transaction) {
        let outputs = tx.output_pts();
        let inputs = tx.input_pts();
        let deps = tx.dep_pts();
        let id = tx.proposal_short_id();

        if self.vertices.remove(&id).is_some() {
            for o in outputs {
                if let Some(cid) = self.edges.remove_inner(&o) {
                    self.dec_ref(&cid);
                    self.edges.insert_outer(o, cid);
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
    pub fn get_mineable_transactions(&self, n: usize) -> Vec<Transaction> {
        self.vertices
            .front_n(n)
            .into_iter()
            .map(|x| x.1.transaction.clone())
            .collect()
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
#[derive(Default, Debug)]
pub struct Orphan {
    pub vertices: FnvHashMap<ProposalShortId, PoolEntry>,
    pub edges: FnvHashMap<OutPoint, Vec<ProposalShortId>>,
}

impl Orphan {
    pub fn new() -> Self {
        Orphan::default()
    }

    //TODO: size
    pub fn size(&self) -> usize {
        self.vertices.len()
    }

    pub fn get(&self, id: &ProposalShortId) -> Option<&Transaction> {
        self.vertices.get(id).map(|x| &x.transaction)
    }

    pub fn contains(&self, tx: &Transaction) -> bool {
        self.vertices.contains_key(&tx.proposal_short_id())
    }

    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.vertices.contains_key(id)
    }

    /// add orphan transaction
    pub fn add_transaction(&mut self, tx: Transaction, unknown: impl Iterator<Item = OutPoint>) {
        let id = tx.proposal_short_id();

        let mut count: usize = 0;

        for o in unknown {
            let e = self.edges.entry(o).or_insert_with(Vec::new);
            e.push(id);
            count += 1;
        }

        self.vertices.insert(id, PoolEntry::new(tx, count));
    }

    pub fn remove(&mut self, id: &ProposalShortId) -> Option<Transaction> {
        if let Some(x) = self.vertices.remove(id) {
            let tx = x.transaction;

            // should remove its children?
            // for o in tx.output_pts() {
            //     if let Some(ids) = self.edges.remove(&o) {
            //         for cid in ids {
            //             self.remove(&cid);
            //         }
            //     }
            // }

            Some(tx)
        } else {
            None
        }
    }

    pub fn reconcile_transaction(&mut self, tx: &Transaction) -> Vec<Transaction> {
        let mut txs = Vec::new();
        let mut q = VecDeque::new();

        self.resolve_conflict(tx);

        q.push_back(tx.output_pts());
        while let Some(outputs) = q.pop_front() {
            for o in outputs {
                if let Some(ids) = self.edges.remove(&o) {
                    for cid in ids {
                        if let Some(mut x) = self.vertices.remove(&cid) {
                            x.refs_count -= 1;
                            if x.refs_count == 0 {
                                q.push_back(x.transaction.output_pts());
                                txs.push(x.transaction);
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

    pub fn resolve_conflict(&mut self, tx: &Transaction) {
        let inputs = tx.input_pts();

        for i in inputs {
            if let Some(ids) = self.edges.remove(&i) {
                for cid in ids {
                    self.remove(&cid);
                }
            }
        }
    }
}

#[derive(Default, Debug)]
pub struct PendingQueue {
    inner: FnvHashMap<ProposalShortId, Transaction>,
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

    pub fn insert(&mut self, id: ProposalShortId, tx: Transaction) -> Option<Transaction> {
        self.inner.insert(id, tx)
    }

    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.inner.contains_key(id)
    }

    pub fn get(&self, id: &ProposalShortId) -> Option<&Transaction> {
        self.inner.get(id)
    }

    pub fn remove(&mut self, id: &ProposalShortId) -> Option<Transaction> {
        self.inner.remove(id)
    }

    pub fn fetch(&self, n: usize) -> Vec<ProposalShortId> {
        self.inner
            .values()
            .take(n)
            .map(|x| x.proposal_short_id())
            .collect()
    }
}

#[derive(Default, Debug)]
pub struct ProposedQueue {
    //the blocknumber at the back of the queue
    tip: BlockNumber,
    queue: VecDeque<FnvHashSet<ProposalShortId>>,
    numbers: FnvHashMap<ProposalShortId, BlockNumber>,
    buff: FnvHashMap<ProposalShortId, Transaction>,
}

impl ProposedQueue {
    pub fn size(&self) -> usize {
        self.buff.len()
    }

    pub fn cap() -> usize {
        (TRANSACTION_PROPAGATION_TIME + BUFF_QUE_LEN) as usize
    }

    pub fn new(n: BlockNumber, ids_list: Vec<Vec<ProposalShortId>>) -> Self {
        let tip = n;
        let cap = (TRANSACTION_PROPAGATION_TIME + BUFF_QUE_LEN) as usize;
        let mut queue = VecDeque::with_capacity(cap as usize + 1);
        let mut numbers = FnvHashMap::default();
        let tail = if TRANSACTION_PROPAGATION_TIMEOUT > tip {
            1
        } else {
            tip + 1 - TRANSACTION_PROPAGATION_TIMEOUT
        };
        let mut cur = tip;

        for ids in ids_list {
            let id_set: FnvHashSet<ProposalShortId> = ids
                .into_iter()
                .map(|id| {
                    if cur >= tail {
                        numbers.insert(id, cur);
                    }
                    id
                }).collect();

            cur -= 1;
            queue.push_front(id_set);
        }

        for _ in queue.len()..cap {
            queue.push_front(FnvHashSet::default());
        }

        let buff = FnvHashMap::default();

        ProposedQueue {
            tip,
            queue,
            numbers,
            buff,
        }
    }

    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.buff.contains_key(id)
    }

    pub fn get_ids(&self, bn: BlockNumber) -> Option<&FnvHashSet<ProposalShortId>> {
        if self.tip < bn {
            return None;
        }

        if bn + self.queue.len() as u64 <= self.tip {
            return None;
        }

        let id = (self.queue.len() as u64 + bn - self.tip - 1) as usize;

        self.queue.get(id)
    }

    pub fn insert(&mut self, tx: Transaction) -> TxStage {
        let id = tx.proposal_short_id();
        if let Some(bn) = self.numbers.get(&id) {
            if bn + TRANSACTION_PROPAGATION_TIME > self.tip + 1 {
                self.buff.insert(id, tx);
                TxStage::Proposed
            } else {
                TxStage::Mineable(tx)
            }
        } else {
            TxStage::Unknown(tx)
        }
    }

    pub fn insert_with_n(&mut self, bn: BlockNumber, tx: Transaction) -> TxStage {
        if bn <= self.tip {
            if bn + TRANSACTION_PROPAGATION_TIMEOUT <= self.tip {
                TxStage::TimeOut(tx)
            } else {
                let mut is_in = false;
                let id = tx.proposal_short_id();

                if let Some(ids) = self.get_ids(bn) {
                    if ids.contains(&id) {
                        is_in = true;
                    }
                }

                if is_in {
                    if bn + TRANSACTION_PROPAGATION_TIME > self.tip + 1 {
                        self.buff.insert(id, tx);
                        TxStage::Proposed
                    } else {
                        TxStage::Mineable(tx)
                    }
                } else {
                    TxStage::Fork(tx)
                }
            }
        } else {
            TxStage::Fork(tx)
        }
    }

    pub fn insert_without_check(&mut self, id: ProposalShortId, tx: Transaction) {
        self.buff.insert(id, tx);
    }

    pub fn push_back(&mut self, ids: Vec<ProposalShortId>) {
        let id_set: FnvHashSet<ProposalShortId> = ids
            .into_iter()
            .map(|id| {
                self.numbers.insert(id, self.tip + 1);
                id
            }).collect();

        if TRANSACTION_PROPAGATION_TIMEOUT <= self.tip + 1 {
            let tail = self.tip + 1 - TRANSACTION_PROPAGATION_TIMEOUT;
            if let Some(ids) = self.get_ids(tail).cloned() {
                for id in ids {
                    self.numbers.remove(&id);
                }
            }
        }

        self.queue.pop_front();
        self.queue.push_back(id_set);
        self.tip += 1;
    }

    pub fn pop_back(&mut self) -> Option<FnvHashSet<ProposalShortId>> {
        if self.tip == 0 {
            return None;
        }

        let r = self.queue.pop_back();
        self.queue.push_front(FnvHashSet::default());
        self.tip -= 1;

        if let Some(ref ids) = r {
            for id in ids {
                self.numbers.remove(id);
            }
        }

        if TRANSACTION_PROPAGATION_TIMEOUT <= self.tip + 1 {
            let tail = self.tip + 1 - TRANSACTION_PROPAGATION_TIMEOUT;
            if let Some(ids) = self.get_ids(tail).cloned() {
                for id in ids {
                    self.numbers.insert(id, tail);
                }
            }
        }

        r
    }

    pub fn reconcile(
        &mut self,
        bn: BlockNumber,
        ids: Vec<ProposalShortId>,
    ) -> Result<Vec<Transaction>, PoolError> {
        if bn < TRANSACTION_PROPAGATION_TIME {
            self.push_back(ids);
            return Ok(Vec::new());
        }

        if self.tip + 1 != bn {
            return Err(PoolError::InvalidBlockNumber);
        }

        let m = bn + 1 - TRANSACTION_PROPAGATION_TIME;
        self.push_back(ids);

        if let Some(x) = self.get_ids(m).cloned() {
            let r: Vec<Transaction> = x.iter().filter_map(|i| self.buff.remove(i)).collect();
            Ok(r)
        } else {
            Ok(Vec::new())
        }
    }

    pub fn get(&self, id: &ProposalShortId) -> Option<&Transaction> {
        self.buff.get(id)
    }

    pub fn remove(
        &mut self,
        bn: BlockNumber,
    ) -> Option<FnvHashMap<ProposalShortId, Option<Transaction>>> {
        if self.tip < bn {
            return None;
        }

        let mut txs = FnvHashMap::default();

        while self.tip >= bn {
            if let Some(ids) = self.pop_back() {
                for id in ids {
                    let v = self.buff.remove(&id);
                    txs.insert(id, v);
                }
            }
        }

        Some(txs)
    }

    // The oldest proposed shortids but still not mineable
    pub fn front(&self) -> Option<&FnvHashSet<ProposalShortId>> {
        if self.tip < TRANSACTION_PROPAGATION_TIME || TRANSACTION_PROPAGATION_TIME <= 1 {
            return None;
        }

        self.get_ids(self.tip + 2 - TRANSACTION_PROPAGATION_TIME)
    }

    // The oldest mineable shortids
    pub fn mineable_front(&self) -> Option<&FnvHashSet<ProposalShortId>> {
        if self.tip < TRANSACTION_PROPAGATION_TIMEOUT {
            return None;
        }

        let t = self.tip + 1 - TRANSACTION_PROPAGATION_TIMEOUT;
        self.get_ids(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bigint::H256;
    use core::transaction::{CellInput, CellOutput, Transaction, TransactionBuilder};

    fn build_tx(inputs: Vec<(H256, u32)>, outputs_len: usize) -> Transaction {
        TransactionBuilder::default()
            .inputs(
                inputs
                    .into_iter()
                    .map(|(txid, index)| {
                        CellInput::new(OutPoint::new(txid, index), Default::default())
                    }).collect(),
            ).outputs(
                (0..outputs_len)
                    .map(|i| CellOutput::new((i + 1) as u64, Vec::new(), H256::from(0), None))
                    .collect(),
            ).build()
    }

    #[test]
    fn test_proposed_queue() {
        let tx1 = build_tx(vec![(H256::zero(), 1), (H256::zero(), 2)], 1);
        let tx1_hash = tx1.hash();
        let tx2 = build_tx(vec![(tx1_hash, 0)], 1);
        let tx3 = build_tx(vec![(H256::zero(), 1)], 2);

        let id1 = tx1.proposal_short_id();
        let id2 = tx2.proposal_short_id();
        let id3 = tx3.proposal_short_id();

        let mut queue = ProposedQueue::new(1000, vec![vec![id2.clone()], vec![id1.clone()]]);

        let set1 = queue.get_ids(1000).unwrap().clone();
        let set2 = queue.get_ids(999).unwrap().clone();
        let set3 = queue.get_ids(990).unwrap().clone();

        assert_eq!(1, set1.len());
        assert_eq!(1, set2.len());
        assert_eq!(0, set3.len());

        assert!(set1.contains(&id2));
        assert!(set2.contains(&id1));

        queue.insert_without_check(id3.clone(), tx3.clone());

        let txs = queue.reconcile(1001, vec![id3]).unwrap();

        // if TRANSACTION_PROPAGATION_TIME = 1:
        assert_eq!(txs, vec![tx3]);

        let set1 = queue.get_ids(1000).unwrap().clone();
        assert_eq!(1, set1.len());
        assert!(set1.contains(&id2));

        assert_eq!(Some(&999), queue.numbers.get(&id1));
        assert_eq!(Some(&1000), queue.numbers.get(&id2));
        assert_eq!(Some(&1001), queue.numbers.get(&id3));
    }

    #[test]
    fn test_add_entry() {
        let tx1 = build_tx(vec![(H256::zero(), 1), (H256::zero(), 2)], 1);
        let tx1_hash = tx1.hash();
        let tx2 = build_tx(vec![(tx1_hash, 0)], 1);

        let mut pool = Pool::new();
        let id1 = tx1.proposal_short_id();
        let id2 = tx2.proposal_short_id();

        pool.add_transaction(tx1.clone());
        pool.add_transaction(tx2.clone());

        assert_eq!(pool.vertices.len(), 2);
        assert_eq!(pool.edges.inner_len(), 2);
        assert_eq!(pool.edges.outer_len(), 2);

        assert_eq!(pool.get_entry(&id1).unwrap().refs_count, 0);
        assert_eq!(pool.get_entry(&id2).unwrap().refs_count, 1);

        pool.commit_transaction(&tx1);
        assert_eq!(pool.edges.inner_len(), 1);
        assert_eq!(pool.edges.outer_len(), 1);

        assert_eq!(pool.get_entry(&id2).unwrap().refs_count, 0);
    }

    #[test]
    fn test_add_roots() {
        let tx1 = build_tx(vec![(H256::zero(), 1), (H256::zero(), 2)], 1);
        let tx2 = build_tx(vec![(H256::from(2), 1), (H256::from(3), 2)], 3);

        let mut pool = Pool::new();

        let id1 = tx1.proposal_short_id();
        let id2 = tx2.proposal_short_id();

        pool.add_transaction(tx1.clone());
        pool.add_transaction(tx2.clone());

        assert_eq!(pool.get_entry(&id1).unwrap().refs_count, 0);
        assert_eq!(pool.get_entry(&id2).unwrap().refs_count, 0);
        assert_eq!(pool.edges.inner_len(), 4);
        assert_eq!(pool.edges.outer_len(), 4);

        let mut mineable = pool.get_mineable_transactions(0);
        assert_eq!(0, mineable.len());

        mineable = pool.get_mineable_transactions(1);
        assert_eq!(1, mineable.len());
        assert!(mineable.contains(&tx1));

        mineable = pool.get_mineable_transactions(2);
        assert_eq!(2, mineable.len());
        assert!(mineable.contains(&tx1) && mineable.contains(&tx2));

        mineable = pool.get_mineable_transactions(3);
        assert_eq!(2, mineable.len());
        assert!(mineable.contains(&tx1) && mineable.contains(&tx2));

        pool.commit_transaction(&tx1);

        assert_eq!(pool.edges.inner_len(), 3);
        assert_eq!(pool.edges.outer_len(), 2);
    }

    #[test]
    fn test_pending_queue() {
        let mut pending = PendingQueue::new();

        for i in 0..20 {
            let tx = build_tx(vec![(H256::zero(), i), (H256::zero(), i + 20)], 2);

            pending.insert(tx.proposal_short_id(), tx);
        }

        assert_eq!(pending.size(), 20);
    }

    #[test]
    fn test_add_no_roots() {
        let tx1 = build_tx(vec![(H256::zero(), 1)], 3);
        let tx2 = build_tx(vec![], 4);
        let tx1_hash = tx1.hash();
        let tx2_hash = tx2.hash();

        let tx3 = build_tx(vec![(tx1_hash, 0), (H256::zero(), 2)], 2);
        let tx4 = build_tx(vec![(tx1_hash, 1), (tx2_hash, 0)], 2);

        let tx3_hash = tx3.hash();
        let tx5 = build_tx(vec![(tx1_hash, 2), (tx3_hash, 0)], 2);

        let id1 = tx1.proposal_short_id();
        let id3 = tx3.proposal_short_id();
        let id5 = tx5.proposal_short_id();

        let mut pool = Pool::new();

        pool.add_transaction(tx1.clone());
        pool.add_transaction(tx2.clone());
        pool.add_transaction(tx3.clone());
        pool.add_transaction(tx4.clone());
        pool.add_transaction(tx5.clone());

        assert_eq!(pool.get_entry(&id1).unwrap().refs_count, 0);
        assert_eq!(pool.get_entry(&id3).unwrap().refs_count, 1);
        assert_eq!(pool.get_entry(&id5).unwrap().refs_count, 2);
        assert_eq!(pool.edges.inner_len(), 13);
        assert_eq!(pool.edges.outer_len(), 2);

        let mut mineable = pool.get_mineable_transactions(0);
        assert_eq!(0, mineable.len());

        mineable = pool.get_mineable_transactions(1);
        assert_eq!(1, mineable.len());
        assert!(mineable.contains(&tx1));

        mineable = pool.get_mineable_transactions(2);
        assert_eq!(2, mineable.len());
        assert!(mineable.contains(&tx1) && mineable.contains(&tx2));

        mineable = pool.get_mineable_transactions(3);
        assert_eq!(3, mineable.len());
        assert!(mineable.contains(&tx1) && mineable.contains(&tx2) && mineable.contains(&tx3));

        mineable = pool.get_mineable_transactions(4);
        assert_eq!(4, mineable.len());
        assert!(mineable.contains(&tx1) && mineable.contains(&tx2));
        assert!(mineable.contains(&tx3) && mineable.contains(&tx4));

        mineable = pool.get_mineable_transactions(5);
        assert_eq!(5, mineable.len());
        assert!(mineable.contains(&tx1) && mineable.contains(&tx2));
        assert!(mineable.contains(&tx3) && mineable.contains(&tx4));
        assert!(mineable.contains(&tx5));

        mineable = pool.get_mineable_transactions(6);
        assert_eq!(5, mineable.len());
        assert!(mineable.contains(&tx1) && mineable.contains(&tx2));
        assert!(mineable.contains(&tx3) && mineable.contains(&tx4));
        assert!(mineable.contains(&tx5));

        pool.commit_transaction(&tx1);

        assert_eq!(pool.get_entry(&id3).unwrap().refs_count, 0);
        assert_eq!(pool.get_entry(&id5).unwrap().refs_count, 1);
        assert_eq!(pool.edges.inner_len(), 10);
        assert_eq!(pool.edges.outer_len(), 4);

        mineable = pool.get_mineable_transactions(1);
        assert_eq!(1, mineable.len());
        assert!(mineable.contains(&tx2));

        mineable = pool.get_mineable_transactions(2);
        assert_eq!(2, mineable.len());
        assert!(mineable.contains(&tx2) && mineable.contains(&tx3));

        mineable = pool.get_mineable_transactions(3);
        assert_eq!(3, mineable.len());
        assert!(mineable.contains(&tx2) && mineable.contains(&tx3));
        assert!(mineable.contains(&tx4));

        mineable = pool.get_mineable_transactions(4);
        assert_eq!(4, mineable.len());
        assert!(mineable.contains(&tx2) && mineable.contains(&tx3));
        assert!(mineable.contains(&tx4) && mineable.contains(&tx5));

        mineable = pool.get_mineable_transactions(5);
        assert_eq!(4, mineable.len());
    }
}
