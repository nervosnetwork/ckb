use crate::component::entry::DefectEntry;
use ckb_metrics::metrics;
use ckb_types::{
    core::TransactionView,
    packed::{OutPoint, ProposalShortId},
};
use ckb_verification::cache::CacheEntry;
use std::collections::VecDeque;
use std::collections::{hash_map, HashMap};
use std::iter::ExactSizeIterator;

pub(crate) const TTL: u64 = 4 * 60 * 60;
pub(crate) const PRUNE_THRESHOLD: usize = 1500;

///not verified, may contain conflict transactions
#[derive(Default, Debug, Clone)]
pub(crate) struct OrphanPool {
    pub(crate) vertices: HashMap<ProposalShortId, DefectEntry>,
    pub(crate) edges: HashMap<OutPoint, Vec<ProposalShortId>>,
    pub(crate) prune_threshold: usize,
}

impl OrphanPool {
    pub(crate) fn new() -> Self {
        let this = OrphanPool::raw_new(PRUNE_THRESHOLD);
        this.record_metrics();
        this
    }

    pub(crate) fn raw_new(prune_threshold: usize) -> Self {
        OrphanPool {
            prune_threshold,
            ..Default::default()
        }
    }

    pub(crate) fn size(&self) -> usize {
        self.vertices.len()
    }

    pub(crate) fn get(&self, id: &ProposalShortId) -> Option<&DefectEntry> {
        self.vertices.get(id)
    }

    pub(crate) fn get_tx(&self, id: &ProposalShortId) -> Option<&TransactionView> {
        self.get(id).map(|x| &x.transaction)
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, tx: &TransactionView) -> bool {
        self.vertices.contains_key(&tx.proposal_short_id())
    }

    pub(crate) fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.vertices.contains_key(id)
    }

    /// add orphan transaction
    pub(crate) fn add_tx(
        &mut self,
        cache_entry: Option<CacheEntry>,
        size: usize,
        tx: TransactionView,
        unknown: impl ExactSizeIterator<Item = OutPoint>,
    ) -> Option<DefectEntry> {
        self.prune();
        let short_id = tx.proposal_short_id();
        let entry = DefectEntry::new(tx, unknown.len(), cache_entry, size);
        for out_point in unknown {
            let edge = self.edges.entry(out_point).or_insert_with(Vec::new);
            edge.push(short_id.clone());
        }
        let ret = self.vertices.insert(short_id, entry);
        self.record_metrics();
        ret
    }

    fn prune(&mut self) {
        if self.size() < self.prune_threshold {
            return;
        }

        let now = faketime::unix_time().as_secs();
        let ids: Vec<ProposalShortId> = self
            .vertices
            .iter()
            .filter_map(|(id, entry)| {
                if entry.timestamp + TTL < now {
                    Some(id)
                } else {
                    None
                }
            })
            .cloned()
            .collect();

        for id in ids {
            self.recursion_remove(&id);
        }
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
        self.record_metrics();
    }

    pub(crate) fn remove_by_ancestor(&mut self, tx: &TransactionView) -> Vec<DefectEntry> {
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
        self.record_metrics();
        txs
    }

    pub(crate) fn remove_conflict(&mut self, tx: &TransactionView) {
        let inputs = tx.input_pts_iter();

        for input in inputs {
            if let Some(ids) = self.edges.remove(&input) {
                for cid in ids {
                    self.recursion_remove(&cid);
                }
            }
        }
    }

    fn record_metrics(&self) {
        metrics!(gauge, "ckb.pool_size", self.vertices.len() as i64, "name" => "orphan");
    }
}

#[cfg(test)]
mod tests {
    use super::{OrphanPool, TTL};
    use ckb_types::{
        bytes::Bytes,
        core::{Capacity, TransactionBuilder, TransactionView},
        packed::{Byte32, CellInput, CellOutput, OutPoint},
        prelude::*,
    };

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

    const MOCK_SIZE: usize = 0;

    #[test]
    fn test_orphan_pool_remove_by_ancestor1() {
        let mut pool = OrphanPool::new();

        let tx1 = build_tx(vec![(&Byte32::zero(), 0)], 1);
        let tx1_hash = tx1.hash();

        let tx2 = build_tx(vec![(&tx1_hash, 0)], 1);
        let tx2_hash = tx2.hash();

        let tx3 = build_tx(vec![(&tx2_hash, 0)], 1);
        let tx3_hash = tx3.hash();

        let tx4 = build_tx(vec![(&tx3_hash, 0)], 1);

        // the tx5 and its descendants(tx6) conflict with tx1
        let tx5 = build_tx(vec![(&Byte32::zero(), 0)], 2);
        let tx5_hash = tx5.hash();

        let tx6 = build_tx(vec![(&tx5_hash, 0)], 1);

        pool.add_tx(None, MOCK_SIZE, tx2.clone(), tx1.output_pts().into_iter());
        pool.add_tx(None, MOCK_SIZE, tx3.clone(), tx2.output_pts().into_iter());
        pool.add_tx(None, MOCK_SIZE, tx4.clone(), tx3.output_pts().into_iter());
        pool.add_tx(
            None,
            MOCK_SIZE,
            tx5.clone(),
            tx1.inputs().into_iter().map(|x| x.previous_output()),
        );
        pool.add_tx(None, MOCK_SIZE, tx6.clone(), tx5.output_pts().into_iter());

        assert!(pool.contains(&tx2));
        assert!(pool.contains(&tx3));
        assert!(pool.contains(&tx4));
        assert!(pool.contains(&tx5));
        assert!(pool.contains(&tx6));

        let txs: Vec<_> = pool
            .remove_by_ancestor(&tx1)
            .into_iter()
            .map(|e| e.transaction)
            .collect();

        assert!(!pool.contains(&tx5));
        assert!(!pool.contains(&tx6));

        assert_eq!(txs, vec![tx2, tx3, tx4]);
    }

    #[test]
    fn test_orphan_pool_remove_by_ancestor2() {
        let mut pool = OrphanPool::new();

        let tx1 = build_tx(vec![(&Byte32::zero(), 0)], 1);
        let tx1_hash = tx1.hash();

        let tx2 = build_tx(vec![(&Byte32::zero(), 1)], 1);
        let tx2_hash = tx2.hash();

        let tx3 = build_tx(vec![(&tx1_hash, 0), (&tx2_hash, 1)], 1);
        let tx3_hash = tx3.hash();

        let tx4 = build_tx(vec![(&tx3_hash, 0)], 1);

        pool.add_tx(None, MOCK_SIZE, tx3.clone(), tx2.output_pts().into_iter());
        pool.add_tx(None, MOCK_SIZE, tx4.clone(), tx3.output_pts().into_iter());

        assert!(pool.contains(&tx3));

        let txs: Vec<_> = pool
            .remove_by_ancestor(&tx1)
            .into_iter()
            .map(|e| e.transaction)
            .collect();
        assert!(txs.is_empty());

        assert_eq!(txs, vec![]);
        assert!(pool.contains(&tx3));
        assert!(pool.contains(&tx4));

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

        let tx1 = build_tx(vec![(&Byte32::zero(), 0)], 1);
        let tx1_hash = tx1.hash();

        let tx2 = build_tx(vec![(&tx1_hash, 0)], 1);
        let tx2_hash = tx2.hash();

        let tx3 = build_tx(vec![(&tx2_hash, 0)], 1);
        let tx3_hash = tx3.hash();

        let tx4 = build_tx(vec![(&tx3_hash, 0)], 1);

        pool.add_tx(None, MOCK_SIZE, tx2.clone(), tx1.output_pts().into_iter());
        pool.add_tx(None, MOCK_SIZE, tx3.clone(), tx2.output_pts().into_iter());
        pool.add_tx(None, MOCK_SIZE, tx4.clone(), tx3.output_pts().into_iter());

        assert!(pool.contains(&tx2));
        assert!(pool.contains(&tx3));
        assert!(pool.contains(&tx4));

        let id = tx2.proposal_short_id();

        pool.recursion_remove(&id);

        assert!(!pool.contains(&tx2));
        assert!(!pool.contains(&tx3));
        assert!(!pool.contains(&tx4));
    }

    #[test]
    fn test_orphan_prune() {
        let faketime_file = faketime::millis_tempfile(0).expect("create faketime file");
        faketime::enable(&faketime_file);

        let mut pool = OrphanPool::raw_new(0);

        let tx1 = build_tx(vec![(&Byte32::zero(), 0)], 1);
        let tx1_hash = tx1.hash();

        let tx2 = build_tx(vec![(&tx1_hash, 0)], 1);
        let tx2_hash = tx2.hash();

        let tx3 = build_tx(vec![(&tx2_hash, 0)], 1);

        pool.add_tx(None, MOCK_SIZE, tx2.clone(), tx1.output_pts().into_iter());
        faketime::write_millis(&faketime_file, (TTL + 1) * 1000).expect("write millis");
        pool.add_tx(None, MOCK_SIZE, tx3.clone(), tx2.output_pts().into_iter());
        assert!(!pool.contains(&tx2));
        assert!(pool.contains(&tx3));
    }
}
