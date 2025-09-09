use ckb_types::core::{BlockExt, BlockView};
use ckb_types::packed::ProposalShortId;
#[cfg(debug_assertions)]
use is_sorted::IsSorted;
use std::collections::{HashSet, VecDeque};

/// The struct represent fork
#[derive(Debug, Default)]
pub struct ForkChanges {
    /// Blocks attached to index after forks
    pub(crate) attached_blocks: VecDeque<BlockView>,
    /// Blocks detached from index after forks
    pub(crate) detached_blocks: VecDeque<BlockView>,
    /// HashSet with proposal_id detached to index after forks
    pub(crate) detached_proposal_id: HashSet<ProposalShortId>,
    /// to be updated exts
    pub(crate) dirty_exts: VecDeque<BlockExt>,
}

impl ForkChanges {
    /// blocks attached to index after forks
    pub fn attached_blocks(&self) -> &VecDeque<BlockView> {
        &self.attached_blocks
    }

    /// blocks detached from index after forks
    pub fn detached_blocks(&self) -> &VecDeque<BlockView> {
        &self.detached_blocks
    }

    /// proposal_id detached to index after forks
    pub fn detached_proposal_id(&self) -> &HashSet<ProposalShortId> {
        &self.detached_proposal_id
    }

    /// are there any block should be detached
    pub fn has_detached(&self) -> bool {
        !self.detached_blocks.is_empty()
    }

    /// cached verified attached block num
    pub fn verified_len(&self) -> usize {
        self.attached_blocks.len() - self.dirty_exts.len()
    }

    /// assertion for make sure attached_blocks and detached_blocks are sorted
    #[cfg(debug_assertions)]
    pub fn is_sorted(&self) -> bool {
        IsSorted::is_sorted_by_key(&mut self.attached_blocks().iter(), |blk| {
            blk.header().number()
        }) && IsSorted::is_sorted_by_key(&mut self.detached_blocks().iter(), |blk| {
            blk.header().number()
        })
    }
}
