#![allow(dead_code)]

use crate::tx_pool::types::PoolEntry;
use ckb_core::cell::{CellProvider, CellStatus};
use ckb_core::transaction::{CellOutput, OutPoint, ProposalShortId, Transaction};
use ckb_core::Cycle;
use fnv::{FnvHashMap, FnvHashSet};
use linked_hash_map::LinkedHashMap;
use std::hash::Hash;

#[derive(Default, Debug, Clone)]
pub(crate) struct Edges<K: Hash + Eq, V: Copy + Eq + Hash> {
    pub(crate) inner: FnvHashMap<K, Option<V>>,
    pub(crate) outer: FnvHashMap<K, Option<V>>,
    pub(crate) deps: FnvHashMap<K, FnvHashSet<V>>,
}

impl<K: Hash + Eq, V: Copy + Eq + Hash> Edges<K, V> {
    pub(crate) fn inner_len(&self) -> usize {
        self.inner.len()
    }

    pub(crate) fn outer_len(&self) -> usize {
        self.outer.len()
    }

    pub(crate) fn insert_outer(&mut self, key: K, value: V) {
        self.outer.insert(key, Some(value));
    }

    pub(crate) fn insert_inner(&mut self, key: K, value: V) {
        self.inner.insert(key, Some(value));
    }

    pub(crate) fn remove_outer(&mut self, key: &K) -> Option<V> {
        self.outer.remove(key).unwrap_or(None)
    }

    pub(crate) fn remove_inner(&mut self, key: &K) -> Option<V> {
        self.inner.remove(key).unwrap_or(None)
    }

    pub(crate) fn mark_inpool(&mut self, key: K) {
        self.inner.insert(key, None);
    }

    pub(crate) fn get_inner(&self, key: &K) -> Option<&Option<V>> {
        self.inner.get(key)
    }

    pub(crate) fn get_outer(&self, key: &K) -> Option<&Option<V>> {
        self.outer.get(key)
    }

    pub(crate) fn get_inner_mut(&mut self, key: &K) -> Option<&mut Option<V>> {
        self.inner.get_mut(key)
    }

    pub(crate) fn remove_edge(&mut self, key: &K) -> Option<V> {
        self.inner.remove(key).unwrap_or(None)
    }

    pub(crate) fn get_deps(&self, key: &K) -> Option<&FnvHashSet<V>> {
        self.deps.get(key)
    }

    pub(crate) fn remove_deps(&mut self, key: &K) -> Option<FnvHashSet<V>> {
        self.deps.remove(key)
    }

    pub(crate) fn contains_key(&self, key: &K) -> bool {
        self.inner.contains_key(&key)
    }

    pub(crate) fn insert_deps(&mut self, key: K, value: V) {
        let e = self.deps.entry(key).or_insert_with(FnvHashSet::default);
        e.insert(value);
    }

    pub(crate) fn delete_value_in_deps(&mut self, key: &K, value: &V) {
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
    pub(crate) vertices: LinkedHashMap<ProposalShortId, PoolEntry>,
    pub(crate) edges: Edges<OutPoint, ProposalShortId>,
}

impl CellProvider for StagingPool {
    fn cell(&self, o: &OutPoint) -> CellStatus {
        if let Some(x) = self.edges.get_inner(o) {
            if x.is_some() {
                CellStatus::Dead
            } else {
                CellStatus::live_output(self.get_output(o).expect("output"), None, false)
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
            .get(&ProposalShortId::from_tx_hash(&o.tx_hash))
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
                if self.edges.inner.remove(&i).is_none() {
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

    pub fn add_tx(&mut self, cycles: Cycle, tx: Transaction) {
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

        self.vertices
            .insert(id, PoolEntry::new(tx, count, Some(cycles)));
    }

    pub fn remove_committed_tx(&mut self, tx: &Transaction) {
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

    // pub fn inc_ref(&mut self, id: &ProposalShortId) {
    //     if let Some(x) = self.vertices.get_mut(&id) {
    //         x.refs_count += 1;
    //     }
    // }

    pub fn dec_ref(&mut self, id: &ProposalShortId) {
        if let Some(x) = self.vertices.get_mut(&id) {
            x.refs_count -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_core::script::Script;
    use ckb_core::transaction::{CellInput, CellOutput, Transaction, TransactionBuilder};
    use ckb_core::Capacity;
    use numext_fixed_hash::H256;

    fn build_tx(inputs: Vec<(H256, u32)>, outputs_len: usize) -> Transaction {
        TransactionBuilder::default()
            .inputs(
                inputs
                    .into_iter()
                    .map(|(txid, index)| {
                        CellInput::new(OutPoint::new(txid, index), 0, Default::default())
                    })
                    .collect(),
            )
            .outputs(
                (0..outputs_len)
                    .map(|i| {
                        CellOutput::new(
                            Capacity::bytes(i + 1).unwrap(),
                            Vec::new(),
                            Script::default(),
                            None,
                        )
                    })
                    .collect(),
            )
            .build()
    }

    pub const MOCK_CYCLES: Cycle = 0;

    #[test]
    fn test_add_entry() {
        let tx1 = build_tx(vec![(H256::zero(), 1), (H256::zero(), 2)], 1);
        let tx1_hash = tx1.hash().clone();
        let tx2 = build_tx(vec![(tx1_hash, 0)], 1);

        let mut pool = StagingPool::new();
        let id1 = tx1.proposal_short_id();
        let id2 = tx2.proposal_short_id();

        pool.add_tx(MOCK_CYCLES, tx1.clone());
        pool.add_tx(MOCK_CYCLES, tx2.clone());

        assert_eq!(pool.vertices.len(), 2);
        assert_eq!(pool.edges.inner_len(), 2);
        assert_eq!(pool.edges.outer_len(), 2);

        assert_eq!(pool.get(&id1).unwrap().refs_count, 0);
        assert_eq!(pool.get(&id2).unwrap().refs_count, 1);

        pool.remove_committed_tx(&tx1);
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

        let id1 = tx1.proposal_short_id();
        let id2 = tx2.proposal_short_id();

        pool.add_tx(MOCK_CYCLES, tx1.clone());
        pool.add_tx(MOCK_CYCLES, tx2.clone());

        assert_eq!(pool.get(&id1).unwrap().refs_count, 0);
        assert_eq!(pool.get(&id2).unwrap().refs_count, 0);
        assert_eq!(pool.edges.inner_len(), 4);
        assert_eq!(pool.edges.outer_len(), 4);

        let mut mineable: Vec<_> = pool.get_txs(0).into_iter().map(|e| e.transaction).collect();
        assert_eq!(0, mineable.len());

        mineable = pool.get_txs(1).into_iter().map(|e| e.transaction).collect();
        assert_eq!(1, mineable.len());
        assert!(mineable.contains(&tx1));

        mineable = pool.get_txs(2).into_iter().map(|e| e.transaction).collect();
        assert_eq!(2, mineable.len());
        assert!(mineable.contains(&tx1) && mineable.contains(&tx2));

        mineable = pool.get_txs(3).into_iter().map(|e| e.transaction).collect();
        assert_eq!(2, mineable.len());
        assert!(mineable.contains(&tx1) && mineable.contains(&tx2));

        pool.remove_committed_tx(&tx1);

        assert_eq!(pool.edges.inner_len(), 3);
        assert_eq!(pool.edges.outer_len(), 2);
    }

    #[test]
    #[allow(clippy::cyclomatic_complexity)]
    fn test_add_no_roots() {
        let tx1 = build_tx(vec![(H256::zero(), 1)], 3);
        let tx2 = build_tx(vec![], 4);
        let tx1_hash = tx1.hash().clone();
        let tx2_hash = tx2.hash().clone();

        let tx3 = build_tx(vec![(tx1_hash.clone(), 0), (H256::zero(), 2)], 2);
        let tx4 = build_tx(vec![(tx1_hash.clone(), 1), (tx2_hash.clone(), 0)], 2);

        let tx3_hash = tx3.hash().clone();
        let tx5 = build_tx(vec![(tx1_hash.clone(), 2), (tx3_hash.clone(), 0)], 2);

        let id1 = tx1.proposal_short_id();
        let id3 = tx3.proposal_short_id();
        let id5 = tx5.proposal_short_id();

        let mut pool = StagingPool::new();

        pool.add_tx(MOCK_CYCLES, tx1.clone());
        pool.add_tx(MOCK_CYCLES, tx2.clone());
        pool.add_tx(MOCK_CYCLES, tx3.clone());
        pool.add_tx(MOCK_CYCLES, tx4.clone());
        pool.add_tx(MOCK_CYCLES, tx5.clone());

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
        assert!(mineable.contains(&tx1));

        mineable = pool.get_txs(2).into_iter().map(|x| x.transaction).collect();
        assert_eq!(2, mineable.len());
        assert!(mineable.contains(&tx1) && mineable.contains(&tx2));

        mineable = pool.get_txs(3).into_iter().map(|x| x.transaction).collect();
        assert_eq!(3, mineable.len());

        assert!(mineable.contains(&tx1) && mineable.contains(&tx2) && mineable.contains(&tx3));

        mineable = pool.get_txs(4).into_iter().map(|x| x.transaction).collect();
        assert_eq!(4, mineable.len());
        assert!(mineable.contains(&tx1) && mineable.contains(&tx2));
        assert!(mineable.contains(&tx3) && mineable.contains(&tx4));

        mineable = pool.get_txs(5).into_iter().map(|x| x.transaction).collect();
        assert_eq!(5, mineable.len());
        assert!(mineable.contains(&tx1) && mineable.contains(&tx2));
        assert!(mineable.contains(&tx3) && mineable.contains(&tx4));
        assert!(mineable.contains(&tx5));

        mineable = pool.get_txs(6).into_iter().map(|x| x.transaction).collect();
        assert_eq!(5, mineable.len());
        assert!(mineable.contains(&tx1) && mineable.contains(&tx2));
        assert!(mineable.contains(&tx3) && mineable.contains(&tx4));
        assert!(mineable.contains(&tx5));

        pool.remove_committed_tx(&tx1);

        assert_eq!(pool.get(&id3).unwrap().refs_count, 0);
        assert_eq!(pool.get(&id5).unwrap().refs_count, 1);
        assert_eq!(pool.edges.inner_len(), 10);
        assert_eq!(pool.edges.outer_len(), 4);

        mineable = pool.get_txs(1).into_iter().map(|x| x.transaction).collect();
        assert_eq!(1, mineable.len());
        assert!(mineable.contains(&tx2));

        mineable = pool.get_txs(2).into_iter().map(|x| x.transaction).collect();
        assert_eq!(2, mineable.len());
        assert!(mineable.contains(&tx2) && mineable.contains(&tx3));

        mineable = pool.get_txs(3).into_iter().map(|x| x.transaction).collect();
        assert_eq!(3, mineable.len());
        assert!(mineable.contains(&tx2) && mineable.contains(&tx3));
        assert!(mineable.contains(&tx4));

        mineable = pool.get_txs(4).into_iter().map(|x| x.transaction).collect();
        assert_eq!(4, mineable.len());
        assert!(mineable.contains(&tx2) && mineable.contains(&tx3));
        assert!(mineable.contains(&tx4) && mineable.contains(&tx5));

        mineable = pool.get_txs(5).into_iter().map(|x| x.transaction).collect();
        assert_eq!(4, mineable.len());
    }
}
