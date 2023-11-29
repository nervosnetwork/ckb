use ckb_types::core::hardfork::HardForks;
use ckb_types::core::{BlockExt, BlockView};
use ckb_types::packed::ProposalShortId;
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
        ckb_rust_unstable_port::IsSorted::is_sorted_by_key(
            &mut self.attached_blocks().iter(),
            |blk| blk.header().number(),
        ) && ckb_rust_unstable_port::IsSorted::is_sorted_by_key(
            &mut self.detached_blocks().iter(),
            |blk| blk.header().number(),
        )
    }

    pub fn during_hardfork(&self, hardfork_switch: &HardForks) -> bool {
        let hardfork_during_detach =
            self.check_if_hardfork_during_blocks(hardfork_switch, &self.detached_blocks);
        let hardfork_during_attach =
            self.check_if_hardfork_during_blocks(hardfork_switch, &self.attached_blocks);

        hardfork_during_detach || hardfork_during_attach
    }

    fn check_if_hardfork_during_blocks(
        &self,
        hardfork: &HardForks,
        blocks: &VecDeque<BlockView>,
    ) -> bool {
        if blocks.is_empty() {
            false
        } else {
            // This method assumes that the input blocks are sorted and unique.
            let rfc_0049 = hardfork.ckb2023.rfc_0049();
            let epoch_first = blocks.front().unwrap().epoch().number();
            let epoch_next = blocks
                .back()
                .unwrap()
                .epoch()
                .minimum_epoch_number_after_n_blocks(1);
            epoch_first < rfc_0049 && rfc_0049 <= epoch_next
        }
    }
}
