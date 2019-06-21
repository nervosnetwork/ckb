#![allow(dead_code)]

use crate::tx_pool::types::ProposedEntry;
use ckb_core::cell::{CellMetaBuilder, CellProvider, CellStatus};
use ckb_core::transaction::{CellOutput, OutPoint, ProposalShortId, Transaction};
use ckb_core::{Capacity, Cycle};
use ckb_util::{FnvHashMap, FnvHashSet, LinkedFnvHashMap};
use std::collections::VecDeque;
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
pub struct ProposedPool {
    pub(crate) vertices: LinkedFnvHashMap<ProposalShortId, ProposedEntry>,
    pub(crate) edges: Edges<OutPoint, ProposalShortId>,
}

impl CellProvider for ProposedPool {
    fn cell(&self, o: &OutPoint) -> CellStatus {
        if o.cell.is_none() {
            return CellStatus::Unspecified;
        }
        if let Some(x) = self.edges.get_inner(o) {
            if x.is_some() {
                CellStatus::Dead
            } else {
                let output = self.get_output(o).expect("output");
                CellStatus::live_cell(
                    CellMetaBuilder::from_cell_output(output.to_owned())
                        .out_point(o.cell.as_ref().unwrap().to_owned())
                        .build(),
                )
            }
        } else if self.edges.get_outer(o).is_some() {
            CellStatus::Dead
        } else {
            CellStatus::Unknown
        }
    }
}

impl ProposedPool {
    pub fn new() -> Self {
        ProposedPool::default()
    }

    pub fn capacity(&self) -> usize {
        self.vertices.len()
    }

    pub fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.vertices.contains_key(id)
    }

    pub fn get(&self, id: &ProposalShortId) -> Option<&ProposedEntry> {
        self.vertices.get(id)
    }

    pub fn get_tx(&self, id: &ProposalShortId) -> Option<&Transaction> {
        self.get(id).map(|x| &x.transaction)
    }

    pub fn get_output(&self, o: &OutPoint) -> Option<CellOutput> {
        o.cell.as_ref().and_then(|cell_out_point| {
            self.vertices
                .get(&ProposalShortId::from_tx_hash(&cell_out_point.tx_hash))
                .and_then(|x| x.transaction.get_output(cell_out_point.index as usize))
        })
    }

    pub fn remove_vertex(&mut self, id: &ProposalShortId) -> Vec<ProposedEntry> {
        let mut entries = Vec::new();
        let mut queue = VecDeque::new();

        queue.push_back(id.to_owned());

        while let Some(id) = queue.pop_front() {
            if let Some(entry) = self.vertices.remove(&id) {
                let tx = &entry.transaction;
                let inputs = tx.input_pts_iter();
                let outputs = tx.output_pts();
                let deps = tx.deps_iter();
                for i in inputs {
                    if self.edges.inner.remove(i).is_none() {
                        self.edges.outer.remove(i);
                    }
                }

                for d in deps {
                    self.edges.delete_value_in_deps(d, &id);
                }

                for o in outputs {
                    if let Some(cid) = self.edges.remove_inner(&o) {
                        queue.push_back(cid);
                    }

                    if let Some(ids) = self.edges.remove_deps(&o) {
                        for cid in ids {
                            queue.push_back(cid);
                        }
                    }
                }

                entries.push(entry);
            }
        }
        entries
    }

    pub fn remove(&mut self, id: &ProposalShortId) -> Vec<ProposedEntry> {
        self.remove_vertex(id)
    }

    pub fn add_tx(&mut self, cycles: Cycle, fee: Capacity, size: usize, tx: Transaction) {
        let inputs = tx.input_pts_iter();
        let outputs = tx.output_pts();
        let deps = tx.deps_iter();

        let id = tx.proposal_short_id();

        let mut count: usize = 0;

        for i in inputs {
            let mut flag = true;
            if let Some(x) = self.edges.get_inner_mut(i) {
                *x = Some(id);
                count += 1;
                flag = false;
            }

            if flag {
                self.edges.insert_outer(i.to_owned(), id);
            }
        }

        for d in deps {
            if self.edges.contains_key(d) {
                count += 1;
            }
            self.edges.insert_deps(d.to_owned(), id);
        }

        for o in outputs {
            self.edges.mark_inpool(o);
        }

        self.vertices
            .insert(id, ProposedEntry::new(tx, count, cycles, fee, size));
    }

    pub fn remove_committed_tx(&mut self, tx: &Transaction) -> Vec<ProposedEntry> {
        let outputs = tx.output_pts();
        let inputs = tx.input_pts_iter();
        let deps = tx.deps_iter();
        let id = tx.proposal_short_id();

        let mut removed = Vec::new();

        if let Some(entry) = self.vertices.remove(&id) {
            removed.push(entry);
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
                self.edges.remove_outer(i);
            }

            for d in deps {
                self.edges.delete_value_in_deps(d, &id);
            }
        } else {
            removed.append(&mut self.resolve_conflict(tx));
        }
        removed
    }

    pub fn resolve_conflict(&mut self, tx: &Transaction) -> Vec<ProposedEntry> {
        let inputs = tx.input_pts_iter();
        let mut removed = Vec::new();

        for i in inputs {
            if let Some(id) = self.edges.remove_outer(i) {
                removed.append(&mut self.remove(&id));
            }

            if let Some(x) = self.edges.remove_deps(&i) {
                for id in x {
                    removed.append(&mut self.remove(&id));
                }
            }
        }
        removed
    }

    /// Get n transactions in topology
    pub fn get_txs(&self, n: usize) -> Vec<ProposedEntry> {
        self.vertices
            .front_n(n)
            .iter()
            .map(|x| x.1.clone())
            .collect()
    }

    pub fn txs_iter(&self) -> impl Iterator<Item = &ProposedEntry> {
        self.vertices.values()
    }

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
    use ckb_core::{Bytes, Capacity};
    use numext_fixed_hash::{h256, H256};

    fn build_tx(inputs: Vec<(&H256, u32)>, outputs_len: usize) -> Transaction {
        TransactionBuilder::default()
            .inputs(
                inputs.into_iter().map(|(txid, index)| {
                    CellInput::new(OutPoint::new_cell(txid.to_owned(), index), 0)
                }),
            )
            .outputs((0..outputs_len).map(|i| {
                CellOutput::new(
                    Capacity::bytes(i + 1).unwrap(),
                    Bytes::default(),
                    Script::default(),
                    None,
                )
            }))
            .build()
    }

    const MOCK_CYCLES: Cycle = 0;
    const MOCK_FEE: Capacity = Capacity::zero();
    const MOCK_SIZE: usize = 0;

    #[test]
    fn test_add_entry() {
        let tx1 = build_tx(vec![(&H256::zero(), 1), (&H256::zero(), 2)], 1);
        let tx1_hash = tx1.hash();
        let tx2 = build_tx(vec![(tx1_hash, 0)], 1);

        let mut pool = ProposedPool::new();
        let id1 = tx1.proposal_short_id();
        let id2 = tx2.proposal_short_id();

        pool.add_tx(MOCK_CYCLES, MOCK_FEE, MOCK_SIZE, tx1.clone());
        pool.add_tx(MOCK_CYCLES, MOCK_FEE, MOCK_SIZE, tx2.clone());

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
        let tx1 = build_tx(vec![(&H256::zero(), 1), (&H256::zero(), 2)], 1);
        let tx2 = build_tx(vec![(&h256!("0x2"), 1), (&h256!("0x3"), 2)], 3);

        let mut pool = ProposedPool::new();

        let id1 = tx1.proposal_short_id();
        let id2 = tx2.proposal_short_id();

        pool.add_tx(MOCK_CYCLES, MOCK_FEE, MOCK_SIZE, tx1.clone());
        pool.add_tx(MOCK_CYCLES, MOCK_FEE, MOCK_SIZE, tx2.clone());

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
        let tx1 = build_tx(vec![(&H256::zero(), 1)], 3);
        let tx2 = build_tx(vec![], 4);
        let tx1_hash = tx1.hash();
        let tx2_hash = tx2.hash();

        let tx3 = build_tx(vec![(tx1_hash, 0), (&H256::zero(), 2)], 2);
        let tx4 = build_tx(vec![(tx1_hash, 1), (tx2_hash, 0)], 2);

        let tx3_hash = tx3.hash();
        let tx5 = build_tx(vec![(tx1_hash, 2), (tx3_hash, 0)], 2);

        let id1 = tx1.proposal_short_id();
        let id3 = tx3.proposal_short_id();
        let id5 = tx5.proposal_short_id();

        let mut pool = ProposedPool::new();

        pool.add_tx(MOCK_CYCLES, MOCK_FEE, MOCK_SIZE, tx1.clone());
        pool.add_tx(MOCK_CYCLES, MOCK_FEE, MOCK_SIZE, tx2.clone());
        pool.add_tx(MOCK_CYCLES, MOCK_FEE, MOCK_SIZE, tx3.clone());
        pool.add_tx(MOCK_CYCLES, MOCK_FEE, MOCK_SIZE, tx4.clone());
        pool.add_tx(MOCK_CYCLES, MOCK_FEE, MOCK_SIZE, tx5.clone());

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
