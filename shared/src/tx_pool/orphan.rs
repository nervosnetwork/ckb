#![allow(dead_code)]

use crate::tx_pool::types::PoolEntry;
use ckb_core::transaction::{OutPoint, ProposalShortId, Transaction};
use ckb_core::Cycle;
use fnv::FnvHashMap;
use std::collections::hash_map;
use std::collections::VecDeque;
use std::iter::ExactSizeIterator;

///not verified, may contain conflict transactions
#[derive(Default, Debug, Clone)]
pub(crate) struct OrphanPool {
    pub(crate) vertices: FnvHashMap<ProposalShortId, PoolEntry>,
    pub(crate) edges: FnvHashMap<OutPoint, Vec<ProposalShortId>>,
}

impl OrphanPool {
    pub(crate) fn new() -> Self {
        OrphanPool::default()
    }

    pub(crate) fn capacity(&self) -> usize {
        self.vertices.len()
    }

    pub(crate) fn get(&self, id: &ProposalShortId) -> Option<&PoolEntry> {
        self.vertices.get(id)
    }

    pub(crate) fn get_tx(&self, id: &ProposalShortId) -> Option<&Transaction> {
        self.get(id).map(|x| &x.transaction)
    }

    pub(crate) fn contains(&self, tx: &Transaction) -> bool {
        self.vertices.contains_key(&tx.proposal_short_id())
    }

    pub(crate) fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.vertices.contains_key(id)
    }

    /// add orphan transaction
    pub(crate) fn add_tx(
        &mut self,
        cycles: Option<Cycle>,
        tx: Transaction,
        unknown: impl ExactSizeIterator<Item = OutPoint>,
    ) {
        let short_id = tx.proposal_short_id();
        let entry = PoolEntry::new(tx, unknown.len(), cycles);
        for out_point in unknown {
            let edge = self.edges.entry(out_point).or_insert_with(Vec::new);
            edge.push(short_id);
        }
        self.vertices.insert(short_id, entry);
    }

    pub(crate) fn recursion_remove(&mut self, id: &ProposalShortId) {
        let mut queue: VecDeque<ProposalShortId> = VecDeque::new();
        queue.push_back(id.clone());
        while let Some(id) = queue.pop_front() {
            if let Some(entry) = self.vertices.remove(&id) {
                for outpoint in entry.transaction.output_pts() {
                    if let Some(ids) = self.edges.remove(&outpoint) {
                        queue.extend(ids);
                    }
                }
            }
        }
    }

    pub(crate) fn remove_by_ancestor(&mut self, tx: &Transaction) -> Vec<PoolEntry> {
        let mut txs = Vec::new();
        let mut queue = VecDeque::new();

        self.remove_conflict(tx);

        queue.push_back(tx.output_pts());
        while let Some(outputs) = queue.pop_front() {
            for o in outputs {
                if let Some(ids) = self.edges.remove(&o) {
                    for cid in ids {
                        if let hash_map::Entry::Occupied(mut o) = self.vertices.entry(cid) {
                            let refs_count = {
                                let tx = o.get_mut();
                                tx.refs_count -= 1;
                                tx.refs_count
                            };

                            if refs_count == 0 {
                                let tx = o.remove();
                                queue.push_back(tx.transaction.output_pts());
                                txs.push(tx);
                            }
                        }
                    }
                }
            }
        }
        txs
    }

    pub(crate) fn remove_conflict(&mut self, tx: &Transaction) {
        let inputs = tx.input_pts();

        for input in inputs {
            if let Some(ids) = self.edges.remove(&input) {
                for cid in ids {
                    self.recursion_remove(&cid);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ckb_core::script::Script;
    use ckb_core::transaction::{CellInput, CellOutput, Transaction, TransactionBuilder};
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
                    .map(|i| CellOutput::new((i + 1) as u64, Vec::new(), Script::default(), None))
                    .collect(),
            )
            .build()
    }

    #[test]
    fn test_orphan_pool_remove_by_ancestor1() {
        let mut pool = OrphanPool::new();

        let tx1 = build_tx(vec![(H256::zero(), 0)], 1);
        let tx1_hash = tx1.hash().clone();

        let tx2 = build_tx(vec![(tx1_hash, 0)], 1);
        let tx2_hash = tx2.hash().clone();

        let tx3 = build_tx(vec![(tx2_hash, 0)], 1);
        let tx3_hash = tx3.hash().clone();

        let tx4 = build_tx(vec![(tx3_hash, 0)], 1);

        pool.add_tx(None, tx2.clone(), tx1.output_pts().into_iter());
        pool.add_tx(None, tx3.clone(), tx2.output_pts().into_iter());
        pool.add_tx(None, tx4.clone(), tx3.output_pts().into_iter());

        assert!(pool.contains(&tx2));
        assert!(pool.contains(&tx3));
        assert!(pool.contains(&tx4));

        let txs: Vec<_> = pool
            .remove_by_ancestor(&tx1)
            .into_iter()
            .map(|e| e.transaction)
            .collect();

        assert_eq!(txs, vec![tx2, tx3, tx4]);
    }

    #[test]
    fn test_orphan_pool_remove_by_ancestor2() {
        let mut pool = OrphanPool::new();

        let tx1 = build_tx(vec![(H256::zero(), 0)], 1);
        let tx1_hash = tx1.hash().clone();

        let tx2 = build_tx(vec![(H256::zero(), 1)], 1);
        let tx2_hash = tx2.hash().clone();

        let tx3 = build_tx(vec![(tx1_hash, 0), (tx2_hash, 1)], 1);
        let tx3_hash = tx3.hash().clone();

        let tx4 = build_tx(vec![(tx3_hash, 0)], 1);

        pool.add_tx(None, tx3.clone(), tx2.output_pts().into_iter());
        pool.add_tx(None, tx4.clone(), tx3.output_pts().into_iter());

        assert!(pool.contains(&tx3));

        let txs: Vec<_> = pool
            .remove_by_ancestor(&tx1)
            .into_iter()
            .map(|e| e.transaction)
            .collect();
        assert!(txs.is_empty());

        let txs: Vec<_> = pool
            .remove_by_ancestor(&tx2)
            .into_iter()
            .map(|e| e.transaction)
            .collect();
        assert_eq!(txs, vec![tx3, tx4]);
    }

    #[test]
    fn test_orphan_pool_recursion_remove() {
        let mut pool = OrphanPool::new();

        let tx1 = build_tx(vec![(H256::zero(), 0)], 1);
        let tx1_hash = tx1.hash().clone();

        let tx2 = build_tx(vec![(tx1_hash, 0)], 1);
        let tx2_hash = tx2.hash().clone();

        let tx3 = build_tx(vec![(tx2_hash, 0)], 1);
        let tx3_hash = tx3.hash().clone();

        let tx4 = build_tx(vec![(tx3_hash, 0)], 1);

        pool.add_tx(None, tx2.clone(), tx1.output_pts().into_iter());
        pool.add_tx(None, tx3.clone(), tx2.output_pts().into_iter());
        pool.add_tx(None, tx4.clone(), tx3.output_pts().into_iter());

        assert!(pool.contains(&tx2));
        assert!(pool.contains(&tx3));
        assert!(pool.contains(&tx4));

        let id = tx2.proposal_short_id();

        pool.recursion_remove(&id);

        assert!(!pool.contains(&tx2));
        assert!(!pool.contains(&tx3));
        assert!(!pool.contains(&tx4));
    }
}
