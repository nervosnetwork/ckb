use crate::component::container::{AncestorsScoreSortKey, SortedTxMap};
use crate::component::entry::TxEntry;
use crate::error::Reject;
use ckb_fee_estimator::FeeRate;
use ckb_types::{
    core::{
        cell::{CellMetaBuilder, CellProvider, CellStatus},
        TransactionView,
    },
    packed::{OutPoint, ProposalShortId},
    prelude::*,
};
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub(crate) struct PendingQueue {
    inner: SortedTxMap,
}

impl PendingQueue {
    pub(crate) fn new(max_ancestors_count: usize) -> Self {
        PendingQueue {
            inner: SortedTxMap::new(max_ancestors_count),
        }
    }

    pub(crate) fn size(&self) -> usize {
        self.inner.size()
    }

    pub(crate) fn add_entry(&mut self, entry: TxEntry) -> Result<Option<TxEntry>, Reject> {
        self.inner.add_entry(entry)
    }

    pub(crate) fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.inner.contains_key(id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&ProposalShortId, &TxEntry)> {
        self.inner.iter()
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

    pub(crate) fn remove_entry(&mut self, id: &ProposalShortId) -> Option<TxEntry> {
        self.inner.remove_entry(id)
    }

    /// find all ancestors from pool
    pub(crate) fn get_ancestors(&self, tx_short_id: &ProposalShortId) -> HashSet<ProposalShortId> {
        self.inner.get_ancestors(tx_short_id)
    }

    pub(crate) fn keys_sorted_by_fee(&self) -> impl Iterator<Item = &AncestorsScoreSortKey> {
        self.inner.keys_sorted_by_fee()
    }

    pub(crate) fn keys_sorted_by_fee_and_relation(&self) -> Vec<&AncestorsScoreSortKey> {
        self.inner.keys_sorted_by_fee_and_relation()
    }

    // fill proposal txs
    pub fn fill_proposals(
        &self,
        limit: usize,
        min_fee_rate: FeeRate,
        exclusion: &HashSet<ProposalShortId>,
        proposals: &mut HashSet<ProposalShortId>,
    ) {
        for key in self.keys_sorted_by_fee() {
            if proposals.len() == limit {
                break;
            } else if proposals.contains(&key.id)
                || key.ancestors_fee < min_fee_rate.fee(key.ancestors_size)
            {
                // ignore tx which already exists in proposals
                // or fee rate is lower than min fee rate
                continue;
            }
            let mut ancestors = self.get_ancestors(&key.id).into_iter().collect::<Vec<_>>();
            ancestors.sort_unstable_by_key(|id| {
                self.get(&id)
                    .map(|entry| entry.ancestors_count)
                    .expect("exists")
            });
            ancestors.push(key.id.clone());

            for candidate in ancestors {
                if proposals.len() == limit {
                    break;
                }
                if !exclusion.contains(&candidate) {
                    proposals.insert(candidate);
                }
            }
        }
    }
}

impl CellProvider for PendingQueue {
    fn cell(&self, out_point: &OutPoint, with_data: bool) -> CellStatus {
        let tx_hash = out_point.tx_hash();
        if let Some(x) = self.inner.get(&ProposalShortId::from_tx_hash(&tx_hash)) {
            match x.transaction.output_with_data(out_point.index().unpack()) {
                Some((output, data)) => {
                    let mut cell_meta = CellMetaBuilder::from_cell_output(output, data)
                        .out_point(out_point.to_owned())
                        .build();
                    if !with_data {
                        cell_meta.mem_cell_data = None;
                    }
                    CellStatus::live_cell(cell_meta)
                }
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

    const DEFAULT_MAX_ANCESTORS_SIZE: usize = 25;

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

    // Choose 5_000_839, so the vbytes is 853.0001094046, which will not lead to carry when
    // calculating the vbytes for a package.
    const MOCK_CYCLES: Cycle = 5_000_839;
    const MOCK_SIZE: usize = 200;

    #[test]
    fn test_sorted_by_tx_fee_rate() {
        let tx1 = build_tx(vec![(&Byte32::zero(), 1)], 1);
        let tx2 = build_tx(vec![(&Byte32::zero(), 2)], 1);
        let tx3 = build_tx(vec![(&Byte32::zero(), 3)], 1);

        let mut pool = PendingQueue::new(DEFAULT_MAX_ANCESTORS_SIZE);

        pool.add_entry(TxEntry::new(
            tx1.clone(),
            MOCK_CYCLES,
            Capacity::shannons(100),
            MOCK_SIZE,
            vec![],
        ))
        .unwrap();
        pool.add_entry(TxEntry::new(
            tx2.clone(),
            MOCK_CYCLES,
            Capacity::shannons(300),
            MOCK_SIZE,
            vec![],
        ))
        .unwrap();
        pool.add_entry(TxEntry::new(
            tx3.clone(),
            MOCK_CYCLES,
            Capacity::shannons(200),
            MOCK_SIZE,
            vec![],
        ))
        .unwrap();

        let txs_sorted_by_fee_rate = pool
            .keys_sorted_by_fee()
            .map(|key| key.id.clone())
            .collect::<Vec<_>>();
        let expect_result = vec![
            tx2.proposal_short_id(),
            tx3.proposal_short_id(),
            tx1.proposal_short_id(),
        ];
        assert_eq!(txs_sorted_by_fee_rate, expect_result);

        let keys_sorted_by_fee_and_relation = pool
            .keys_sorted_by_fee_and_relation()
            .iter()
            .map(|key| key.id.clone())
            .collect::<Vec<_>>();

        // `keys_sorted_by_fee_and_relation` is same as `txs_sorted_by_fee_rate`,
        // because all the transactions have
        // no relation with each others.
        assert_eq!(keys_sorted_by_fee_and_relation, txs_sorted_by_fee_rate);
    }

    #[test]
    fn test_sorted_by_ancestors_score() {
        let tx1 = build_tx(vec![(&Byte32::zero(), 1)], 2);
        let tx1_hash = tx1.hash();
        let tx2 = build_tx(vec![(&tx1_hash, 1)], 1);
        let tx2_hash = tx2.hash();
        let tx3 = build_tx(vec![(&tx1_hash, 2)], 1);
        let tx4 = build_tx(vec![(&tx2_hash, 1)], 1);

        let mut pool = PendingQueue::new(DEFAULT_MAX_ANCESTORS_SIZE);

        pool.add_entry(TxEntry::new(
            tx1.clone(),
            MOCK_CYCLES,
            Capacity::shannons(100),
            MOCK_SIZE,
            vec![],
        ))
        .unwrap();
        pool.add_entry(TxEntry::new(
            tx2.clone(),
            MOCK_CYCLES,
            Capacity::shannons(300),
            MOCK_SIZE,
            vec![],
        ))
        .unwrap();
        pool.add_entry(TxEntry::new(
            tx3.clone(),
            MOCK_CYCLES,
            Capacity::shannons(200),
            MOCK_SIZE,
            vec![],
        ))
        .unwrap();
        pool.add_entry(TxEntry::new(
            tx4.clone(),
            MOCK_CYCLES,
            Capacity::shannons(400),
            MOCK_SIZE,
            vec![],
        ))
        .unwrap();

        let txs_sorted_by_fee_rate = pool
            .keys_sorted_by_fee()
            .map(|key| key.id.clone())
            .collect::<Vec<_>>();
        let expect_result = vec![
            tx4.proposal_short_id(),
            tx2.proposal_short_id(),
            tx3.proposal_short_id(),
            tx1.proposal_short_id(),
        ];
        assert_eq!(txs_sorted_by_fee_rate, expect_result);

        let keys_sorted_by_fee_and_relation = pool
            .keys_sorted_by_fee_and_relation()
            .iter()
            .map(|key| key.id.clone())
            .collect::<Vec<_>>();

        // The best expect_result is tx1, tx2, tx4, tx3.
        // Because tx4 fee_rate is better than tx3 and
        // they don't have the dependency relation.
        // Here we make a compromise.
        let expect_result = vec![
            tx1.proposal_short_id(),
            tx2.proposal_short_id(),
            tx3.proposal_short_id(),
            tx4.proposal_short_id(),
        ];
        assert_eq!(keys_sorted_by_fee_and_relation, expect_result);
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

        let mut pool = PendingQueue::new(DEFAULT_MAX_ANCESTORS_SIZE);

        for &tx in &[&tx1, &tx2, &tx3, &tx2_1, &tx2_2, &tx2_3, &tx2_4] {
            pool.add_entry(TxEntry::new(
                tx.clone(),
                MOCK_CYCLES,
                Capacity::shannons(200),
                MOCK_SIZE,
                vec![],
            ))
            .unwrap();
        }

        let txs_sorted_by_fee_rate = pool
            .keys_sorted_by_fee()
            .map(|key| key.id.clone())
            .collect::<Vec<_>>();

        let expect_result = vec![
            tx2_4.proposal_short_id(),
            tx3.proposal_short_id(),
            tx2_3.proposal_short_id(),
            tx2.proposal_short_id(),
            tx2_2.proposal_short_id(),
            tx1.proposal_short_id(),
            tx2_1.proposal_short_id(),
        ];
        assert_eq!(txs_sorted_by_fee_rate, expect_result);

        let keys_sorted_by_fee_and_relation = pool
            .keys_sorted_by_fee_and_relation()
            .iter()
            .map(|key| key.id.clone())
            .collect::<Vec<_>>();

        let expect_result = vec![
            tx1.proposal_short_id(),
            tx2_1.proposal_short_id(),
            tx2.proposal_short_id(),
            tx2_2.proposal_short_id(),
            tx3.proposal_short_id(),
            tx2_3.proposal_short_id(),
            tx2_4.proposal_short_id(),
        ];
        assert_eq!(keys_sorted_by_fee_and_relation, expect_result);
    }
}
