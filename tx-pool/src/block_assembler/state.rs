//! Block assembler state: cached template size and the current template snapshot.

use crate::block_assembler::builder::BlockTemplate;
use ckb_snapshot::Snapshot;
use ckb_types::core::EpochExt;
use std::sync::Arc;

/// Version of the published template content. Partial updates compare this
/// token before applying; full updates deliberately ignore it and overwrite a
/// racing partial update.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct TemplateRevision(u64);

impl TemplateRevision {
    pub(crate) const INITIAL: Self = Self(0);

    pub(crate) fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

/// Generation of the latest published authoritative reset. A full rebuild may
/// overwrite partial content, but it may not cross this boundary.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(crate) struct ResetEpoch(u64);

impl ResetEpoch {
    pub(crate) const INITIAL: Self = Self(0);

    pub(crate) fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

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
    /// Publication metadata is co-located with the state it versions. This
    /// avoids an atomic/template split where a reader could pair a template
    /// with a counter from another publication.
    pub(crate) revision: TemplateRevision,
    pub(crate) reset_epoch: ResetEpoch,
}
