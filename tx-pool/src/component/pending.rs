use crate::component::entry::TxEntry;
use ckb_types::{
    core::{
        cell::{CellChecker, CellMetaBuilder, CellProvider, CellStatus},
        TransactionView,
    },
    packed::{OutPoint, ProposalShortId},
    prelude::*,
};
use ckb_util::{LinkedHashMap, LinkedHashMapEntries};
use std::collections::HashSet;

#[derive(Debug, Clone)]
pub(crate) struct PendingQueue {
    inner: LinkedHashMap<ProposalShortId, TxEntry>,
}

impl PendingQueue {
    pub(crate) fn new() -> Self {
        PendingQueue {
            inner: Default::default(),
        }
    }

    pub(crate) fn size(&self) -> usize {
        self.inner.len()
    }

    pub(crate) fn add_entry(&mut self, entry: TxEntry) -> Option<TxEntry> {
        self.inner.insert(entry.proposal_short_id(), entry)
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
        self.inner.get(id).map(|entry| entry.transaction())
    }

    pub(crate) fn remove_entry(&mut self, id: &ProposalShortId) -> Option<TxEntry> {
        self.inner.remove(id)
    }

    pub fn entries(&mut self) -> LinkedHashMapEntries<ProposalShortId, TxEntry> {
        self.inner.entries()
    }

    // fill proposal txs
    pub fn fill_proposals(
        &self,
        limit: usize,
        exclusion: &HashSet<ProposalShortId>,
        proposals: &mut HashSet<ProposalShortId>,
    ) {
        for id in self.inner.keys() {
            if proposals.len() == limit {
                break;
            }
            if !exclusion.contains(&id) {
                proposals.insert(id.clone());
            }
        }
    }
}

impl CellProvider for PendingQueue {
    fn cell(&self, out_point: &OutPoint, with_data: bool, allow_in_txpool: bool) -> CellStatus {
        let tx_hash = out_point.tx_hash();
        if let Some(entry) = self.inner.get(&ProposalShortId::from_tx_hash(&tx_hash)) {
            match entry
                .transaction()
                .output_with_data(out_point.index().unpack())
            {
                Some((output, data)) => {
                    let mut cell_meta = CellMetaBuilder::from_cell_output(output, data)
                        .out_point(out_point.to_owned())
                        .build();
                    if !allow_in_txpool && !with_data {
                        cell_meta.mem_cell_data_hash = None;
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

impl CellChecker for PendingQueue {
    fn is_live(&self, out_point: &OutPoint) -> Option<bool> {
        let tx_hash = out_point.tx_hash();
        if let Some(entry) = self.inner.get(&ProposalShortId::from_tx_hash(&tx_hash)) {
            entry
                .transaction()
                .output(out_point.index().unpack())
                .map(|_| true)
        } else {
            None
        }
    }
}
