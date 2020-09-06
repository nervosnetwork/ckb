use crate::component::container::SortedTxMap;
use crate::component::entry::TxEntry;
use crate::error::Reject;
use ckb_types::{
    bytes::Bytes,
    core::{
        cell::{CellMetaBuilder, CellProvider, CellStatus},
        TransactionView,
    },
    packed::{CellOutput, OutPoint, ProposalShortId},
    prelude::*,
};
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

#[derive(Default, Debug, Clone)]
pub(crate) struct Edges<K: Hash + Eq, V: Eq + Hash> {
    pub(crate) inner: HashMap<K, Option<V>>,
    pub(crate) outer: HashMap<K, Option<V>>,
    pub(crate) deps: HashMap<K, HashSet<V>>,
}

impl<K: Hash + Eq, V: Eq + Hash> Edges<K, V> {
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

    pub(crate) fn remove_deps(&mut self, key: &K) -> Option<HashSet<V>> {
        self.deps.remove(key)
    }

    pub(crate) fn insert_deps(&mut self, key: K, value: V) {
        self.deps.entry(key).or_default().insert(value);
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

#[derive(Debug, Clone)]
pub struct ProposedPool {
    pub(crate) edges: Edges<OutPoint, ProposalShortId>,
    inner: SortedTxMap,
}

impl CellProvider for ProposedPool {
    fn cell(&self, out_point: &OutPoint, with_data: bool) -> CellStatus {
        if let Some(x) = self.edges.get_inner(out_point) {
            if x.is_some() {
                CellStatus::Unknown
            } else {
                let (output, data) = self.get_output_with_data(out_point).expect("output");
                let mut cell_meta = CellMetaBuilder::from_cell_output(output, data)
                    .out_point(out_point.to_owned())
                    .build();
                if !with_data {
                    cell_meta.mem_cell_data = None;
                }
                CellStatus::live_cell(cell_meta)
            }
        } else if self.edges.get_outer(out_point).is_some() {
            CellStatus::Dead
        } else {
            CellStatus::Unknown
        }
    }
}

impl ProposedPool {
    pub(crate) fn new(max_ancestors_count: usize) -> Self {
        ProposedPool {
            edges: Default::default(),
            inner: SortedTxMap::new(max_ancestors_count),
        }
    }

    pub(crate) fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.inner.contains_key(id)
    }

    pub fn get(&self, id: &ProposalShortId) -> Option<&TxEntry> {
        self.inner.get(id)
    }

    pub(crate) fn get_tx(&self, id: &ProposalShortId) -> Option<&TransactionView> {
        self.get(id).map(|x| x.transaction())
    }

    pub fn size(&self) -> usize {
        self.inner.size()
    }

    pub(crate) fn get_output_with_data(&self, out_point: &OutPoint) -> Option<(CellOutput, Bytes)> {
        self.inner
            .get(&ProposalShortId::from_tx_hash(&out_point.tx_hash()))
            .and_then(|x| x.transaction().output_with_data(out_point.index().unpack()))
    }

    // remove entry and all it's descendants
    pub(crate) fn remove_entry_and_descendants(&mut self, id: &ProposalShortId) -> Vec<TxEntry> {
        let removed_entries = self.inner.remove_entry_and_descendants(id);
        for entry in &removed_entries {
            let tx = entry.transaction();
            let inputs = tx.input_pts_iter();
            let outputs = tx.output_pts();
            for i in inputs {
                if self.edges.inner.remove(&i).is_none() {
                    self.edges.outer.remove(&i);
                }
            }

            for d in entry.related_dep_out_points_iter() {
                self.edges.delete_value_in_deps(&d, &id);
            }

            for o in outputs {
                self.edges.remove_inner(&o);
                self.edges.remove_deps(&o);
            }
        }
        removed_entries
    }

    pub(crate) fn remove_committed_tx(
        &mut self,
        tx: &TransactionView,
        related_out_points: &[OutPoint],
    ) -> Vec<TxEntry> {
        let outputs = tx.output_pts();
        let inputs = tx.input_pts_iter();
        // TODO: handle header deps
        let id = tx.proposal_short_id();

        let mut removed = Vec::new();

        if let Some(entry) = self.inner.remove_entry(&id) {
            removed.push(entry);
            for o in outputs {
                if let Some(cid) = self.edges.remove_inner(&o) {
                    self.edges.insert_outer(o.clone(), cid);
                }
            }

            for i in inputs {
                self.edges.remove_outer(&i);
            }

            for d in related_out_points {
                self.edges.delete_value_in_deps(&d, &id);
            }
        } else {
            removed.append(&mut self.resolve_conflict(tx));
        }
        removed
    }

    pub(crate) fn add_entry(&mut self, entry: TxEntry) -> Result<Option<TxEntry>, Reject> {
        let inputs = entry.transaction().input_pts_iter();
        let outputs = entry.transaction().output_pts();

        let tx_short_id = entry.transaction().proposal_short_id();

        for i in inputs {
            if let Some(id) = self.edges.get_inner_mut(&i) {
                *id = Some(tx_short_id.clone());
            } else {
                self.edges.insert_outer(i.to_owned(), tx_short_id.clone());
            }
        }

        for d in entry.related_dep_out_points_iter() {
            self.edges.insert_deps(d.to_owned(), tx_short_id.clone());
        }

        for o in outputs {
            self.edges.mark_inpool(o);
        }
        self.inner.add_entry(entry)
    }

    fn resolve_conflict(&mut self, tx: &TransactionView) -> Vec<TxEntry> {
        let inputs = tx.input_pts_iter();
        let mut removed = Vec::new();

        for i in inputs {
            if let Some(id) = self.edges.remove_outer(&i) {
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
        F: FnOnce(&mut dyn Iterator<Item = &TxEntry>) -> Ret,
    {
        let mut iter = self.inner.keys_sorted_by_fee().map(|key| {
            self.inner
                .get(&key.id)
                .expect("proposed pool must be consistent")
        });
        func(&mut iter)
    }

    /// find all ancestors from pool
    pub fn get_ancestors(&self, tx_short_id: &ProposalShortId) -> HashSet<ProposalShortId> {
        self.inner.get_ancestors(&tx_short_id)
    }

    /// find all descendants from pool
    pub fn get_descendants(&self, tx_short_id: &ProposalShortId) -> HashSet<ProposalShortId> {
        self.inner.get_descendants(&tx_short_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_types::{
        bytes::Bytes,
        core::{
            cell::{get_related_dep_out_points, ResolvedTransaction},
            Capacity, Cycle, DepType, TransactionBuilder,
        },
        h256,
        packed::{Byte32, CellDep, CellInput, CellOutput},
        H256,
    };

    const DEFAULT_MAX_ANCESTORS_SIZE: usize = 25;

    fn build_tx(inputs: Vec<(&Byte32, u32)>, outputs_len: usize) -> ResolvedTransaction {
        let transaction = TransactionBuilder::default()
            .inputs(
                inputs
                    .into_iter()
                    .map(|(txid, index)| CellInput::new(OutPoint::new(txid.to_owned(), index), 0)),
            )
            .outputs((0..outputs_len).map(|i| {
                CellOutput::new_builder()
                    .capacity(Capacity::bytes(i + 1).unwrap().pack())
                    .build()
            }))
            .outputs_data((0..outputs_len).map(|_| Bytes::new().pack()))
            .build();

        ResolvedTransaction {
            transaction,
            resolved_cell_deps: vec![],
            resolved_inputs: vec![],
            resolved_dep_groups: vec![],
        }
    }

    const MOCK_CYCLES: Cycle = 0;
    const MOCK_FEE: Capacity = Capacity::zero();
    const MOCK_SIZE: usize = 0;

    #[test]
    fn test_add_entry() {
        let tx1 = build_tx(vec![(&Byte32::zero(), 1), (&Byte32::zero(), 2)], 1);
        let tx1_hash = tx1.hash();
        let tx2 = build_tx(vec![(&tx1_hash, 0)], 1);

        let mut pool = ProposedPool::new(DEFAULT_MAX_ANCESTORS_SIZE);

        pool.add_entry(TxEntry::new(tx1.clone(), MOCK_CYCLES, MOCK_FEE, MOCK_SIZE))
            .unwrap();
        pool.add_entry(TxEntry::new(tx2, MOCK_CYCLES, MOCK_FEE, MOCK_SIZE))
            .unwrap();

        assert_eq!(pool.size(), 2);
        assert_eq!(pool.edges.inner_len(), 2);
        assert_eq!(pool.edges.outer_len(), 2);

        pool.remove_committed_tx(
            &tx1.transaction,
            &get_related_dep_out_points(&tx1.transaction, |_| None).unwrap(),
        );
        assert_eq!(pool.edges.inner_len(), 1);
        assert_eq!(pool.edges.outer_len(), 1);
    }

    #[test]
    fn test_add_roots() {
        let tx1 = build_tx(vec![(&Byte32::zero(), 1), (&Byte32::zero(), 2)], 1);
        let tx2 = build_tx(
            vec![(&h256!("0x2").pack(), 1), (&h256!("0x3").pack(), 2)],
            3,
        );

        let mut pool = ProposedPool::new(DEFAULT_MAX_ANCESTORS_SIZE);

        pool.add_entry(TxEntry::new(tx1.clone(), MOCK_CYCLES, MOCK_FEE, MOCK_SIZE))
            .unwrap();
        pool.add_entry(TxEntry::new(tx2, MOCK_CYCLES, MOCK_FEE, MOCK_SIZE))
            .unwrap();

        assert_eq!(pool.edges.inner_len(), 4);
        assert_eq!(pool.edges.outer_len(), 4);

        pool.remove_committed_tx(
            &tx1.transaction,
            &get_related_dep_out_points(&tx1.transaction, |_| None).unwrap(),
        );

        assert_eq!(pool.edges.inner_len(), 3);
        assert_eq!(pool.edges.outer_len(), 2);
    }

    #[test]
    #[allow(clippy::cognitive_complexity)]
    fn test_add_no_roots() {
        let tx1 = build_tx(vec![(&Byte32::zero(), 1)], 3);
        let tx2 = build_tx(vec![], 4);
        let tx1_hash = tx1.hash();
        let tx2_hash = tx2.hash();

        let tx3 = build_tx(vec![(&tx1_hash, 0), (&Byte32::zero(), 2)], 2);
        let tx4 = build_tx(vec![(&tx1_hash, 1), (&tx2_hash, 0)], 2);

        let tx3_hash = tx3.hash();
        let tx5 = build_tx(vec![(&tx1_hash, 2), (&tx3_hash, 0)], 2);

        let mut pool = ProposedPool::new(DEFAULT_MAX_ANCESTORS_SIZE);

        pool.add_entry(TxEntry::new(tx1.clone(), MOCK_CYCLES, MOCK_FEE, MOCK_SIZE))
            .unwrap();
        pool.add_entry(TxEntry::new(tx2, MOCK_CYCLES, MOCK_FEE, MOCK_SIZE))
            .unwrap();
        pool.add_entry(TxEntry::new(tx3, MOCK_CYCLES, MOCK_FEE, MOCK_SIZE))
            .unwrap();
        pool.add_entry(TxEntry::new(tx4, MOCK_CYCLES, MOCK_FEE, MOCK_SIZE))
            .unwrap();
        pool.add_entry(TxEntry::new(tx5, MOCK_CYCLES, MOCK_FEE, MOCK_SIZE))
            .unwrap();

        assert_eq!(pool.edges.inner_len(), 13);
        assert_eq!(pool.edges.outer_len(), 2);

        pool.remove_committed_tx(
            &tx1.transaction,
            &get_related_dep_out_points(&tx1.transaction, |_| None).unwrap(),
        );

        assert_eq!(pool.edges.inner_len(), 10);
        assert_eq!(pool.edges.outer_len(), 4);
    }

    #[test]
    fn test_sorted_by_tx_fee_rate() {
        let tx1 = build_tx(vec![(&Byte32::zero(), 1)], 1);
        let tx2 = build_tx(vec![(&Byte32::zero(), 2)], 1);
        let tx3 = build_tx(vec![(&Byte32::zero(), 3)], 1);

        let mut pool = ProposedPool::new(DEFAULT_MAX_ANCESTORS_SIZE);

        let cycles = 5_000_000;
        let size = 200;

        pool.add_entry(TxEntry::new(
            tx1.clone(),
            cycles,
            Capacity::shannons(100),
            size,
        ))
        .unwrap();
        pool.add_entry(TxEntry::new(
            tx2.clone(),
            cycles,
            Capacity::shannons(300),
            size,
        ))
        .unwrap();
        pool.add_entry(TxEntry::new(
            tx3.clone(),
            cycles,
            Capacity::shannons(200),
            size,
        ))
        .unwrap();

        let txs_sorted_by_fee_rate = pool.with_sorted_by_score_iter(|iter| {
            iter.map(|entry| entry.transaction().hash())
                .collect::<Vec<_>>()
        });
        let expect_result = vec![tx2.hash(), tx3.hash(), tx1.hash()];
        assert_eq!(txs_sorted_by_fee_rate, expect_result);
    }

    #[test]
    fn test_sorted_by_ancestors_score() {
        let tx1 = build_tx(vec![(&Byte32::zero(), 1)], 2);
        let tx1_hash = tx1.hash();
        let tx2 = build_tx(vec![(&tx1_hash, 1)], 1);
        let tx2_hash = tx2.hash();
        let tx3 = build_tx(vec![(&tx1_hash, 2)], 1);
        let tx4 = build_tx(vec![(&tx2_hash, 1)], 1);

        let mut pool = ProposedPool::new(DEFAULT_MAX_ANCESTORS_SIZE);

        let cycles = 5_000_000;
        let size = 200;

        pool.add_entry(TxEntry::new(
            tx1.clone(),
            cycles,
            Capacity::shannons(100),
            size,
        ))
        .unwrap();
        pool.add_entry(TxEntry::new(
            tx2.clone(),
            cycles,
            Capacity::shannons(300),
            size,
        ))
        .unwrap();
        pool.add_entry(TxEntry::new(
            tx3.clone(),
            cycles,
            Capacity::shannons(200),
            size,
        ))
        .unwrap();
        pool.add_entry(TxEntry::new(
            tx4.clone(),
            cycles,
            Capacity::shannons(400),
            size,
        ))
        .unwrap();

        let txs_sorted_by_fee_rate = pool.with_sorted_by_score_iter(|iter| {
            iter.map(|entry| entry.transaction().hash())
                .collect::<Vec<_>>()
        });
        let expect_result = vec![tx4.hash(), tx2.hash(), tx3.hash(), tx1.hash()];
        assert_eq!(txs_sorted_by_fee_rate, expect_result);
    }

    #[test]
    fn test_sorted_by_ancestors_score_competitive() {
        let tx1 = build_tx(vec![(&Byte32::zero(), 1)], 2);
        let tx1_hash = tx1.hash();
        let tx2 = build_tx(vec![(&tx1_hash, 0)], 1);
        let tx2_hash = tx2.hash();
        let tx3 = build_tx(vec![(&tx2_hash, 0)], 1);

        let tx2_1 = build_tx(vec![(&Byte32::zero(), 2)], 2);
        let tx2_1_hash = tx2_1.hash();
        let tx2_2 = build_tx(vec![(&tx2_1_hash, 0)], 1);
        let tx2_2_hash = tx2_2.hash();
        let tx2_3 = build_tx(vec![(&tx2_2_hash, 0)], 1);
        let tx2_3_hash = tx2_3.hash();
        let tx2_4 = build_tx(vec![(&tx2_3_hash, 0)], 1);

        let mut pool = ProposedPool::new(DEFAULT_MAX_ANCESTORS_SIZE);

        // Choose 5_000_839, so the vbytes is 853.0001094046, which will not lead to carry when
        // calculating the vbytes for a package.
        let cycles = 5_000_839;
        let size = 200;

        for &tx in &[&tx1, &tx2, &tx3, &tx2_1, &tx2_2, &tx2_3, &tx2_4] {
            pool.add_entry(TxEntry::new(
                tx.clone(),
                cycles,
                Capacity::shannons(200),
                size,
            ))
            .unwrap();
        }

        let txs_sorted_by_fee_rate = pool.with_sorted_by_score_iter(|iter| {
            iter.map(|entry| format!("{}", entry.transaction().hash()))
                .collect::<Vec<_>>()
        });
        // the entry with most ancestors score will win
        let expect_result = format!("{}", tx2_4.hash());
        assert_eq!(txs_sorted_by_fee_rate[0], expect_result);
    }

    #[test]
    fn test_get_ancestors() {
        let tx1 = build_tx(vec![(&Byte32::zero(), 1)], 2);
        let tx1_hash = tx1.hash();
        let tx2 = build_tx(vec![(&tx1_hash, 0)], 1);
        let tx2_hash = tx2.hash();
        let tx3 = build_tx(vec![(&tx1_hash, 1)], 1);
        let tx4 = build_tx(vec![(&tx2_hash, 0)], 1);

        let mut pool = ProposedPool::new(DEFAULT_MAX_ANCESTORS_SIZE);

        let cycles = 5_000_000;
        let size = 200;

        pool.add_entry(TxEntry::new(
            tx1.clone(),
            cycles,
            Capacity::shannons(100),
            size,
        ))
        .unwrap();
        pool.add_entry(TxEntry::new(
            tx2.clone(),
            cycles,
            Capacity::shannons(300),
            size,
        ))
        .unwrap();
        pool.add_entry(TxEntry::new(
            tx3.clone(),
            cycles,
            Capacity::shannons(200),
            size,
        ))
        .unwrap();
        pool.add_entry(TxEntry::new(
            tx4.clone(),
            cycles,
            Capacity::shannons(400),
            size,
        ))
        .unwrap();

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

        let ancestors = pool.get_ancestors(&tx1.transaction.proposal_short_id());
        assert_eq!(ancestors, Default::default());
        let entry = pool
            .get(&tx1.transaction.proposal_short_id())
            .expect("exists");
        assert_eq!(entry.ancestors_cycles, cycles);
        assert_eq!(entry.ancestors_size, size);
        assert_eq!(entry.ancestors_count, 1);
    }

    #[test]
    fn test_dep_group() {
        let tx1 = build_tx(vec![(&h256!("0x1").pack(), 0)], 1);
        let tx1_out_point = OutPoint::new(tx1.hash(), 0);

        // Dep group cell
        let tx2_data = vec![tx1_out_point.clone()].pack().as_bytes();
        let tx2 = {
            let transaction = TransactionBuilder::default()
                .input(CellInput::new(OutPoint::new(h256!("0x2").pack(), 0), 0))
                .output(
                    CellOutput::new_builder()
                        .capacity(Capacity::bytes(1000).unwrap().pack())
                        .build(),
                )
                .output_data(tx2_data.pack())
                .build();

            ResolvedTransaction {
                transaction,
                resolved_cell_deps: vec![],
                resolved_inputs: vec![],
                resolved_dep_groups: vec![],
            }
        };
        let tx2_out_point = OutPoint::new(tx2.hash(), 0);

        // Transaction use dep group
        let dep = CellDep::new_builder()
            .out_point(tx2_out_point.clone())
            .dep_type(DepType::DepGroup.into())
            .build();
        let tx3 = {
            let transaction = TransactionBuilder::default()
                .cell_dep(dep)
                .input(CellInput::new(OutPoint::new(h256!("0x3").pack(), 0), 0))
                .output(
                    CellOutput::new_builder()
                        .capacity(Capacity::bytes(3).unwrap().pack())
                        .build(),
                )
                .output_data(Bytes::new().pack())
                .build();
            let cell =
                CellMetaBuilder::from_cell_output(tx1.transaction.output(0).unwrap(), Bytes::new())
                    .out_point(tx1_out_point.clone())
                    .build();

            let dep_group = CellMetaBuilder::from_cell_output(
                tx2.transaction.output(0).unwrap(),
                tx2_data.clone(),
            )
            .out_point(tx2_out_point.clone())
            .build();

            ResolvedTransaction {
                transaction,
                resolved_cell_deps: vec![cell],
                resolved_inputs: vec![],
                resolved_dep_groups: vec![dep_group],
            }
        };

        let tx3_out_point = OutPoint::new(tx3.hash(), 0);

        let get_cell_data = |out_point: &OutPoint| -> Option<Bytes> {
            if out_point == &tx2_out_point {
                Some(tx2_data.clone())
            } else {
                None
            }
        };

        let mut pool = ProposedPool::new(DEFAULT_MAX_ANCESTORS_SIZE);
        for tx in &[&tx1, &tx2, &tx3] {
            pool.add_entry(TxEntry::new(
                (*tx).clone(),
                MOCK_CYCLES,
                MOCK_FEE,
                MOCK_SIZE,
            ))
            .unwrap();
        }

        let get_deps_len = |pool: &ProposedPool, out_point: &OutPoint| -> usize {
            pool.edges
                .deps
                .get(out_point)
                .map(|deps| deps.len())
                .unwrap_or_default()
        };
        assert_eq!(get_deps_len(&pool, &tx1_out_point), 1);
        assert_eq!(get_deps_len(&pool, &tx2_out_point), 1);
        assert_eq!(get_deps_len(&pool, &tx3_out_point), 0);

        pool.remove_committed_tx(
            &tx3.transaction,
            &get_related_dep_out_points(&tx3.transaction, &get_cell_data).unwrap(),
        );
        assert_eq!(get_deps_len(&pool, &tx1_out_point), 0);
        assert_eq!(get_deps_len(&pool, &tx2_out_point), 0);
        assert_eq!(get_deps_len(&pool, &tx3_out_point), 0);
    }
}
