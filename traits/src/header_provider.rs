use ckb_types::{
    core::{BlockNumber, EpochNumberWithFraction, HeaderView},
    packed::Byte32,
};

/// Trait for header storage
pub trait HeaderProvider {
    /// Get the header of the given block hash
    fn get_header(&self, hash: &Byte32) -> Option<HeaderView>;
}

/// A compact representation of header fields, used for header verification and median time calculation
pub struct HeaderFields {
    /// Block hash
    pub hash: Byte32,
    /// Block number
    pub number: BlockNumber,
    /// Block epoch
    pub epoch: EpochNumberWithFraction,
    /// Block timestamp
    pub timestamp: u64,
    /// Block parent hash
    pub parent_hash: Byte32,
}

/// Trait for header fields storage
pub trait HeaderFieldsProvider {
    /// Get the header fields of the given block hash
    fn get_header_fields(&self, hash: &Byte32) -> Option<HeaderFields>;

    /// Get past block median time, **including the timestamp of the given one**
    fn block_median_time(&self, block_hash: &Byte32, median_block_count: usize) -> u64 {
        let mut timestamps: Vec<u64> = Vec::with_capacity(median_block_count);
        let mut block_hash = block_hash.clone();
        for _ in 0..median_block_count {
            let header_fields = self
                .get_header_fields(&block_hash)
                .expect("parent header exist");
            timestamps.push(header_fields.timestamp);
            block_hash = header_fields.parent_hash;

            if header_fields.number == 0 {
                break;
            }
        }

        // return greater one if count is even.
        timestamps.sort_unstable();
        timestamps[timestamps.len() >> 1]
    }
}
