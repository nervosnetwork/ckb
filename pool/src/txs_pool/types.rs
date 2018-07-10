//! The primary module containing the implementations of the transaction pool
//! and its top-level members.
#![allow(unknown_lints)]
#![allow(while_let_loop)]

use std::collections::HashMap;
use std::iter::Iterator;

use bigint::H256;
use core::header::Header;
use core::transaction::{OutPoint, Transaction};
use nervos_verification::TransactionError;

use time;

const DEFAULT_MAX_POOL_SIZE: usize = 50_000;

/// Transaction pool configuration
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PoolConfig {
    /// Maximum capacity of the pool in number of transactions
    pub max_pool_size: usize,
}

impl Default for PoolConfig {
    fn default() -> PoolConfig {
        PoolConfig {
            max_pool_size: DEFAULT_MAX_POOL_SIZE,
        }
    }
}

/// This enum describes the parent for a given input of a transaction.
#[derive(Clone, Debug)]
pub enum Parent {
    Unknown,
    BlockTransaction,
    PoolTransaction,
    OrphanTransaction,
    AlreadySpent,
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
    /// orphan transaction
    OrphanTransaction,
    /// A duplicate output
    DuplicateOutput,
}

/// Interface that the pool requires from a blockchain implementation.
pub trait BlockChain: Send + Sync {
    /// Check the output is not spent
    fn is_spent(&self, output_ref: &OutPoint) -> Option<Parent>;

    /// Get the tip block header
    fn tip_header(&self) -> Option<Header>;
}

pub struct Pool {
    pub pool: DirectedGraph,
}

impl Default for Pool {
    fn default() -> Self {
        Self::new()
    }
}

impl Pool {
    pub fn new() -> Self {
        Pool {
            pool: DirectedGraph::new(),
        }
    }

    pub fn is_pool_tx(&self, h: &H256) -> bool {
        self.pool.is_pool_tx(h)
    }

    /// add a verified transaction
    pub fn add_transaction(&mut self, tx: Transaction) {
        self.pool.add_transaction(tx);
    }

    /// readd a verified transaction
    pub fn readd_transaction(&mut self, tx: &Transaction) {
        self.pool.readd_transaction(&tx);
    }

    /// when the transaction related to the vertex was packaged, we remove it.
    pub fn commit_transaction(&mut self, tx: &Transaction) -> bool {
        let hash = tx.hash();

        // only roots can be removed
        if self.pool.roots.remove(&hash).is_some() {
            self.pool.reconcile_transaction(tx);
            true
        } else {
            false
        }
    }

    pub fn resolve_conflict(&mut self, tx: &Transaction) {
        self.pool.resolve_conflict(tx);
    }

    /// Currently a single rule for miner preference -
    /// return all txs if less than `n` txs in the entire pool
    /// otherwise return `n` of just the roots
    pub fn get_mineable_transactions(&self, n: usize) -> Vec<Transaction> {
        if self.size() <= n {
            self.pool.get_vertices()
        } else {
            self.pool.get_roots(n)
        }
    }

    pub fn is_spent(&self, o: &OutPoint) -> Option<Parent> {
        if self.pool.out_edges.get(o).is_some() {
            return Some(Parent::AlreadySpent);
        }

        self.pool.edges.get(o).map(|x| match *x {
            Some(_) => Parent::AlreadySpent,
            None => Parent::PoolTransaction,
        })
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

    /// add orphan transaction
    pub fn add_transaction(&mut self, tx: Transaction, unknown: Vec<OutPoint>) {
        for o in unknown {
            self.pool.edges.insert(o, None);
        }

        self.pool.add_transaction(tx);
    }

    pub fn is_spent(&self, o: &OutPoint) -> Option<Parent> {
        if self.pool.out_edges.get(o).is_some() {
            return Some(Parent::AlreadySpent);
        }

        self.pool.edges.get(o).map(|x| match *x {
            Some(_) => Parent::AlreadySpent,
            None => Parent::OrphanTransaction,
        })
    }

    /// when a transaction is added in pool or chain, reconcile it.
    pub fn commit_transaction(&mut self, tx: &Transaction) {
        self.pool.reconcile_transaction(tx);
    }

    pub fn resolve_conflict(&mut self, tx: &Transaction) {
        self.pool.resolve_conflict(tx);
    }

    pub fn get_no_orphan(&mut self) -> Vec<Transaction> {
        let mut txs = Vec::new();

        loop {
            let tmp = self.pool.get_roots(self.pool.len_roots());

            if tmp.is_empty() {
                break;
            }

            self.pool.roots = HashMap::new();

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
    pub transaction: Transaction,
    /// refs count
    pub refs_count: u64,
    /// Size estimate
    pub size_estimate: u64,
    /// Receive timestamp
    pub receive_ts: u64,
}

impl PoolEntry {
    /// Create new transaction pool entry
    pub fn new(tx: Transaction, count: u64) -> PoolEntry {
        PoolEntry {
            size_estimate: estimate_transaction_size(&tx),
            transaction: tx,
            refs_count: count,
            receive_ts: time::now_ms(),
        }
    }
}

/// TODO guessing this needs implementing
fn estimate_transaction_size(_tx: &Transaction) -> u64 {
    0
}

/// The generic graph container. Both graphs, the pool and orphans, embed this
/// structure and add additional capability on top of it.
#[derive(Debug, PartialEq, Clone, Default)]
pub struct DirectedGraph {
    edges: HashMap<OutPoint, Option<H256>>,
    out_edges: HashMap<OutPoint, Option<H256>>,
    no_roots: HashMap<H256, PoolEntry>,

    // roots (vertices with in-degree 0, no pool reference)
    roots: HashMap<H256, PoolEntry>,
}

impl DirectedGraph {
    /// Create an empty directed graph
    pub fn new() -> DirectedGraph {
        DirectedGraph {
            edges: HashMap::new(),
            out_edges: HashMap::new(),
            no_roots: HashMap::new(),
            roots: HashMap::new(),
        }
    }

    /// Get an edge's destnation(tx hash) by output
    pub fn get_edge(&self, o: &OutPoint) -> Option<H256> {
        self.edges.get(o).and_then(|x| *x)
    }

    /// Remove an edge by output's hash
    pub fn remove_edge(&mut self, o: &OutPoint) -> Option<H256> {
        self.edges.remove(o).unwrap_or(None)
    }

    /// Remove an out edge by output's hash
    pub fn remove_out_edge(&mut self, o: &OutPoint) -> Option<H256> {
        self.out_edges.remove(o).unwrap_or(None)
    }

    pub fn is_pool_tx(&self, h: &H256) -> bool {
        self.roots.contains_key(h) || self.no_roots.contains_key(h)
    }

    //
    pub fn readd_transaction(&mut self, tx: &Transaction) {
        let inputs = tx.input_pts();
        let outputs = tx.output_pts();
        let h = tx.hash();

        for i in inputs {
            self.out_edges.insert(i, Some(h));
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
            } else {
                self.edges.insert(o, None);
            }
        }
    }

    /// add a verified transaction
    pub fn add_transaction(&mut self, tx: Transaction) {
        let inputs = tx.input_pts();
        let outputs = tx.output_pts();
        let h = tx.hash();

        let mut count: u64 = 0;

        for i in inputs {
            if let Some(x) = self.edges.get_mut(&i) {
                *x = Some(h);
                count += 1;
            } else {
                self.out_edges.insert(i, Some(h));
            }
        }

        for o in outputs {
            self.edges.entry(o).or_insert(None);
        }

        if count == 0 {
            self.roots.insert(h, PoolEntry::new(tx, count));
        } else {
            self.no_roots.insert(h, PoolEntry::new(tx, count));
        }
    }

    fn reconcile_transaction(&mut self, tx: &Transaction) {
        let outputs = tx.output_pts();
        let inputs = tx.input_pts();

        for o in outputs {
            if let Some(h) = self.remove_edge(&o) {
                self.dec_ref(&h);
                self.out_edges.insert(o, Some(h));
            }
        }

        for i in inputs {
            self.out_edges.remove(&i);
        }
    }

    fn resolve_conflict(&mut self, tx: &Transaction) {
        let inputs = tx.input_pts();

        for i in inputs {
            if let Some(h) = self.remove_out_edge(&i) {
                self.remove_vertex(&h);
            }
        }
    }

    /// when the transaction's input is used by other transaction, we remove it.
    pub fn remove_vertex(&mut self, h: &H256) {
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

            for o in tx.output_pts() {
                if let Some(ch) = self.remove_edge(&o) {
                    self.remove_vertex(&ch);
                }
            }
        }
    }

    /// dec vertex's pool output ref num
    pub fn dec_ref(&mut self, h: &H256) {
        let mut count = 1;
        if let Some(x) = self.no_roots.get_mut(h) {
            x.refs_count -= 1;
            count = x.refs_count;
        }

        if count == 0 {
            self.update_root(h);
        }
    }

    pub fn get_potential_root(
        &self,
        tx: &Transaction,
        counts: &mut HashMap<H256, u64>,
    ) -> Vec<Transaction> {
        let mut roots = Vec::new();
        let outputs = tx.output_pts();

        for o in outputs {
            if let Some(h) = self.get_edge(&o) {
                if let Some(x) = self.no_roots.get(&h) {
                    let c = *counts.get(&h).unwrap_or(&1);
                    if x.refs_count == c {
                        roots.push(x.transaction.clone());
                    } else {
                        counts.insert(h, c + 1);
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
    pub fn get_roots(&self, n: usize) -> Vec<Transaction> {
        if self.roots.len() <= n {
            self.roots
                .values()
                .take(n)
                .map(|x| &x.transaction)
                .cloned()
                .collect()
        } else {
            let mut roots: Vec<Transaction> = self
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
    pub fn get_vertices(&self) -> Vec<Transaction> {
        self.roots
            .values()
            .map(|x| &x.transaction)
            .chain(self.no_roots.values().map(|x| &x.transaction))
            .cloned()
            .collect::<Vec<Transaction>>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::transaction::{CellInput, CellOutput};

    #[test]
    fn test_add_entry() {
        let inputs = vec![
            CellInput::new(OutPoint::new(H256::zero(), 1), Vec::new()),
            CellInput::new(OutPoint::new(H256::zero(), 2), Vec::new()),
        ];

        let outputs = vec![CellOutput::new(10, 10, Vec::new(), Vec::new())];

        let tx1 = Transaction::new(0, Vec::new(), inputs, outputs.clone());

        let tx1_hash = tx1.hash();

        let inputs2 = vec![CellInput::new(OutPoint::new(tx1_hash, 0), Vec::new())];

        let tx2 = Transaction::new(0, Vec::new(), inputs2, outputs);

        let mut pool = Pool::new();

        pool.add_transaction(tx1.clone());
        pool.add_transaction(tx2.clone());

        assert_eq!(pool.pool.no_roots.len(), 1);
        assert_eq!(pool.pool.roots.len(), 1);
        assert_eq!(pool.pool.edges.len(), 2);

        pool.commit_transaction(&tx1);

        assert_eq!(pool.pool.roots.len(), 1);
        assert_eq!(pool.pool.no_roots.len(), 0);
    }

}
