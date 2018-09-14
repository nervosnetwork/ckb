//! The primary module containing the implementations of the transaction pool
//! and its top-level members.

use bigint::H256;
use ckb_verification::TransactionError;
use core::transaction::{
    CellOutput, IndexedTransaction, OutPoint, ProposalShortId, ProposalTransaction,
};
use core::BlockNumber;
use fnv::{FnvHashMap, FnvHashSet};
use std::collections::btree_map;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::iter::Iterator;
use std::mem;
use std::ops::Range;
use time;

/// Transaction pool configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PoolConfig {
    /// Maximum capacity of the pool in number of transactions
    pub max_pool_size: usize,
    pub max_mining_size: usize,
}

/// This enum describes the status of a transaction's outpoint.
#[derive(Clone, Debug, PartialEq)]
pub enum OutPointStatus {
    Unknown,
    InOrphan,
    Spent,
}

#[derive(Clone, Debug)]
pub enum InsertionResult {
    Normal,
    Orphan,
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
    DoubleSpend,
    /// Transaction pool is over capacity, can't accept more transactions
    OverCapacity,
    /// A duplicate output
    DuplicateOutput,
    ///ConflictOrphan
    ConflictOrphan,
    /// Coinbase transaction
    CellBase,
}

#[derive(Default, Debug)]
pub struct CandidatePool {
    inner: FnvHashSet<ProposalTransaction>,
}

impl CandidatePool {
    pub fn new() -> CandidatePool {
        CandidatePool::default()
    }

    pub fn with_capacity(capacity: usize) -> CandidatePool {
        CandidatePool {
            inner: FnvHashSet::with_capacity_and_hasher(capacity, Default::default()),
        }
    }

    pub fn size(&self) -> usize {
        self.inner.len()
    }

    pub fn insert(&mut self, tx: IndexedTransaction) -> bool {
        self.inner.insert(tx.into())
    }

    pub fn take(&self, n: usize) -> FnvHashSet<ProposalTransaction> {
        if n > self.inner.len() {
            self.inner.clone()
        } else {
            self.inner.iter().take(n).cloned().collect()
        }
    }

    pub fn update_difference(&mut self, remove: &FnvHashSet<ProposalTransaction>) {
        self.inner = self.inner.difference(&remove).cloned().collect();
    }

    pub fn remove(&mut self, remove: &ProposalTransaction) {
        self.inner.remove(remove);
    }
}

#[derive(Default, Debug)]
pub struct ProposalPool {
    inner: BTreeMap<BlockNumber, FnvHashMap<ProposalShortId, IndexedTransaction>>,
}

impl ProposalPool {
    pub fn new() -> ProposalPool {
        ProposalPool::default()
    }

    pub fn insert(
        &mut self,
        block_number: BlockNumber,
        txs: impl Iterator<Item = ProposalTransaction>,
    ) {
        match self.inner.entry(block_number) {
            btree_map::Entry::Vacant(v) => {
                v.insert(txs.map(|ptx| ptx.into_pair()).collect());
            }
            btree_map::Entry::Occupied(mut o) => {
                o.get_mut().extend(txs.map(|ptx| ptx.into_pair()));
            }
        }
    }

    pub fn query(
        &self,
        block_number: BlockNumber,
        filter: impl Iterator<Item = ProposalShortId>,
    ) -> Option<(Vec<IndexedTransaction>, Vec<ProposalShortId>)> {
        self.inner.get(&block_number).map(|txs_map| {
            filter.fold((vec![], vec![]), |mut acc, id| {
                if let Some(tx) = txs_map.get(&id) {
                    acc.0.push(tx.clone());
                } else {
                    acc.1.push(id);
                }
                acc
            })
        })
    }

    pub fn query_ids(&self, block_number: BlockNumber) -> Option<FnvHashSet<ProposalShortId>> {
        self.inner
            .get(&block_number)
            .map(|txs_map| txs_map.keys().cloned().collect())
    }

    //Panics if range start > end. Panics if range start == end and both bounds are Excluded.
    pub fn take(&self, range: Range<BlockNumber>) -> FnvHashSet<ProposalTransaction> {
        let mut proposals = FnvHashSet::default();
        for (_, txs) in self.inner.range(range) {
            proposals.extend(
                txs.clone()
                    .into_iter()
                    .map(|(k, v)| ProposalTransaction::new(k, v)),
            );
        }
        proposals
    }

    pub fn clean(&mut self, block_number: BlockNumber) {
        let mut retain = self.inner.split_off(&block_number);
        mem::swap(&mut retain, &mut self.inner);
    }
}

pub struct CommitPool {
    pub pool: DirectedGraph,
}

impl Default for CommitPool {
    fn default() -> Self {
        Self::new()
    }
}

impl CommitPool {
    pub fn new() -> Self {
        CommitPool {
            pool: DirectedGraph::new(),
        }
    }

    pub fn is_pool_tx(&self, h: &H256) -> bool {
        self.pool.is_pool_tx(h)
    }

    pub fn get_output(&self, o: &OutPoint) -> Option<CellOutput> {
        self.pool.get_output(o)
    }

    /// Add a verified transaction.
    pub fn add_transaction(&mut self, tx: IndexedTransaction) {
        self.pool.add_transaction(tx);
    }

    /// Readd a verified transaction which is rolled back from chain. Since the rolled back
    /// transaction should depend on any transaction in the pool, it is safe to skip some checking.
    pub fn readd_transaction(&mut self, tx: &IndexedTransaction) {
        self.pool.readd_transaction(&tx);
    }

    /// When the transaction related to the vertex was packaged, we remove it.
    pub fn commit_transaction(&mut self, tx: &IndexedTransaction) -> bool {
        let hash = tx.hash();

        // only roots can be removed
        if self.pool.roots.remove(&hash).is_some() {
            self.pool.reconcile_transaction(tx);
            true
        } else {
            false
        }
    }

    pub fn resolve_conflict(&mut self, tx: &IndexedTransaction) {
        self.pool.resolve_conflict(tx);
    }

    /// Currently a single rule for miner preference -
    /// return all txs if less than `n` txs in the entire pool
    /// otherwise return `n` of just the roots
    pub fn get_mineable_transactions(&self, n: usize) -> Vec<IndexedTransaction> {
        if self.size() <= n {
            self.pool.get_vertices()
        } else {
            self.pool.get_roots(n)
        }
    }

    pub fn outpoint_status(&self, o: &OutPoint) -> OutPointStatus {
        self.pool
            .out_edges
            .get(o)
            .map(|_| OutPointStatus::Spent)
            .or_else(|| {
                self.pool.edges.get(o).map(|x| match *x {
                    Some(_) => OutPointStatus::Spent,
                    None => OutPointStatus::InOrphan,
                })
            }).unwrap_or(OutPointStatus::Unknown)
    }

    pub fn size(&self) -> usize {
        self.pool.len_vertices()
    }
}

pub struct OrphanPool {
    pub pool: DirectedGraph,
}

impl Default for OrphanPool {
    fn default() -> Self {
        Self::new()
    }
}

impl OrphanPool {
    pub fn new() -> Self {
        OrphanPool {
            pool: DirectedGraph::new(),
        }
    }

    pub fn is_pool_tx(&self, h: &H256) -> bool {
        self.pool.is_pool_tx(h)
    }

    pub fn get_output(&self, o: &OutPoint) -> Option<CellOutput> {
        self.pool.get_output(o)
    }

    /// add orphan transaction
    pub fn add_transaction(
        &mut self,
        tx: IndexedTransaction,
        unknown: impl Iterator<Item = OutPoint>,
    ) {
        for o in unknown {
            self.pool.edges.insert(o, None);
            self.pool.dep_edges.insert(o, FnvHashSet::default());
        }

        self.pool.add_transaction(tx);
    }

    pub fn outpoint_status(&self, o: &OutPoint) -> OutPointStatus {
        self.pool
            .out_edges
            .get(o)
            .map(|_| OutPointStatus::Spent)
            .or_else(|| {
                self.pool.edges.get(o).map(|x| match *x {
                    Some(_) => OutPointStatus::Spent,
                    None => OutPointStatus::InOrphan,
                })
            }).unwrap_or(OutPointStatus::Unknown)
    }

    /// when a transaction is added in pool or chain, reconcile it.
    pub fn commit_transaction(&mut self, tx: &IndexedTransaction) {
        self.pool.reconcile_transaction(tx);
    }

    pub fn resolve_conflict(&mut self, tx: &IndexedTransaction) {
        self.pool.resolve_conflict(tx);
    }

    pub fn get_no_orphan(&mut self) -> Vec<IndexedTransaction> {
        let mut txs = Vec::new();

        loop {
            let tmp = self.pool.get_roots(self.pool.len_roots());

            if tmp.is_empty() {
                break;
            }

            self.pool.roots = FnvHashMap::default();

            for tx in &tmp {
                self.pool.reconcile_transaction(tx);
            }

            txs.extend(tmp);
        }

        txs
    }

    pub fn size(&self) -> usize {
        self.pool.len_vertices()
    }
}

/// An entry in the transaction pool.
#[derive(Debug, PartialEq, Clone)]
pub struct PoolEntry {
    /// Transaction
    pub transaction: IndexedTransaction,
    /// refs count
    pub refs_count: u64,
    /// Size estimate
    pub size_estimate: u64,
    /// Receive timestamp
    pub receive_ts: u64,
}

impl PoolEntry {
    /// Create new transaction pool entry
    pub fn new(tx: IndexedTransaction, count: u64) -> PoolEntry {
        PoolEntry {
            size_estimate: estimate_transaction_size(&tx),
            transaction: tx,
            refs_count: count,
            receive_ts: time::now_ms(),
        }
    }
}

/// TODO guessing this needs implementing
fn estimate_transaction_size(_tx: &IndexedTransaction) -> u64 {
    0
}

/// The generic graph container. Both graphs, the pool and orphans, embed this
/// structure and add additional capability on top of it.
#[derive(Debug, PartialEq, Clone, Default)]
pub struct DirectedGraph {
    /// Transactions which dependencies are not in the graph.
    roots: FnvHashMap<H256, PoolEntry>,
    /// Transactions which has at least a dependency in the graph.
    no_roots: FnvHashMap<H256, PoolEntry>,
    /// Keys are OutPoints pointing to transactions that are in the graph.
    ///
    /// The value is the hash of transaction in the graph if it is not none. The transaction
    /// contains the key as one of its inputs.
    edges: FnvHashMap<OutPoint, Option<H256>>,
    /// Keys are OutPoints pointing to transactions that are not in the graph(on chain).
    ///
    /// The value is the hash of transaction in the graph if it is not none. The transaction
    /// contains the key as one of its inputs.
    out_edges: FnvHashMap<OutPoint, Option<H256>>,

    ///Keys are Dependents OutPoints pointing to transactions that are in the graph.
    dep_edges: FnvHashMap<OutPoint, FnvHashSet<H256>>,

    ///Keys are Dependents OutPoints pointing to transactions that are not in the graph(on chain).
    out_dep_edges: FnvHashMap<OutPoint, FnvHashSet<H256>>,
}

impl DirectedGraph {
    /// Create an empty directed graph
    pub fn new() -> DirectedGraph {
        DirectedGraph::default()
    }

    /// Get an edge's destnation(tx hash) by OutPoint
    fn get_edge(&self, o: &OutPoint) -> Option<H256> {
        self.edges.get(o).and_then(|x| *x)
    }

    pub fn get_output(&self, o: &OutPoint) -> Option<CellOutput> {
        (self
            .roots
            .get(&o.hash)
            .or_else(|| self.no_roots.get(&o.hash)))
            .and_then(|x| x.transaction.outputs.get(o.index as usize).cloned())
    }

    /// Remove an edge by OutPoint
    fn remove_edge(&mut self, o: &OutPoint) -> Option<H256> {
        self.edges.remove(o).unwrap_or(None)
    }

    /// Remove an out edge by OutPoint
    fn remove_out_edge(&mut self, o: &OutPoint) -> Option<H256> {
        self.out_edges.remove(o).unwrap_or(None)
    }

    pub fn is_pool_tx(&self, h: &H256) -> bool {
        self.roots.contains_key(h) || self.no_roots.contains_key(h)
    }

    pub fn readd_transaction(&mut self, tx: &IndexedTransaction) {
        let inputs = tx.input_pts();
        let outputs = tx.output_pts();
        let deps = tx.dep_pts();
        let h = tx.hash();

        for i in inputs {
            self.out_edges.insert(i, Some(h));
        }

        for d in deps {
            let e = self
                .out_dep_edges
                .entry(d)
                .or_insert_with(FnvHashSet::default);
            e.insert(h);
        }

        for o in outputs {
            if let Some(h) = self.remove_out_edge(&o) {
                if let Some(mut x) = self.roots.remove(&h) {
                    x.refs_count += 1;
                    self.no_roots.insert(h, x);
                } else if let Some(x) = self.no_roots.get_mut(&h) {
                    x.refs_count += 1;
                }
                self.edges.insert(o, Some(h));
            } else if let Some(hs) = self.out_dep_edges.remove(&o) {
                for h in hs.clone() {
                    if let Some(mut x) = self.roots.remove(&h) {
                        x.refs_count += 1;
                        self.no_roots.insert(h, x);
                    } else if let Some(x) = self.no_roots.get_mut(&h) {
                        x.refs_count += 1;
                    }
                }
                self.dep_edges.insert(o, hs);
            } else {
                self.edges.insert(o, None);
                self.dep_edges.insert(o, FnvHashSet::default());
            }
        }
    }

    /// add a verified transaction
    pub fn add_transaction(&mut self, tx: IndexedTransaction) {
        let inputs = tx.input_pts();
        let outputs = tx.output_pts();
        let deps = tx.dep_pts();

        let h = tx.hash();

        let mut count: u64 = 0;

        for i in inputs {
            if let Some(x) = { self.dep_edges.get(&i).cloned() } {
                for h in x {
                    self.remove_vertex(&h);
                }
            }

            if let Some(x) = self.edges.get_mut(&i) {
                *x = Some(h);
                count += 1;
            } else {
                self.out_edges.insert(i, Some(h));
            }
        }

        for d in deps {
            if let Some(x) = self.dep_edges.get_mut(&d) {
                x.insert(h);
                count += 1;
            } else {
                let e = self
                    .out_dep_edges
                    .entry(d)
                    .or_insert_with(FnvHashSet::default);
                e.insert(h);
            }
        }

        for o in outputs {
            self.edges.entry(o).or_insert(None);
            self.dep_edges.entry(o).or_insert_with(FnvHashSet::default);
        }

        if count == 0 {
            self.roots.insert(h, PoolEntry::new(tx, count));
        } else {
            self.no_roots.insert(h, PoolEntry::new(tx, count));
        }
    }

    fn reconcile_transaction(&mut self, tx: &IndexedTransaction) {
        let outputs = tx.output_pts();
        let inputs = tx.input_pts();
        let deps = tx.dep_pts();
        let tx_hash = tx.hash();

        for o in outputs {
            if let Some(h) = self.remove_edge(&o) {
                self.dec_ref(&h);
                self.out_edges.insert(o, Some(h));
            }

            if let Some(x) = self.dep_edges.remove(&o) {
                for h in x.clone() {
                    self.dec_ref(&h);
                }

                self.out_dep_edges.insert(o, x);
            }
        }

        for i in inputs {
            self.out_edges.remove(&i);
        }

        for d in deps {
            let mut empty = false;
            if let Some(x) = self.out_dep_edges.get_mut(&d) {
                x.remove(&tx_hash);
                empty = x.is_empty();
            };

            if empty {
                self.out_dep_edges.remove(&d);
            }
        }
    }

    fn resolve_conflict(&mut self, tx: &IndexedTransaction) {
        let inputs = tx.input_pts();

        for i in inputs {
            if let Some(h) = self.remove_out_edge(&i) {
                self.remove_vertex(&h);
            }

            if let Some(x) = self.out_dep_edges.remove(&i) {
                for h in x {
                    self.remove_vertex(&h);
                }
            }
        }

        //we don't need resolve deps becasue tx is executed first.
    }

    /// when the transaction's input is used by other transaction, we remove it.
    fn remove_vertex(&mut self, h: &H256) {
        if let Some(x) = self.no_roots.remove(h).or_else(|| self.roots.remove(h)) {
            let tx = x.transaction;

            for i in tx.input_pts() {
                //TODO: remove blockchain ref
                if let Some(x) = self.edges.get_mut(&i) {
                    *x = None;
                } else {
                    self.out_edges.remove(&i);
                }
            }

            for d in tx.dep_pts() {
                let mut empty = false;

                if let Some(x) = self.dep_edges.get_mut(&d) {
                    x.remove(&h);
                } else if let Some(x) = self.out_dep_edges.get_mut(&d) {
                    x.remove(&h);
                    empty = x.is_empty();
                }

                if empty {
                    self.out_dep_edges.remove(&d);
                }
            }

            for o in tx.output_pts() {
                if let Some(ch) = self.remove_edge(&o) {
                    self.remove_vertex(&ch);
                }

                if let Some(x) = self.dep_edges.remove(&o) {
                    for ch in x {
                        self.remove_vertex(&ch);
                    }
                }
            }
        }
    }

    /// dec vertex's pool output ref num
    fn dec_ref(&mut self, h: &H256) {
        let mut count = 1;
        if let Some(x) = self.no_roots.get_mut(h) {
            x.refs_count -= 1;
            count = x.refs_count;
        }

        if count == 0 {
            self.update_root(h);
        }
    }

    fn get_potential_root(
        &self,
        tx: &IndexedTransaction,
        counts: &mut HashMap<H256, u64>,
    ) -> Vec<IndexedTransaction> {
        let mut roots = Vec::new();
        let outputs = tx.output_pts();

        for o in outputs {
            if let Some(h) = self.get_edge(&o) {
                if let Some(x) = self.no_roots.get(&h) {
                    let c = counts.get(&h).map_or(1, |c| *c + 1);
                    if x.refs_count == c {
                        roots.push(x.transaction.clone());
                    } else {
                        counts.insert(h, c);
                    }
                }
            }

            if let Some(d) = self.dep_edges.get(&o) {
                for h in d {
                    if let Some(x) = self.no_roots.get(&h) {
                        let c = counts.get(&h).map_or(1, |c| *c + 1);
                        if x.refs_count == c {
                            roots.push(x.transaction.clone());
                        } else {
                            counts.insert(*h, c);
                        }
                    }
                }
            }
        }

        roots
    }

    /// move a poolentry from vertices to roots
    pub fn update_root(&mut self, h: &H256) {
        if let Some(x) = self.no_roots.remove(h) {
            self.roots.insert(*h, x);
        }
    }

    /// Number of vertices (root + internal)
    pub fn len_vertices(&self) -> usize {
        self.no_roots.len() + self.roots.len()
    }

    /// Number of root vertices only
    pub fn len_roots(&self) -> usize {
        self.roots.len()
    }

    /// Number of edges
    pub fn len_edges(&self) -> usize {
        self.edges.len()
    }

    /// Get the current list of roots
    #[cfg_attr(feature = "cargo-clippy", allow(while_let_loop))]
    pub fn get_roots(&self, n: usize) -> Vec<IndexedTransaction> {
        if self.roots.len() >= n {
            self.roots
                .values()
                .take(n)
                .map(|x| &x.transaction)
                .cloned()
                .collect()
        } else {
            let mut roots: Vec<IndexedTransaction> = self
                .roots
                .values()
                .map(|x| &x.transaction)
                .cloned()
                .collect();
            let mut counts = HashMap::new();
            let mut i = 0;
            let mut new;
            loop {
                if let Some(r) = roots.get(i) {
                    new = self.get_potential_root(&r, &mut counts);
                } else {
                    break;
                }

                roots.append(&mut new);
                if roots.len() >= n {
                    break;
                }
                i += 1;
            }

            if roots.len() > n {
                roots.split_off(n);
            }

            roots
        }
    }

    /// Get list of all vertices in this graph including the roots
    pub fn get_vertices(&self) -> Vec<IndexedTransaction> {
        self.roots
            .values()
            .map(|x| &x.transaction)
            .chain(self.no_roots.values().map(|x| &x.transaction))
            .cloned()
            .collect::<Vec<IndexedTransaction>>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::transaction::{CellInput, CellOutput, Transaction};

    fn build_tx(inputs: Vec<(H256, u32)>, outputs_len: usize) -> IndexedTransaction {
        Transaction::new(
            0,
            Vec::new(),
            inputs
                .into_iter()
                .map(|(txid, index)| CellInput::new(OutPoint::new(txid, index), Default::default()))
                .collect(),
            (0..outputs_len)
                .map(|i| CellOutput::new((i + 1) as u64, Vec::new(), H256::from(0)))
                .collect(),
        ).into()
    }

    #[test]
    fn test_add_entry() {
        let tx1 = build_tx(vec![(H256::zero(), 1), (H256::zero(), 2)], 1);
        let tx1_hash = tx1.hash();
        let tx2 = build_tx(vec![(tx1_hash, 0)], 1);

        let mut pool = CommitPool::new();

        pool.add_transaction(tx1.clone());
        pool.add_transaction(tx2.clone());

        assert_eq!(pool.pool.no_roots.len(), 1);
        assert_eq!(pool.pool.roots.len(), 1);
        assert_eq!(pool.pool.edges.len(), 2);

        pool.commit_transaction(&tx1);

        assert_eq!(pool.pool.roots.len(), 1);
        assert_eq!(pool.pool.no_roots.len(), 0);
    }

    #[test]
    fn test_add_roots() {
        let tx1 = build_tx(vec![(H256::zero(), 1), (H256::zero(), 2)], 1);
        let tx2 = build_tx(vec![(H256::from(2), 1), (H256::from(3), 2)], 3);

        let mut pool = CommitPool::new();

        pool.add_transaction(tx1.clone());
        pool.add_transaction(tx2.clone());

        assert_eq!(pool.pool.no_roots.len(), 0);
        assert_eq!(pool.pool.roots.len(), 2);
        assert_eq!(pool.pool.edges.len(), 4);
        assert_eq!(pool.pool.out_edges.len(), 4);

        let mut mineable = pool.get_mineable_transactions(0);
        assert_eq!(0, mineable.len());

        mineable = pool.get_mineable_transactions(1);
        assert_eq!(1, mineable.len());
        assert!(mineable.contains(&tx1) || mineable.contains(&tx2));

        mineable = pool.get_mineable_transactions(2);
        assert_eq!(2, mineable.len());
        assert!(mineable.contains(&tx1) && mineable.contains(&tx2));

        mineable = pool.get_mineable_transactions(3);
        assert_eq!(2, mineable.len());
        assert!(mineable.contains(&tx1) && mineable.contains(&tx2));

        pool.commit_transaction(&tx1);

        assert_eq!(pool.pool.no_roots.len(), 0);
        assert_eq!(pool.pool.roots.len(), 1);
        assert_eq!(pool.pool.edges.len(), 3);
        assert_eq!(pool.pool.out_edges.len(), 2);
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

        let mut pool = CommitPool::new();

        pool.add_transaction(tx1.clone());
        pool.add_transaction(tx2.clone());
        pool.add_transaction(tx3.clone());
        pool.add_transaction(tx4.clone());
        pool.add_transaction(tx5.clone());

        assert_eq!(pool.pool.no_roots.len(), 3);
        assert_eq!(pool.pool.roots.len(), 2);
        assert_eq!(pool.pool.edges.len(), 13);
        assert_eq!(pool.pool.out_edges.len(), 2);

        let mut mineable = pool.get_mineable_transactions(0);
        assert_eq!(0, mineable.len());

        mineable = pool.get_mineable_transactions(1);
        assert_eq!(1, mineable.len());
        assert!(mineable.contains(&tx1) || mineable.contains(&tx2));

        mineable = pool.get_mineable_transactions(2);
        assert_eq!(2, mineable.len());
        assert!(mineable.contains(&tx1) && mineable.contains(&tx2));

        mineable = pool.get_mineable_transactions(3);
        assert_eq!(3, mineable.len());
        assert!(mineable.contains(&tx1) && mineable.contains(&tx2));
        assert!(mineable.contains(&tx3) || mineable.contains(&tx4));

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

        assert_eq!(pool.pool.no_roots.len(), 2);
        assert_eq!(pool.pool.roots.len(), 2);
        assert_eq!(pool.pool.edges.len(), 10);
        assert_eq!(pool.pool.out_edges.len(), 4);

        mineable = pool.get_mineable_transactions(1);
        assert_eq!(1, mineable.len());
        assert!(mineable.contains(&tx2) || mineable.contains(&tx3));

        mineable = pool.get_mineable_transactions(2);
        assert_eq!(2, mineable.len());
        assert!(mineable.contains(&tx2) && mineable.contains(&tx3));

        mineable = pool.get_mineable_transactions(3);
        assert_eq!(3, mineable.len());
        assert!(mineable.contains(&tx2) && mineable.contains(&tx3));
        assert!(mineable.contains(&tx4) || mineable.contains(&tx5));

        mineable = pool.get_mineable_transactions(4);
        assert_eq!(4, mineable.len());
        assert!(mineable.contains(&tx2) && mineable.contains(&tx3));
        assert!(mineable.contains(&tx4) && mineable.contains(&tx5));

        mineable = pool.get_mineable_transactions(5);
        assert_eq!(4, mineable.len());
    }
}
