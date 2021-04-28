use ckb_types::{
    core::{BlockNumber, HeaderView},
    packed::Byte32,
};

/// TODO(doc): @quake
pub trait HeaderProvider {
    /// TODO(doc): @quake
    fn get_header(&self, hash: &Byte32) -> Option<HeaderView>;

    /// Return timestamp and block_number of the corresponding block_hash, and hash of parent block
    fn timestamp_and_parent(&self, block_hash: &Byte32) -> (u64, BlockNumber, Byte32) {
        let header = self.get_header(block_hash).expect("parent header exist");
        (
            header.timestamp(),
            header.number(),
            header.data().raw().parent_hash(),
        )
    }

    /// Return past block median time, **including the timestamp of the given one**
    fn block_median_time(&self, block_hash: &Byte32, median_block_count: usize) -> u64 {
        let mut timestamps: Vec<u64> = Vec::with_capacity(median_block_count);
        let mut block_hash = block_hash.clone();
        for _ in 0..median_block_count {
            let (timestamp, block_number, parent_hash) = self.timestamp_and_parent(&block_hash);
            timestamps.push(timestamp);
            block_hash = parent_hash;

            if block_number == 0 {
                break;
            }
        }

        // return greater one if count is even.
        timestamps.sort_unstable();
        timestamps[timestamps.len() >> 1]
    }
}

impl HeaderProvider for Box<dyn Fn(Byte32) -> Option<HeaderView>> {
    fn get_header(&self, hash: &Byte32) -> Option<HeaderView> {
        (self)(hash.to_owned())
    }
}
