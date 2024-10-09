use ckb_shared::Snapshot;
use ckb_store::ChainStore;
use ckb_types::{
    core::{BlockExt, BlockNumber, BlockView, HeaderView},
    packed,
    prelude::*,
    utilities::compact_to_difficulty,
    U256,
};

pub(crate) trait SnapshotExt {
    fn get_header_by_number(&self, num: BlockNumber) -> Option<HeaderView>;

    fn get_block_by_number(&self, num: BlockNumber) -> Option<BlockView>;

    fn get_block_ext_by_number(&self, num: BlockNumber) -> Option<BlockExt>;

    fn get_verifiable_header_by_number(&self, num: BlockNumber)
        -> Option<packed::VerifiableHeader>;

    fn get_block_difficulty_by_number(&self, num: BlockNumber) -> Option<U256> {
        self.get_header_by_number(num)
            .map(|header| compact_to_difficulty(header.compact_target()))
    }

    fn get_total_difficulty_by_number(&self, num: BlockNumber) -> Option<U256> {
        self.get_block_ext_by_number(num)
            .map(|block_ext| block_ext.total_difficulty)
    }

    fn build_last_state_by_number(&self, num: BlockNumber) -> Option<packed::LightClientMessage> {
        self.get_verifiable_header_by_number(num).map(|header| {
            let content = packed::SendLastState::new_builder()
                .last_header(header)
                .build();
            packed::LightClientMessage::new_builder()
                .set(content)
                .build()
        })
    }
}

impl SnapshotExt for Snapshot {
    fn get_header_by_number(&self, num: BlockNumber) -> Option<HeaderView> {
        self.get_block_hash(num)
            .and_then(|hash| self.get_block_header(&hash))
    }

    fn get_block_by_number(&self, num: BlockNumber) -> Option<BlockView> {
        self.get_block_hash(num)
            .and_then(|hash| self.get_block(&hash))
    }

    fn get_block_ext_by_number(&self, num: BlockNumber) -> Option<BlockExt> {
        self.get_block_hash(num)
            .and_then(|hash| self.get_block_ext(&hash))
    }

    fn get_verifiable_header_by_number(
        &self,
        num: BlockNumber,
    ) -> Option<packed::VerifiableHeader> {
        self.get_block_by_number(num).map(|block| {
            let mmr = self.chain_root_mmr(num - 1);
            let parent_chain_root = mmr.get_root().expect("has chain root");
            packed::VerifiableHeader::new_builder()
                .header(block.data().header())
                .uncles_hash(block.calc_uncles_hash())
                .extension(Pack::pack(&block.extension()))
                .parent_chain_root(parent_chain_root)
                .build()
        })
    }
}
