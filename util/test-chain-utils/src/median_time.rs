use ckb_traits::{BlockMedianTimeContext, HeaderProvider};
use ckb_types::{
    core::{BlockNumber, EpochNumberWithFraction, HeaderView, TransactionInfo},
    packed::Byte32,
    prelude::*,
};

/// TODO(doc): @chuijiaolianying
pub struct MockMedianTime {
    timestamps: Vec<u64>,
}

impl BlockMedianTimeContext for MockMedianTime {
    fn median_block_count(&self) -> u64 {
        11
    }

    fn timestamp_and_parent(&self, block_hash: &Byte32) -> (u64, BlockNumber, Byte32) {
        for i in 0..self.timestamps.len() {
            if &Self::get_block_hash(i as u64) == block_hash {
                if i == 0 {
                    return (self.timestamps[i], i as u64, Byte32::zero());
                } else {
                    return (
                        self.timestamps[i],
                        i as u64,
                        Self::get_block_hash(i as u64 - 1),
                    );
                }
            }
        }
        unreachable!()
    }
}

impl HeaderProvider for MockMedianTime {
    fn get_header(&self, _hash: &Byte32) -> Option<HeaderView> {
        None
    }
}

impl MockMedianTime {
    /// TODO(doc): @chuijiaolianying
    pub fn new(timestamps: Vec<u64>) -> Self {
        Self { timestamps }
    }

    /// TODO(doc): @chuijiaolianying
    pub fn get_block_hash(block_number: BlockNumber) -> Byte32 {
        let vec: Vec<u8> = (0..32).map(|_| block_number as u8).collect();
        Byte32::from_slice(vec.as_slice()).unwrap()
    }

    /// TODO(doc): @chuijiaolianying
    pub fn get_transaction_info(
        block_number: BlockNumber,
        block_epoch: EpochNumberWithFraction,
        index: usize,
    ) -> TransactionInfo {
        let block_hash = Self::get_block_hash(block_number);
        TransactionInfo {
            block_number,
            block_epoch,
            block_hash,
            index,
        }
    }
}
