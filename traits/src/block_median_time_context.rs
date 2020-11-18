use crate::HeaderProvider;
use ckb_types::{core::BlockNumber, packed::Byte32};

/// The invoker should only rely on `block_median_time` function
/// the other functions only use to help the default `block_median_time`, and maybe unimplemented.
pub trait BlockMedianTimeContext: HeaderProvider {
    /// TODO(doc): @quake
    fn median_block_count(&self) -> u64;

    /// Return timestamp and block_number of the corresponding block_hash, and hash of parent block
    fn timestamp_and_parent(&self, block_hash: &Byte32) -> (u64, BlockNumber, Byte32) {
        let header = self
            .get_header(block_hash)
            .expect("[BlockMedianTimeContext] blocks used for median time exist");
        (
            header.timestamp(),
            header.number(),
            header.data().raw().parent_hash(),
        )
    }

    /// Return past block median time, **including the timestamp of the given one**
    fn block_median_time(&self, block_hash: &Byte32) -> u64 {
        let median_time_span = self.median_block_count();
        let mut timestamps: Vec<u64> = Vec::with_capacity(median_time_span as usize);
        let mut block_hash = block_hash.clone();
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
