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
    ) -> usize {
        let total = self.calc_total_by_uncles(new_uncles_size);
        if new_proposals_size > self.proposals {
            total.saturating_add(new_proposals_size - self.proposals)
        } else {
            total.saturating_sub(self.proposals - new_proposals_size)
        }
    }

    pub(crate) fn calc_total_by_uncles(&self, new_uncles_size: usize) -> usize {
        if new_uncles_size > self.uncles {
            self.total.saturating_add(new_uncles_size - self.uncles)
        } else {
            self.total.saturating_sub(self.uncles - new_uncles_size)
        }
    }

    pub(crate) fn calc_total_by_txs(&self, new_txs_size: usize) -> usize {
        if new_txs_size > self.txs {
            self.total.saturating_add(new_txs_size - self.txs)
        } else {
            self.total.saturating_sub(self.txs - new_txs_size)
        }
    }
}

#[derive(Clone)]
pub(crate) struct CurrentTemplate {
    pub(crate) template: BlockTemplate,
    pub(crate) size: TemplateSize,
    pub(crate) snapshot: Arc<Snapshot>,
    pub(crate) epoch: EpochExt,
}
