//! Block assembler state: cached template size and the current template snapshot.

use crate::block_assembler::builder::BlockTemplate;
use ckb_snapshot::Snapshot;
use ckb_types::core::EpochExt;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) struct TemplateSize {
    pub(crate) txs: usize,
    pub(crate) proposals: usize,
    pub(crate) uncles: usize,
    pub(crate) total: usize,
}

impl TemplateSize {
    pub(crate) fn calc_total_by_uncles_and_proposals(
        &self,
        new_uncles_size: usize,
        new_proposals_size: usize,
    ) -> Option<usize> {
        self.total
            .checked_sub(self.uncles)?
            .checked_sub(self.proposals)?
            .checked_add(new_uncles_size)?
            .checked_add(new_proposals_size)
    }

    pub(crate) fn calc_total_by_txs(&self, new_txs_size: usize) -> Option<usize> {
        self.total.checked_sub(self.txs)?.checked_add(new_txs_size)
    }
}

#[derive(Clone)]
pub(crate) struct CurrentTemplate {
    pub(crate) template: BlockTemplate,
    pub(crate) size: TemplateSize,
    pub(crate) snapshot: Arc<Snapshot>,
    pub(crate) epoch: EpochExt,
}
