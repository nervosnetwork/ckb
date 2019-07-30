use crate::tx_pool::types::{AncestorsScoreSortKey, ProposedEntry, TxLink};
use ckb_core::cell::{CellMetaBuilder, CellProvider, CellStatus};
use ckb_core::transaction::{CellOutput, OutPoint, ProposalShortId, Transaction};
use ckb_core::{Bytes, Capacity, Cycle};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::hash::Hash;

#[derive(Default, Debug, Clone)]
pub(crate) struct Edges<K: Hash + Eq, V: Copy + Eq + Hash> {
    pub(crate) inner: HashMap<K, Option<V>>,
    pub(crate) outer: HashMap<K, Option<V>>,
    pub(crate) deps: HashMap<K, HashSet<V>>,
}

impl<K: Hash + Eq, V: Copy + Eq + Hash> Edges<K, V> {
    #[cfg(test)]
    pub(crate) fn inner_len(&self) -> usize {
        self.inner.len()
    }

    #[cfg(test)]
    pub(crate) fn outer_len(&self) -> usize {
        self.outer.len()
    }

    pub(crate) fn insert_outer(&mut self, key: K, value: V) {
        self.outer.insert(key, Some(value));
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

    pub(crate) fn get_deps(&self, key: &K) -> Option<&HashSet<V>> {
        self.deps.get(key)
    }

    pub(crate) fn remove_deps(&mut self, key: &K) -> Option<HashSet<V>> {
        self.deps.remove(key)
    }

    pub(crate) fn contains_key(&self, key: &K) -> bool {
        self.inner.contains_key(&key)
    }

    pub(crate) fn insert_deps(&mut self, key: K, value: V) {
        let e = self.deps.entry(key).or_insert_with(HashSet::default);
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
    pub(crate) vertices: HashMap<ProposalShortId, ProposedEntry>,
    pub(crate) edges: Edges<OutPoint, ProposalShortId>,
    /// A index sorted by tx ancestors score
    ancestors_score_index: BTreeSet<AncestorsScoreSortKey>,
    /// A map track transaction ancestors and descendants
    links: HashMap<ProposalShortId, TxLink>,
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
                let (output, data) = self.get_output_with_data(o).expect("output");
                CellStatus::live_cell(
                    CellMetaBuilder::from_cell_output(output.to_owned(), data)
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
    pub(crate) fn new() -> Self {
        ProposedPool::default()
    }

    pub(crate) fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.vertices.contains_key(id)
    }

    pub fn get(&self, id: &ProposalShortId) -> Option<&ProposedEntry> {
        self.vertices.get(id)
    }

    pub(crate) fn get_tx(&self, id: &ProposalShortId) -> Option<&Transaction> {
        self.get(id).map(|x| &x.transaction)
    }

    pub(crate) fn get_output_with_data(&self, o: &OutPoint) -> Option<(CellOutput, Bytes)> {
        o.cell.as_ref().and_then(|cell_out_point| {
            self.vertices
                .get(&ProposalShortId::from_tx_hash(&cell_out_point.tx_hash))
                .and_then(|x| {
                    x.transaction
                        .get_output_with_data(cell_out_point.index as usize)
                })
        })
    }

    // remove entry and all it's descendants
    pub(crate) fn remove_entry_and_descendants(
        &mut self,
        id: &ProposalShortId,
    ) -> Vec<ProposedEntry> {
        let mut entries = Vec::new();
        let mut queue = VecDeque::new();

        queue.push_back(id.to_owned());

        while let Some(id) = queue.pop_front() {
            if let Some(entry) = self.remove_entry(&id) {
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

    pub(crate) fn add_tx(&mut self, cycles: Cycle, fee: Capacity, size: usize, tx: Transaction) {
        let inputs = tx.input_pts_iter();
        let outputs = tx.output_pts();
        let deps = tx.deps_iter();

        let tx_short_id = tx.proposal_short_id();

        let mut count: usize = 0;
        let mut parents: HashSet<ProposalShortId> = Default::default();

        for i in inputs {
            if let Some(id) = self.edges.get_inner_mut(i) {
                *id = Some(tx_short_id);
                count += 1;
                if let Some(cell_out_point) = i.cell.as_ref() {
                    parents.insert(ProposalShortId::from_tx_hash(&cell_out_point.tx_hash));
                }
            } else {
                self.edges.insert_outer(i.to_owned(), tx_short_id);
            }
        }

        for d in deps {
            if self.edges.contains_key(d) {
                count += 1;
                if let Some(cell_out_point) = d.cell.as_ref() {
                    parents.insert(ProposalShortId::from_tx_hash(&cell_out_point.tx_hash));
                }
            }
            self.edges.insert_deps(d.to_owned(), tx_short_id);
        }

        for o in outputs {
            self.edges.mark_inpool(o);
        }

        let entry = ProposedEntry::new(tx, count, cycles, fee, size);
        self.insert_entry(tx_short_id, entry, parents);
    }

    pub(crate) fn remove_committed_tx(&mut self, tx: &Transaction) -> Vec<ProposedEntry> {
        let outputs = tx.output_pts();
        let inputs = tx.input_pts_iter();
        let deps = tx.deps_iter();
        let id = tx.proposal_short_id();

        let mut removed = Vec::new();

        if let Some(entry) = self.remove_entry(&id) {
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

    fn resolve_conflict(&mut self, tx: &Transaction) -> Vec<ProposedEntry> {
        let inputs = tx.input_pts_iter();
        let mut removed = Vec::new();

        for i in inputs {
            if let Some(id) = self.edges.remove_outer(i) {
                removed.append(&mut self.remove_entry_and_descendants(&id));
            }

            if let Some(x) = self.edges.remove_deps(&i) {
                for id in x {
                    removed.append(&mut self.remove_entry_and_descendants(&id));
                }
            }
        }
        removed
    }

    /// Iterate sorted transactions
    /// transaction is sorted by ancestor score from higher to lower,
    /// this method is used for package txs into block
    pub(crate) fn with_sorted_by_score_iter<F, Ret>(&self, func: F) -> Ret
    where
        F: FnOnce(&mut dyn Iterator<Item = &ProposedEntry>) -> Ret,
    {
        let mut iter = self.ancestors_score_index.iter().rev().map(|key| {
            self.vertices
                .get(&key.id)
                .expect("proposed pool must be consistent")
        });
        func(&mut iter)
    }

    pub(crate) fn dec_ref(&mut self, id: &ProposalShortId) {
        if let Some(x) = self.vertices.get_mut(&id) {
            x.refs_count -= 1;
        }
    }

    /// find all ancestors from pool
    pub fn get_ancestors(&self, tx_short_id: &ProposalShortId) -> HashSet<ProposalShortId> {
        TxLink::get_ancestors(&self.links, tx_short_id)
    }

    /// find all descendants from pool
    pub fn get_descendants(&self, tx_short_id: &ProposalShortId) -> HashSet<ProposalShortId> {
        TxLink::get_descendants(&self.links, tx_short_id)
    }

    /// update entry ancestor prefix fields
    fn update_ancestors_stat_for_entry(
        &self,
        entry: &mut ProposedEntry,
        parents: &HashSet<ProposalShortId>,
    ) {
        for id in parents {
            let tx_entry = self.vertices.get(&id).expect("pool consistent");
            entry.ancestors_cycles = entry
                .ancestors_cycles
                .saturating_add(tx_entry.ancestors_cycles);
            entry.ancestors_size = entry.ancestors_size.saturating_add(tx_entry.ancestors_size);
            entry.ancestors_fee = Capacity::shannons(
                entry
                    .ancestors_fee
                    .as_u64()
                    .saturating_add(tx_entry.ancestors_fee.as_u64()),
            );
            entry.ancestors_count = entry
                .ancestors_count
                .saturating_add(tx_entry.ancestors_count);
        }
    }

    /// insert an entry
    fn insert_entry(
        &mut self,
        id: ProposalShortId,
        mut entry: ProposedEntry,
        parents: HashSet<ProposalShortId>,
    ) {
        // update ancestor_fields
        self.update_ancestors_stat_for_entry(&mut entry, &parents);
        // insert links
        self.insert_link(id, parents);
        let key = AncestorsScoreSortKey::from(&entry);
        self.ancestors_score_index.insert(key);
        self.vertices.insert(id, entry);
    }

    /// delete an entry
    fn remove_entry(&mut self, id: &ProposalShortId) -> Option<ProposedEntry> {
        self.vertices.remove(id).map(|entry| {
            let key = AncestorsScoreSortKey::from(&entry);
            self.ancestors_score_index.remove(&key);
            self.remove_link(id.to_owned());
            entry
        })
    }

    /// insert a link then update related links
    /// NOTICE: you should consider insert_entry instead call this function directly
    fn insert_link(&mut self, tx_short_id: ProposalShortId, parents: HashSet<ProposalShortId>) {
        // update parents links
        for id in &parents {
            if let Some(link) = self.links.get_mut(id) {
                link.children.insert(tx_short_id);
            }
        }
        self.links.insert(
            tx_short_id,
            TxLink {
                parents,
                children: Default::default(),
            },
        );
    }
    /// remove a link then update related links
    /// NOTICE: you should consider remove_entry instead call this function directly
    fn remove_link(&mut self, tx_short_id: ProposalShortId) {
        let mut remove_queue = vec![tx_short_id];
        while !remove_queue.is_empty() {
            let id = remove_queue.pop().expect("exists");
            // link may removed by previous loop in case B -> A, C -> A, B
            if let Some(TxLink { children, parents }) = self.links.remove(&id) {
                // remove children
                remove_queue.extend(children);
                // update parents
                for parent in parents {
                    // parent may removed by previous loop
                    if let Some(parent_entry) = self.links.get_mut(&parent) {
                        parent_entry.children.remove(&id);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_core::transaction::{CellInput, CellOutputBuilder, Transaction, TransactionBuilder};
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
                CellOutputBuilder::default()
                    .capacity(Capacity::bytes(i + 1).unwrap())
                    .build()
            }))
            .outputs_data((0..outputs_len).map(|_| Bytes::new()))
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

        pool.remove_committed_tx(&tx1);

        assert_eq!(pool.edges.inner_len(), 3);
        assert_eq!(pool.edges.outer_len(), 2);
    }

    #[test]
    #[allow(clippy::cognitive_complexity)]
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

        pool.remove_committed_tx(&tx1);

        assert_eq!(pool.get(&id3).unwrap().refs_count, 0);
        assert_eq!(pool.get(&id5).unwrap().refs_count, 1);
        assert_eq!(pool.edges.inner_len(), 10);
        assert_eq!(pool.edges.outer_len(), 4);
    }

    #[test]
    fn test_sorted_by_tx_fee_rate() {
        let tx1 = build_tx(vec![(&H256::zero(), 1)], 1);
        let tx2 = build_tx(vec![(&H256::zero(), 2)], 1);
        let tx3 = build_tx(vec![(&H256::zero(), 3)], 1);

        let mut pool = ProposedPool::new();

        let cycles = 5_000_000;
        let size = 200;

        pool.add_tx(cycles, Capacity::shannons(100), size, tx1.clone());
        pool.add_tx(cycles, Capacity::shannons(300), size, tx2.clone());
        pool.add_tx(cycles, Capacity::shannons(200), size, tx3.clone());

        let txs_sorted_by_fee_rate = pool.with_sorted_by_score_iter(|iter| {
            iter.map(|entry| entry.transaction.hash().to_owned())
                .collect::<Vec<_>>()
        });
        let expect_result = vec![
            tx2.hash().to_owned(),
            tx3.hash().to_owned(),
            tx1.hash().to_owned(),
        ];
        assert_eq!(txs_sorted_by_fee_rate, expect_result);
    }

    #[test]
    fn test_sorted_by_ancestors_score() {
        let tx1 = build_tx(vec![(&H256::zero(), 1)], 2);
        let tx1_hash = tx1.hash();
        let tx2 = build_tx(vec![(&tx1_hash, 1)], 1);
        let tx2_hash = tx2.hash();
        let tx3 = build_tx(vec![(&tx1_hash, 2)], 1);
        let tx4 = build_tx(vec![(&tx2_hash, 1)], 1);

        let mut pool = ProposedPool::new();

        let cycles = 5_000_000;
        let size = 200;

        pool.add_tx(cycles, Capacity::shannons(100), size, tx1.clone());
        pool.add_tx(cycles, Capacity::shannons(300), size, tx2.clone());
        pool.add_tx(cycles, Capacity::shannons(200), size, tx3.clone());
        pool.add_tx(cycles, Capacity::shannons(400), size, tx4.clone());

        let txs_sorted_by_fee_rate = pool.with_sorted_by_score_iter(|iter| {
            iter.map(|entry| entry.transaction.hash().to_owned())
                .collect::<Vec<_>>()
        });
        let expect_result = vec![
            tx4.hash().to_owned(),
            tx2.hash().to_owned(),
            tx3.hash().to_owned(),
            tx1.hash().to_owned(),
        ];
        assert_eq!(txs_sorted_by_fee_rate, expect_result);
    }

    #[test]
    fn test_sorted_by_ancestors_score_competitive() {
        let tx1 = build_tx(vec![(&H256::zero(), 1)], 2);
        let tx1_hash = tx1.hash();
        let tx2 = build_tx(vec![(&tx1_hash, 0)], 1);
        let tx2_hash = tx2.hash();
        let tx3 = build_tx(vec![(&tx2_hash, 0)], 1);

        let tx2_1 = build_tx(vec![(&H256::zero(), 2)], 2);
        let tx2_1_hash = tx2_1.hash();
        let tx2_2 = build_tx(vec![(&tx2_1_hash, 0)], 1);
        let tx2_2_hash = tx2_2.hash();
        let tx2_3 = build_tx(vec![(&tx2_2_hash, 0)], 1);
        let tx2_3_hash = tx2_3.hash();
        let tx2_4 = build_tx(vec![(&tx2_3_hash, 0)], 1);

        let mut pool = ProposedPool::new();

        let cycles = 5_000_000;
        let size = 200;

        for &tx in &[&tx1, &tx2, &tx3, &tx2_1, &tx2_2, &tx2_3, &tx2_4] {
            pool.add_tx(cycles, Capacity::shannons(200), size, tx.clone());
        }

        let txs_sorted_by_fee_rate = pool.with_sorted_by_score_iter(|iter| {
            iter.map(|entry| format!("{:x}", entry.transaction.hash()))
                .collect::<Vec<_>>()
        });
        // the entry with most ancestors score will win
        let expect_result = format!("{:x}", tx2_4.hash());
        assert_eq!(txs_sorted_by_fee_rate[0], expect_result);
    }

    #[test]
    fn test_get_ancestors() {
        let tx1 = build_tx(vec![(&H256::zero(), 1)], 2);
        let tx1_hash = tx1.hash();
        let tx2 = build_tx(vec![(&tx1_hash, 0)], 1);
        let tx2_hash = tx2.hash();
        let tx3 = build_tx(vec![(&tx1_hash, 1)], 1);
        let tx4 = build_tx(vec![(&tx2_hash, 0)], 1);

        let mut pool = ProposedPool::new();

        let cycles = 5_000_000;
        let size = 200;

        pool.add_tx(cycles, Capacity::shannons(100), size, tx1.clone());
        pool.add_tx(cycles, Capacity::shannons(300), size, tx2.clone());
        pool.add_tx(cycles, Capacity::shannons(200), size, tx3.clone());
        pool.add_tx(cycles, Capacity::shannons(400), size, tx4.clone());

        let ancestors = pool.get_ancestors(&tx4.proposal_short_id());
        let expect_result = vec![tx1.proposal_short_id(), tx2.proposal_short_id()]
            .into_iter()
            .collect();
        assert_eq!(ancestors, expect_result);
        let entry = pool.get(&tx4.proposal_short_id()).expect("exists");
        assert_eq!(
            entry.ancestors_cycles,
            ancestors
                .iter()
                .map(|id| pool.get(id).unwrap().cycles)
                .sum::<u64>()
                + cycles
        );
        assert_eq!(
            entry.ancestors_size,
            ancestors
                .iter()
                .map(|id| pool.get(id).unwrap().size)
                .sum::<usize>()
                + size
        );
        assert_eq!(entry.ancestors_count, ancestors.len() + 1);

        let ancestors = pool.get_ancestors(&tx3.proposal_short_id());
        let expect_result = vec![tx1.proposal_short_id()].into_iter().collect();
        assert_eq!(ancestors, expect_result);
        let entry = pool.get(&tx3.proposal_short_id()).expect("exists");
        assert_eq!(
            entry.ancestors_cycles,
            ancestors
                .iter()
                .map(|id| pool.get(id).unwrap().cycles)
                .sum::<u64>()
                + cycles
        );
        assert_eq!(
            entry.ancestors_size,
            ancestors
                .iter()
                .map(|id| pool.get(id).unwrap().size)
                .sum::<usize>()
                + size
        );
        assert_eq!(entry.ancestors_count, ancestors.len() + 1);

        let ancestors = pool.get_ancestors(&tx1.proposal_short_id());
        assert_eq!(ancestors, Default::default());
        let entry = pool.get(&tx1.proposal_short_id()).expect("exists");
        assert_eq!(entry.ancestors_cycles, cycles);
        assert_eq!(entry.ancestors_size, size);
        assert_eq!(entry.ancestors_count, 1);
    }
}
