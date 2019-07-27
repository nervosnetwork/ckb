use ckb_core::BlockNumber;
use numext_fixed_hash::H256;

/// The invoker should only rely on `block_median_time` function
/// the other functions only use to help the default `block_median_time`, and maybe unimplemented.
pub trait BlockMedianTimeContext {
    fn median_block_count(&self) -> u64;

    /// Return timestamp and block_number of the corresponding bloch_hash, and hash of parent block
    ///
    /// Fake implementation:
    /// ```ignore
    /// let current_header = get_block_header(block_hash);
    /// let parent_header = current_header.timestamp_and_parent().header();
    /// return (parent_header.timestamp(), current_header.number(), parent_header.hash());
    /// ```
    fn timestamp_and_parent(&self, block_hash: &H256) -> (u64, BlockNumber, H256);

    /// Return past block median time, **including the timestamp of the given one**
    fn block_median_time(&self, block_hash: &H256) -> u64 {
        let median_time_span = self.median_block_count();
        let mut timestamps: Vec<u64> = Vec::with_capacity(median_time_span as usize);
        let mut block_hash = block_hash.to_owned();
        for _ in 0..median_time_span {
            let (timestamp, block_number, parent_hash) = self.timestamp_and_parent(&block_hash);
            timestamps.push(timestamp);
            block_hash = parent_hash;

            if block_number == 0 {
                break;
            }
        }

        // return greater one if count is even.
        timestamps.sort();
        timestamps[timestamps.len() >> 1]
    }
}
