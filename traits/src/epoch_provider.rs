use ckb_types::{
    core::{BlockExt, BlockNumber, EpochExt, HeaderView},
    packed,
};

/// Trait for epoch storage.
pub trait EpochProvider {
    /// Get corresponding `EpochExt` by block header
    fn get_epoch_ext(&self, block_header: &HeaderView) -> Option<EpochExt>;
    /// Get block header hash by block number
    fn get_block_hash(&self, number: BlockNumber) -> Option<packed::Byte32>;
    /// Get block ext by block header hash
    fn get_block_ext(&self, block_hash: &packed::Byte32) -> Option<BlockExt>;
    /// Get header by block header hash
    fn get_block_header(&self, hash: &packed::Byte32) -> Option<HeaderView>;

    /// Get corresponding epoch progress information by block header
    fn get_block_epoch(&self, header: &HeaderView) -> Option<BlockEpoch> {
        self.get_epoch_ext(header).map(|epoch| {
            if header.number() != epoch.start_number() + epoch.length() - 1 {
                BlockEpoch::NonTailBlock { epoch }
            } else {
                let last_block_hash_in_previous_epoch = if epoch.is_genesis() {
                    self.get_block_hash(0).expect("genesis block stored")
                } else {
                    epoch.last_block_hash_in_previous_epoch()
                };
                let epoch_uncles_count = self
                    .get_block_ext(&header.hash())
                    .expect("stored block ext")
                    .total_uncles_count
                    - self
                        .get_block_ext(&last_block_hash_in_previous_epoch)
                        .expect("stored block ext")
                        .total_uncles_count;
                let epoch_duration_in_milliseconds = header.timestamp()
                    - self
                        .get_block_header(&last_block_hash_in_previous_epoch)
                        .expect("stored block header")
                        .timestamp();

                BlockEpoch::TailBlock {
                    epoch,
                    epoch_uncles_count,
                    epoch_duration_in_milliseconds,
                }
            }
        })
    }
}

/// Progress of block's corresponding epoch
pub enum BlockEpoch {
    /// Block is the tail block of epoch, provides extrat statistics for next epoch generating or verifying
    TailBlock {
        /// epoch information
        epoch: EpochExt,
        /// epoch uncles count
        epoch_uncles_count: u64,
        /// epoch duration
        epoch_duration_in_milliseconds: u64,
    },
    /// Non tail block of epoch
    NonTailBlock {
        /// epoch information
        epoch: EpochExt,
    },
}

impl BlockEpoch {
    /// Return block's corresponding epoch information
    pub fn epoch(self) -> EpochExt {
        match self {
            Self::TailBlock {
                epoch,
                epoch_uncles_count: _,
                epoch_duration_in_milliseconds: _,
            } => epoch,
            Self::NonTailBlock { epoch } => epoch,
        }
    }
}
