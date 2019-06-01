#![allow(dead_code)]

use crate::tx_pool::types::PendingEntry;
use ckb_core::cell::{CellMetaBuilder, CellProvider, CellStatus};
use ckb_core::transaction::{OutPoint, ProposalShortId, Transaction};
use ckb_core::Cycle;
use ckb_util::{LinkedFnvHashMap, LinkedFnvHashMapEntries};

#[derive(Default, Debug, Clone)]
pub(crate) struct PendingQueue {
    pub(crate) inner: LinkedFnvHashMap<ProposalShortId, PendingEntry>,
}

impl PendingQueue {
    pub fn new() -> Self {
        PendingQueue {
            inner: LinkedFnvHashMap::default(),
        }
    }

    pub fn size(&self) -> usize {
        self.inner.len()
    }

    pub(crate) fn add_tx(
        &mut self,
        cycles: Option<Cycle>,
        size: usize,
        tx: Transaction,
    ) -> Option<PendingEntry> {
        let short_id = tx.proposal_short_id();
        self.inner
            .insert(short_id, PendingEntry::new(tx, cycles, size))
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

    pub(crate) fn remove(&mut self, id: &ProposalShortId) -> Option<PendingEntry> {
        self.inner.remove(id)
    }

    pub(crate) fn fetch(&self, n: usize) -> Vec<ProposalShortId> {
        self.inner.keys().take(n).cloned().collect()
    }

    pub(crate) fn keys(&self) -> impl Iterator<Item = &ProposalShortId> {
        self.inner.keys()
    }

    pub(crate) fn entries(&mut self) -> LinkedFnvHashMapEntries<ProposalShortId, PendingEntry> {
        self.inner.entries()
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
                    Some(output) => CellStatus::live_cell(
                        CellMetaBuilder::from_cell_output(output.to_owned())
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
