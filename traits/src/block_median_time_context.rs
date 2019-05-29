use ckb_core::BlockNumber;
use numext_fixed_hash::H256;
use std::cmp::min;

/// The invoker should only rely on `block_median_time` function
/// the other functions only use to help the default `block_median_time`, and maybe unimplemented.
pub trait BlockMedianTimeContext {
    fn median_block_count(&self) -> u64;

    /// Return timestamp of the correspoding bloch_hash, and hash of parent block
    ///
    /// Fake implementation:
    /// ```ignore
    /// let current_header = get_block_header(block_hash);
    /// let parent_header = current_header.timestamp_and_parent().header();
    /// return (parent_header.timestamp(), parent_header.hash());
    /// ```
    fn timestamp_and_parent(&self, block_hash: &H256) -> (u64, H256);

    /// Return past block median time, **including the timestamp of the given one**
    ///
    /// It's duty for outside caller to ensure that the block_number and block_hash are matched
    fn block_median_time(&self, block_number: BlockNumber, block_hash: &H256) -> u64 {
        let median_time_span = min(block_number + 1, self.median_block_count());
        let mut timestamps: Vec<u64> = Vec::with_capacity(median_time_span as usize);
        let mut block_hash = block_hash.to_owned();
        for _ in 0..median_time_span {
            let (timestamp, parent_hash) = self.timestamp_and_parent(&block_hash);
            timestamps.push(timestamp);
            block_hash = parent_hash;
        }

        // return greater one if count is even.
        timestamps.sort();
        timestamps[timestamps.len() / 2]
    }

    /// Return the corresponding block_hash
    ///
    /// It's just a convenience way that constructing a BlockMedianContext, to get the
    /// corresponding block_hash when you only know a block_number.
    ///
    /// Often used in verifying "since by block number".
    fn get_block_hash(&self, block_number: BlockNumber) -> Option<H256>;
}
