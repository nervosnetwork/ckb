use crate::tx_pool::types::{AncestorsScoreSortKey, PendingEntry, TxLink};
use ckb_core::cell::{CellMetaBuilder, CellProvider, CellStatus};
use ckb_core::transaction::{OutPoint, ProposalShortId, Transaction};
use ckb_core::{Capacity, Cycle};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

#[derive(Default, Debug, Clone)]
pub(crate) struct PendingQueue {
    inner: HashMap<ProposalShortId, PendingEntry>,
    sorted_index: BTreeSet<AncestorsScoreSortKey>,
    /// A map track transaction ancestors and descendants
    links: HashMap<ProposalShortId, TxLink>,
}

impl PendingQueue {
    pub(crate) fn new() -> Self {
        PendingQueue {
            inner: HashMap::default(),
            sorted_index: Default::default(),
            links: Default::default(),
        }
    }

    pub(crate) fn size(&self) -> usize {
        self.inner.len()
    }

    /// update entry ancestor prefix fields
    fn update_ancestors_stat_for_entry(
        &self,
        entry: &mut PendingEntry,
        parents: &HashSet<ProposalShortId>,
    ) {
        for id in parents {
            let tx_entry = self.inner.get(&id).expect("pool consistent");
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

    pub(crate) fn add_tx(
        &mut self,
        cycles: Cycle,
        fee: Capacity,
        size: usize,
        tx: Transaction,
    ) -> Option<PendingEntry> {
        let short_id = tx.proposal_short_id();
        let mut parents: HashSet<ProposalShortId> =
            HashSet::with_capacity(tx.inputs().len() + tx.deps().len());
        for input in tx.inputs() {
            let parent_hash = &input
                .previous_output
                .cell
                .as_ref()
                .expect("cell outpoint")
                .tx_hash;
            let id = ProposalShortId::from_tx_hash(&parent_hash);
            if self.links.contains_key(&id) {
                parents.insert(id);
            }
        }
        for dep in tx.deps() {
            if let Some(cell_output) = &dep.cell {
                let id = ProposalShortId::from_tx_hash(&cell_output.tx_hash);
                if self.links.contains_key(&id) {
                    parents.insert(id);
                }
            }
        }
        let mut entry = PendingEntry::new(tx, cycles, fee, size);
        // update ancestor_fields
        self.update_ancestors_stat_for_entry(&mut entry, &parents);
        // insert links
        self.links.insert(
            short_id,
            TxLink {
                parents,
                children: Default::default(),
            },
        );
        self.sorted_index
            .insert(AncestorsScoreSortKey::from(&entry));
        self.inner.insert(short_id, entry)
    }

    pub(crate) fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.inner.contains_key(id)
    }

    pub(crate) fn get(&self, id: &ProposalShortId) -> Option<&PendingEntry> {
        self.inner.get(id)
    }

    pub(crate) fn get_tx(&self, id: &ProposalShortId) -> Option<&Transaction> {
        self.get(id).map(|x| &x.transaction)
    }

    pub(crate) fn remove_entry_and_descendants(
        &mut self,
        id: &ProposalShortId,
    ) -> Vec<PendingEntry> {
        let mut queue = VecDeque::new();
        let mut removed = Vec::new();
        queue.push_back(*id);
        while let Some(id) = queue.pop_front() {
            if let Some(entry) = self.inner.remove(&id) {
                let deleted = self
                    .sorted_index
                    .remove(&AncestorsScoreSortKey::from(&entry));
                debug_assert!(deleted, "pending pool inconsistent");
                if let Some(link) = self.links.remove(&id) {
                    queue.extend(link.children);
                }
                removed.push(entry);
            }
        }
        removed
    }

    /// find all ancestors from pool
    pub(crate) fn get_ancestors(&self, tx_short_id: &ProposalShortId) -> HashSet<ProposalShortId> {
        TxLink::get_ancestors(&self.links, tx_short_id)
    }

    pub(crate) fn sorted_keys(&self) -> impl Iterator<Item = &ProposalShortId> {
        self.sorted_index.iter().rev().map(|key| &key.id)
    }
}

impl CellProvider for PendingQueue {
    fn cell(&self, o: &OutPoint) -> CellStatus {
        if let Some(cell_out_point) = &o.cell {
            if let Some(x) = self
                .inner
                .get(&ProposalShortId::from_tx_hash(&cell_out_point.tx_hash))
            {
                match x
                    .transaction
                    .get_output_with_data(cell_out_point.index as usize)
                {
                    Some((output, data)) => CellStatus::live_cell(
                        CellMetaBuilder::from_cell_output(output.to_owned(), data)
                            .out_point(cell_out_point.to_owned())
                            .build(),
                    ),
                    None => CellStatus::Unknown,
                }
            } else {
                CellStatus::Unknown
            }
        } else {
            CellStatus::Unspecified
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_core::transaction::{CellInput, CellOutputBuilder, Transaction, TransactionBuilder};
    use ckb_core::{Bytes, Capacity};
    use numext_fixed_hash::H256;

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

    const MOCK_CYCLES: Cycle = 5_000_000;
    const MOCK_SIZE: usize = 200;

    #[test]
    fn test_sorted_by_tx_fee_rate() {
        let tx1 = build_tx(vec![(&H256::zero(), 1)], 1);
        let tx2 = build_tx(vec![(&H256::zero(), 2)], 1);
        let tx3 = build_tx(vec![(&H256::zero(), 3)], 1);

        let mut pool = PendingQueue::new();

        pool.add_tx(MOCK_CYCLES, Capacity::shannons(100), MOCK_SIZE, tx1.clone());
        pool.add_tx(MOCK_CYCLES, Capacity::shannons(300), MOCK_SIZE, tx2.clone());
        pool.add_tx(MOCK_CYCLES, Capacity::shannons(200), MOCK_SIZE, tx3.clone());

        let txs_sorted_by_fee_rate = pool.sorted_keys().cloned().collect::<Vec<_>>();
        let expect_result = vec![
            tx2.proposal_short_id(),
            tx3.proposal_short_id(),
            tx1.proposal_short_id(),
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

        let mut pool = PendingQueue::new();

        pool.add_tx(MOCK_CYCLES, Capacity::shannons(100), MOCK_SIZE, tx1.clone());
        pool.add_tx(MOCK_CYCLES, Capacity::shannons(300), MOCK_SIZE, tx2.clone());
        pool.add_tx(MOCK_CYCLES, Capacity::shannons(200), MOCK_SIZE, tx3.clone());
        pool.add_tx(MOCK_CYCLES, Capacity::shannons(400), MOCK_SIZE, tx4.clone());

        let txs_sorted_by_fee_rate = pool.sorted_keys().cloned().collect::<Vec<_>>();
        let expect_result = vec![
            tx4.proposal_short_id(),
            tx2.proposal_short_id(),
            tx3.proposal_short_id(),
            tx1.proposal_short_id(),
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

        let mut pool = PendingQueue::new();

        for &tx in &[&tx1, &tx2, &tx3, &tx2_1, &tx2_2, &tx2_3, &tx2_4] {
            pool.add_tx(MOCK_CYCLES, Capacity::shannons(200), MOCK_SIZE, tx.clone());
        }

        let txs_sorted_by_fee_rate = pool.sorted_keys().cloned().collect::<Vec<_>>();
        // the entry with most ancestors score will win
        let expect_result = tx2_4.proposal_short_id();
        assert_eq!(txs_sorted_by_fee_rate[0], expect_result);
    }
}
