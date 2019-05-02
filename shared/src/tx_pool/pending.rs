#![allow(dead_code)]

use crate::tx_pool::types::PoolEntry;
use ckb_core::cell::{CellMeta, CellProvider, CellStatus};
use ckb_core::transaction::{OutPoint, ProposalShortId, Transaction};
use ckb_core::Cycle;
use fnv::FnvHashMap;

#[derive(Default, Debug, Clone)]
pub(crate) struct PendingQueue {
    pub(crate) inner: FnvHashMap<ProposalShortId, PoolEntry>,
}

impl PendingQueue {
    pub fn new() -> Self {
        PendingQueue {
            inner: FnvHashMap::default(),
        }
    }

    pub fn size(&self) -> usize {
        self.inner.len()
    }

    pub(crate) fn add_tx(&mut self, cycles: Option<Cycle>, tx: Transaction) -> Option<PoolEntry> {
        let short_id = tx.proposal_short_id();
        self.inner.insert(short_id, PoolEntry::new(tx, 0, cycles))
    }

    pub(crate) fn contains_key(&self, id: &ProposalShortId) -> bool {
        self.inner.contains_key(id)
    }

    pub(crate) fn get(&self, id: &ProposalShortId) -> Option<&PoolEntry> {
        self.inner.get(id)
    }

    pub(crate) fn get_tx(&self, id: &ProposalShortId) -> Option<&Transaction> {
        self.get(id).map(|x| &x.transaction)
    }

    pub(crate) fn remove(&mut self, id: &ProposalShortId) -> Option<PoolEntry> {
        self.inner.remove(id)
    }

    pub(crate) fn fetch(&self, n: usize) -> Vec<ProposalShortId> {
        self.inner.keys().take(n).cloned().collect()
    }
}

impl CellProvider for PendingQueue {
    fn cell(&self, o: &OutPoint) -> CellStatus {
        if let Some(cell_out_point) = &o.cell {
            if let Some(x) = self
                .inner
                .get(&ProposalShortId::from_tx_hash(&cell_out_point.tx_hash))
            {
                match x.transaction.get_output(cell_out_point.index as usize) {
                    Some(output) => CellStatus::live_cell(CellMeta {
                        cell_output: Some(output.clone()),
                        cellbase: false,
                        block_number: None,
                        out_point: cell_out_point.to_owned(),
                        capacity: output.capacity,
                        data_hash: None,
                    }),
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
