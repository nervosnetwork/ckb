use crate::component::container::SortedTxMap;
use crate::component::entry::TxEntry;
use ckb_types::{
    core::{
        cell::{CellMetaBuilder, CellProvider, CellStatus},
        TransactionView,
    },
    packed::{OutPoint, ProposalShortId},
    prelude::*,
};
use std::collections::HashSet;

#[derive(Default, Debug, Clone)]
pub(crate) struct PendingQueue {
    inner: SortedTxMap,
}

impl PendingQueue {
    pub(crate) fn new() -> Self {
        PendingQueue {
            inner: Default::default(),
        }
    }

    pub(crate) fn size(&self) -> usize {
        self.inner.size()
    }

    pub(crate) fn add_entry(&mut self, entry: TxEntry) -> Option<TxEntry> {
        self.inner.add_entry(entry)
    }

    pub(crate) fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.inner.contains_key(id)
    }

    pub(crate) fn get(&self, id: &ProposalShortId) -> Option<&TxEntry> {
        self.inner.get(id)
    }

    pub(crate) fn get_tx(&self, id: &ProposalShortId) -> Option<&TransactionView> {
        self.inner.get(id).map(|x| &x.transaction)
    }

    pub(crate) fn remove_entry_and_descendants(&mut self, id: &ProposalShortId) -> Vec<TxEntry> {
        self.inner.remove_entry_and_descendants(id)
    }

    /// find all ancestors from pool
    pub(crate) fn get_ancestors(&self, tx_short_id: &ProposalShortId) -> HashSet<ProposalShortId> {
        self.inner.get_ancestors(tx_short_id)
    }

    pub(crate) fn sorted_keys(&self) -> impl Iterator<Item = &ProposalShortId> {
        self.inner.sorted_keys().map(|key| &key.id)
    }

    // fill proposal txs
    pub fn fill_proposals(&self, limit: usize, proposals: &mut HashSet<ProposalShortId>) {
        for id in self.sorted_keys() {
            if proposals.len() == limit {
                break;
            } else if proposals.contains(&id) {
                // implies that ancestors are already in proposals
                continue;
            }
            let mut ancestors = self.get_ancestors(&id).into_iter().collect::<Vec<_>>();
            ancestors.sort_unstable_by_key(|id| {
                self.get(&id)
                    .map(|entry| entry.ancestors_count)
                    .expect("exists")
            });
            ancestors.push(id.clone());
            proposals.extend(ancestors.into_iter().take(limit - proposals.len()));
        }
    }
}

impl CellProvider for PendingQueue {
    fn cell(&self, out_point: &OutPoint, _with_data: bool) -> CellStatus {
        let tx_hash = out_point.tx_hash();
        if let Some(x) = self.inner.get(&ProposalShortId::from_tx_hash(&tx_hash)) {
            match x.transaction.output_with_data(out_point.index().unpack()) {
                Some((output, data)) => CellStatus::live_cell(
                    CellMetaBuilder::from_cell_output(output.to_owned(), data)
                        .out_point(out_point.to_owned())
                        .build(),
                ),
                None => CellStatus::Unknown,
            }
        } else {
            CellStatus::Unknown
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_types::{
        bytes::Bytes,
        core::{Capacity, Cycle, TransactionBuilder},
        packed::{Byte32, CellInput, CellOutputBuilder},
    };

    fn build_tx(inputs: Vec<(&Byte32, u32)>, outputs_len: usize) -> TransactionView {
        TransactionBuilder::default()
            .inputs(
                inputs
                    .into_iter()
                    .map(|(txid, index)| CellInput::new(OutPoint::new(txid.to_owned(), index), 0)),
            )
            .outputs((0..outputs_len).map(|i| {
                CellOutputBuilder::default()
                    .capacity(Capacity::bytes(i + 1).unwrap().pack())
                    .build()
            }))
            .outputs_data((0..outputs_len).map(|_| Bytes::new().pack()))
            .build()
    }

    const MOCK_CYCLES: Cycle = 5_000_000;
    const MOCK_SIZE: usize = 200;

    #[test]
    fn test_sorted_by_tx_fee_rate() {
        let tx1 = build_tx(vec![(&Byte32::zero(), 1)], 1);
        let tx2 = build_tx(vec![(&Byte32::zero(), 2)], 1);
        let tx3 = build_tx(vec![(&Byte32::zero(), 3)], 1);

        let mut pool = PendingQueue::new();

        pool.add_entry(TxEntry::new(
            tx1.clone(),
            MOCK_CYCLES,
            Capacity::shannons(100),
            MOCK_SIZE,
            vec![],
        ));
        pool.add_entry(TxEntry::new(
            tx2.clone(),
            MOCK_CYCLES,
            Capacity::shannons(300),
            MOCK_SIZE,
            vec![],
        ));
        pool.add_entry(TxEntry::new(
            tx3.clone(),
            MOCK_CYCLES,
            Capacity::shannons(200),
            MOCK_SIZE,
            vec![],
        ));

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
        let tx1 = build_tx(vec![(&Byte32::zero(), 1)], 2);
        let tx1_hash = tx1.hash();
        let tx2 = build_tx(vec![(&tx1_hash, 1)], 1);
        let tx2_hash = tx2.hash();
        let tx3 = build_tx(vec![(&tx1_hash, 2)], 1);
        let tx4 = build_tx(vec![(&tx2_hash, 1)], 1);

        let mut pool = PendingQueue::new();

        pool.add_entry(TxEntry::new(
            tx1.clone(),
            MOCK_CYCLES,
            Capacity::shannons(100),
            MOCK_SIZE,
            vec![],
        ));
        pool.add_entry(TxEntry::new(
            tx2.clone(),
            MOCK_CYCLES,
            Capacity::shannons(300),
            MOCK_SIZE,
            vec![],
        ));
        pool.add_entry(TxEntry::new(
            tx3.clone(),
            MOCK_CYCLES,
            Capacity::shannons(200),
            MOCK_SIZE,
            vec![],
        ));
        pool.add_entry(TxEntry::new(
            tx4.clone(),
            MOCK_CYCLES,
            Capacity::shannons(400),
            MOCK_SIZE,
            vec![],
        ));

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

        let mut pool = PendingQueue::new();

        for &tx in &[&tx1, &tx2, &tx3, &tx2_1, &tx2_2, &tx2_3, &tx2_4] {
            pool.add_entry(TxEntry::new(
                tx.clone(),
                MOCK_CYCLES,
                Capacity::shannons(200),
                MOCK_SIZE,
                vec![],
            ));
        }

        let txs_sorted_by_fee_rate = pool.sorted_keys().cloned().collect::<Vec<_>>();
        // the entry with most ancestors score will win
        let expect_result = tx2_4.proposal_short_id();
        assert_eq!(txs_sorted_by_fee_rate[0], expect_result);
    }
}
