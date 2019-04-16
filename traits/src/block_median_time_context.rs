use ckb_core::BlockNumber;

/// The invoker should only rely on `block_median_time` function
/// the other functions only use to help the default `block_median_time`, and maybe unimplemented.
pub trait BlockMedianTimeContext {
    fn median_block_count(&self) -> u64;
    /// block timestamp
    fn timestamp(&self, block_number: BlockNumber) -> Option<u64>;
    /// ancestor timestamps from a block
    fn ancestor_timestamps(&self, block_number: BlockNumber) -> Vec<u64> {
        let count = self.median_block_count();
        (block_number.saturating_sub(count)..=block_number)
            .filter_map(|n| self.timestamp(n))
            .collect()
    }

    /// get block median time
    fn block_median_time(&self, block_number: BlockNumber) -> Option<u64> {
        let mut timestamps: Vec<u64> = self.ancestor_timestamps(block_number);
        timestamps.sort_by(|a, b| a.cmp(b));
        // return greater one if count is even.
        timestamps.get(timestamps.len() / 2).cloned()
    }
}
