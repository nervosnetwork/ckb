use ckb_types::core::{EpochExt, HeaderView};

/// Trait for epoch storage.
pub trait EpochProvider {
    /// Get corresponding `EpochExt` by block header
    fn get_epoch_ext(&self, block_header: &HeaderView) -> Option<EpochExt>;
    /// Get corresponding epoch progress information by block header
    fn get_block_epoch(&self, block_header: &HeaderView) -> Option<BlockEpoch>;
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
