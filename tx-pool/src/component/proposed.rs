use crate::component::container::SortedTxMap;
use crate::component::entry::TxEntry;
use crate::error::Reject;
use ckb_types::{
    bytes::Bytes,
    core::{
        cell::{CellMeta, CellMetaBuilder, CellProvider, CellStatus, ResolvedTransaction},
        error::OutPointError,
        TransactionView,
    },
    packed::{CellOutput, OutPoint, ProposalShortId},
    prelude::*,
};
use std::collections::{HashMap, HashSet};
use std::iter;

type ConflictEntry = (TxEntry, Reject);

#[derive(Default, Debug, Clone)]
pub(crate) struct Edges {
    /// output-op<txid> map represent in-pool tx's outputs
    pub(crate) outputs: HashMap<OutPoint, Option<ProposalShortId>>,
    /// input-op<txid> map represent in-pool tx's inputs
    pub(crate) inputs: HashMap<OutPoint, Option<ProposalShortId>>,
    /// dep-set<txid> map represent in-pool tx's deps
    pub(crate) deps: HashMap<OutPoint, HashSet<ProposalShortId>>,
}

impl Edges {
    #[cfg(test)]
    pub(crate) fn outputs_len(&self) -> usize {
        self.outputs.len()
    }

    #[cfg(test)]
    pub(crate) fn inputs_len(&self) -> usize {
        self.inputs.len()
    }

    pub(crate) fn insert_input(&mut self, out_point: OutPoint, txid: ProposalShortId) {
        self.inputs.insert(out_point, Some(txid));
    }

    pub(crate) fn remove_input(&mut self, out_point: &OutPoint) -> Option<ProposalShortId> {
        self.inputs.remove(out_point).unwrap_or(None)
    }

    pub(crate) fn remove_output(&mut self, out_point: &OutPoint) -> Option<ProposalShortId> {
        self.outputs.remove(out_point).unwrap_or(None)
    }

    pub(crate) fn insert_output(&mut self, out_point: OutPoint) {
        self.outputs.insert(out_point, None);
    }

    pub(crate) fn get_output_ref(&self, out_point: &OutPoint) -> Option<&Option<ProposalShortId>> {
        self.outputs.get(out_point)
    }

    pub(crate) fn get_input_ref(&self, out_point: &OutPoint) -> Option<&Option<ProposalShortId>> {
        self.inputs.get(out_point)
    }

    pub(crate) fn get_output_mut_ref(
        &mut self,
        out_point: &OutPoint,
    ) -> Option<&mut Option<ProposalShortId>> {
        self.outputs.get_mut(out_point)
    }

    pub(crate) fn remove_deps(&mut self, out_point: &OutPoint) -> Option<HashSet<ProposalShortId>> {
        self.deps.remove(out_point)
    }

    pub(crate) fn insert_deps(&mut self, out_point: OutPoint, txid: ProposalShortId) {
        self.deps.entry(out_point).or_default().insert(txid);
    }

    pub(crate) fn delete_txid_by_dep(&mut self, out_point: &OutPoint, txid: &ProposalShortId) {
        let mut empty = false;

        if let Some(x) = self.deps.get_mut(out_point) {
            x.remove(txid);
            empty = x.is_empty();
        }

        if empty {
            self.deps.remove(out_point);
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProposedPool {
    pub(crate) edges: Edges,
    inner: SortedTxMap,
}

impl CellProvider for ProposedPool {
    fn cell(&self, out_point: &OutPoint, with_data: bool) -> CellStatus {
        if let Some(x) = self.edges.get_output_ref(out_point) {
            // output consumed
            if x.is_some() {
                CellStatus::Dead
            } else {
                let (output, data) = self.get_output_with_data(out_point).expect("output");
                let mut cell_meta = CellMetaBuilder::from_cell_output(output, data)
                    .out_point(out_point.to_owned())
                    .build();
                if !with_data {
                    cell_meta.mem_cell_data_hash = None;
                }
                CellStatus::live_cell(cell_meta)
            }
        } else if self.edges.get_input_ref(out_point).is_some() {
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

    pub fn iter(&self) -> impl Iterator<Item = (&ProposalShortId, &TxEntry)> {
        self.inner.iter()
    }

    pub(crate) fn get_tx(&self, id: &ProposalShortId) -> Option<&TransactionView> {
        self.get(id).map(|entry| entry.transaction())
    }

    pub fn size(&self) -> usize {
        self.inner.size()
    }

    pub(crate) fn get_output_with_data(&self, out_point: &OutPoint) -> Option<(CellOutput, Bytes)> {
        self.inner
            .get(&ProposalShortId::from_tx_hash(&out_point.tx_hash()))
            .and_then(|entry| {
                entry
                    .transaction()
                    .output_with_data(out_point.index().unpack())
            })
    }

    // remove entry and all it's descendants
    pub(crate) fn remove_entry_and_descendants(&mut self, id: &ProposalShortId) -> Vec<TxEntry> {
        let removed_entries = self.inner.remove_entry_and_descendants(id);
        for entry in &removed_entries {
            let tx = entry.transaction();
            let inputs = tx.input_pts_iter();
            let outputs = tx.output_pts();
            for i in inputs {
                if self.edges.outputs.remove(&i).is_none() {
                    self.edges.inputs.remove(&i);
                }
            }

            for d in entry.related_dep_out_points() {
                self.edges.delete_txid_by_dep(d, &id);
            }

            for o in outputs {
                self.edges.remove_output(&o);
                // self.edges.remove_deps(&o);
            }
        }
        removed_entries
    }

    pub(crate) fn remove_committed_tx(
        &mut self,
        tx: &TransactionView,
        related_out_points: &[OutPoint],
    ) -> Option<TxEntry> {
        let outputs = tx.output_pts();
        let inputs = tx.input_pts_iter();
        // TODO: handle header deps
        let id = tx.proposal_short_id();

        if let Some(entry) = self.inner.remove_entry(&id) {
            for o in outputs {
                // notice: cause tx removed by committed,
                // remove output, but if this output consumed by other in-pool tx,
                // we need record it to intputs' map
                if let Some(cid) = self.edges.remove_output(&o) {
                    self.edges.insert_input(o.clone(), cid);
                }
            }

            for i in inputs {
                // release input record
                self.edges.remove_input(&i);
            }

            for d in related_out_points {
                self.edges.delete_txid_by_dep(&d, &id);
            }

            return Some(entry);
        }
        None
    }

    pub(crate) fn add_entry(&mut self, entry: TxEntry) -> Result<Option<TxEntry>, Reject> {
        let inputs = entry.transaction().input_pts_iter();
        let outputs = entry.transaction().output_pts();

        let tx_short_id = entry.proposal_short_id();

        // if input reference a in-pool output, connnect it
        // otherwise, record input for conflict check
        for i in inputs {
            if let Some(id) = self.edges.get_output_mut_ref(&i) {
                *id = Some(tx_short_id.clone());
            } else {
                self.edges.insert_input(i.to_owned(), tx_short_id.clone());
            }
        }

        // record dep-txid
        for d in entry.related_dep_out_points() {
            self.edges.insert_deps(d.to_owned(), tx_short_id.clone());
        }

        // record tx unconsumed output
        for o in outputs {
            self.edges.insert_output(o);
        }

        self.inner.add_entry(entry)
    }

    pub(crate) fn resolve_conflict(
        &mut self,
        tx: &TransactionView,
    ) -> (Vec<ConflictEntry>, Vec<ConflictEntry>) {
        let inputs = tx.input_pts_iter();
        let mut input_conflict = Vec::new();
        let mut deps_consumed = Vec::new();

        for i in inputs {
            if let Some(id) = self.edges.remove_input(&i) {
                let entries = self.remove_entry_and_descendants(&id);
                if !entries.is_empty() {
                    let reject = Reject::Resolve(OutPointError::Dead(i.clone()));
                    let rejects = iter::repeat(reject).take(entries.len());
                    input_conflict.extend(entries.into_iter().zip(rejects));
                }
            }

            // deps consumed
            if let Some(x) = self.edges.remove_deps(&i) {
                for id in x {
                    let entries = self.remove_entry_and_descendants(&id);
                    if !entries.is_empty() {
                        let reject = Reject::Resolve(OutPointError::Dead(i.clone()));
                        let rejects = iter::repeat(reject).take(entries.len());
                        deps_consumed.extend(entries.into_iter().zip(rejects));
                    }
                }
            }
        }
        (input_conflict, deps_consumed)
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
            cell::get_related_dep_out_points, Capacity, Cycle, DepType, TransactionBuilder,
            TransactionView,
        },
        h256,
        packed::{Byte32, CellDep, CellInput, CellOutput},
        H256,
    };

    const DEFAULT_MAX_ANCESTORS_SIZE: usize = 25;

    fn build_tx(inputs: Vec<(&Byte32, u32)>, outputs_len: usize) -> TransactionView {
        TransactionBuilder::default()
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
            .build()
    }

    const MOCK_CYCLES: Cycle = 0;
    const MOCK_FEE: Capacity = Capacity::zero();
    const MOCK_SIZE: usize = 0;

    fn dummy_resolve<F: Fn(&OutPoint) -> Option<Bytes>>(
        tx: TransactionView,
        get_cell_data: F,
    ) -> ResolvedTransaction {
        let resolved_cell_deps = get_related_dep_out_points(&tx, get_cell_data)
            .expect("dummy resolve")
            .into_iter()
            .map(|out_point| {
                CellMeta {
                    cell_output: CellOutput::new_builder().build(),
                    out_point,
                    transaction_info: None,
                    data_bytes: 0,
                    mem_cell_data: None,
                    mem_cell_data_hash: None, // make sure load_cell_data_hash works within block
                }
            })
            .collect();

        ResolvedTransaction {
            transaction: tx,
            resolved_cell_deps,
            resolved_inputs: vec![],
            resolved_dep_groups: vec![],
        }
    }

    #[test]
    fn test_add_entry() {
        let tx1 = build_tx(vec![(&Byte32::zero(), 1), (&Byte32::zero(), 2)], 1);
        let tx1_hash = tx1.hash();
        let tx2 = build_tx(vec![(&tx1_hash, 0)], 1);

        let mut pool = ProposedPool::new(DEFAULT_MAX_ANCESTORS_SIZE);

        pool.add_entry(TxEntry::new(
            dummy_resolve(tx1.clone(), |_| None),
            MOCK_CYCLES,
            MOCK_FEE,
            MOCK_SIZE,
        ))
        .unwrap();
        pool.add_entry(TxEntry::new(
            dummy_resolve(tx2.clone(), |_| None),
            MOCK_CYCLES,
            MOCK_FEE,
            MOCK_SIZE,
        ))
        .unwrap();

        assert_eq!(pool.size(), 2);
        assert_eq!(pool.edges.outputs_len(), 2);
        assert_eq!(pool.edges.inputs_len(), 2);

        pool.remove_committed_tx(&tx1, &get_related_dep_out_points(&tx1, |_| None).unwrap());
        assert_eq!(pool.edges.outputs_len(), 1);
        assert_eq!(pool.edges.inputs_len(), 1);
    }

    #[test]
    fn test_add_roots() {
        let tx1 = build_tx(vec![(&Byte32::zero(), 1), (&Byte32::zero(), 2)], 1);
        let tx2 = build_tx(
            vec![(&h256!("0x2").pack(), 1), (&h256!("0x3").pack(), 2)],
            3,
        );

        let mut pool = ProposedPool::new(DEFAULT_MAX_ANCESTORS_SIZE);

        pool.add_entry(TxEntry::new(
            dummy_resolve(tx1.clone(), |_| None),
            MOCK_CYCLES,
            MOCK_FEE,
            MOCK_SIZE,
        ))
        .unwrap();
        pool.add_entry(TxEntry::new(
            dummy_resolve(tx2.clone(), |_| None),
            MOCK_CYCLES,
            MOCK_FEE,
            MOCK_SIZE,
        ))
        .unwrap();

        assert_eq!(pool.edges.outputs_len(), 4);
        assert_eq!(pool.edges.inputs_len(), 4);

        pool.remove_committed_tx(&tx1, &get_related_dep_out_points(&tx1, |_| None).unwrap());

        assert_eq!(pool.edges.outputs_len(), 3);
        assert_eq!(pool.edges.inputs_len(), 2);
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

        pool.add_entry(TxEntry::new(
            dummy_resolve(tx1.clone(), |_| None),
            MOCK_CYCLES,
            MOCK_FEE,
            MOCK_SIZE,
        ))
        .unwrap();
        pool.add_entry(TxEntry::new(
            dummy_resolve(tx2.clone(), |_| None),
            MOCK_CYCLES,
            MOCK_FEE,
            MOCK_SIZE,
        ))
        .unwrap();
        pool.add_entry(TxEntry::new(
            dummy_resolve(tx3.clone(), |_| None),
            MOCK_CYCLES,
            MOCK_FEE,
            MOCK_SIZE,
        ))
        .unwrap();
        pool.add_entry(TxEntry::new(
            dummy_resolve(tx4.clone(), |_| None),
            MOCK_CYCLES,
            MOCK_FEE,
            MOCK_SIZE,
        ))
        .unwrap();
        pool.add_entry(TxEntry::new(
            dummy_resolve(tx5.clone(), |_| None),
            MOCK_CYCLES,
            MOCK_FEE,
            MOCK_SIZE,
        ))
        .unwrap();

        assert_eq!(pool.edges.outputs_len(), 13);
        assert_eq!(pool.edges.inputs_len(), 2);

        pool.remove_committed_tx(&tx1, &get_related_dep_out_points(&tx1, |_| None).unwrap());

        assert_eq!(pool.edges.outputs_len(), 10);
        assert_eq!(pool.edges.inputs_len(), 4);
    }

    #[test]
    fn test_sorted_by_tx_fee_rate() {
        let tx1 = build_tx(vec![(&Byte32::zero(), 1)], 1);
        let tx2 = build_tx(vec![(&Byte32::zero(), 2)], 1);
        let tx3 = build_tx(vec![(&Byte32::zero(), 3)], 1);

        let mut pool = ProposedPool::new(DEFAULT_MAX_ANCESTORS_SIZE);

        let cycles = 5_000_000;
        let size = 200;

        pool.add_entry(TxEntry::dummy_resolve(
            tx1.clone(),
            cycles,
            Capacity::shannons(100),
            size,
        ))
        .unwrap();
        pool.add_entry(TxEntry::dummy_resolve(
            tx2.clone(),
            cycles,
            Capacity::shannons(300),
            size,
        ))
        .unwrap();
        pool.add_entry(TxEntry::dummy_resolve(
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

        pool.add_entry(TxEntry::dummy_resolve(
            tx1.clone(),
            cycles,
            Capacity::shannons(100),
            size,
        ))
        .unwrap();
        pool.add_entry(TxEntry::dummy_resolve(
            tx2.clone(),
            cycles,
            Capacity::shannons(300),
            size,
        ))
        .unwrap();
        pool.add_entry(TxEntry::dummy_resolve(
            tx3.clone(),
            cycles,
            Capacity::shannons(200),
            size,
        ))
        .unwrap();
        pool.add_entry(TxEntry::dummy_resolve(
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
            pool.add_entry(TxEntry::dummy_resolve(
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

        pool.add_entry(TxEntry::dummy_resolve(
            tx1.clone(),
            cycles,
            Capacity::shannons(100),
            size,
        ))
        .unwrap();
        pool.add_entry(TxEntry::dummy_resolve(
            tx2.clone(),
            cycles,
            Capacity::shannons(300),
            size,
        ))
        .unwrap();
        pool.add_entry(TxEntry::dummy_resolve(
            tx3.clone(),
            cycles,
            Capacity::shannons(200),
            size,
        ))
        .unwrap();
        pool.add_entry(TxEntry::dummy_resolve(
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

        let ancestors = pool.get_ancestors(&tx1.proposal_short_id());
        assert_eq!(ancestors, Default::default());
        let entry = pool.get(&tx1.proposal_short_id()).expect("exists");
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
        let tx2 = TransactionBuilder::default()
            .input(CellInput::new(OutPoint::new(h256!("0x2").pack(), 0), 0))
            .output(
                CellOutput::new_builder()
                    .capacity(Capacity::bytes(1000).unwrap().pack())
                    .build(),
            )
            .output_data(tx2_data.pack())
            .build();
        let tx2_out_point = OutPoint::new(tx2.hash(), 0);

        // Transaction use dep group
        let dep = CellDep::new_builder()
            .out_point(tx2_out_point.clone())
            .dep_type(DepType::DepGroup.into())
            .build();
        let tx3 = TransactionBuilder::default()
            .cell_dep(dep)
            .input(CellInput::new(OutPoint::new(h256!("0x3").pack(), 0), 0))
            .output(
                CellOutput::new_builder()
                    .capacity(Capacity::bytes(3).unwrap().pack())
                    .build(),
            )
            .output_data(Bytes::new().pack())
            .build();
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
                dummy_resolve((*tx).clone(), get_cell_data),
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
            &tx3,
            &get_related_dep_out_points(&tx3, &get_cell_data).unwrap(),
        );
        assert_eq!(get_deps_len(&pool, &tx1_out_point), 0);
        assert_eq!(get_deps_len(&pool, &tx2_out_point), 0);
        assert_eq!(get_deps_len(&pool, &tx3_out_point), 0);
    }
}
