use ckb_traits::HeaderProvider;
use ckb_types::{
    core::{BlockNumber, EpochNumberWithFraction, HeaderView, TransactionInfo},
    packed::Byte32,
    prelude::*,
};

/// There's a consensus rule to verify that the block timestamp must be larger than
/// the median timestamp of the previous 37 blocks.
///
/// `MockMedianTime` is a mock for the median time in testing.
/// And the number of previous blocks for calculating median timestamp is set as 11.
#[doc(hidden)]
pub struct MockMedianTime {
    timestamps: Vec<u64>,
}

#[doc(hidden)]
pub const MOCK_MEDIAN_TIME_COUNT: usize = 11;

impl HeaderProvider for MockMedianTime {
    fn get_header(&self, _hash: &Byte32) -> Option<HeaderView> {
        None
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

impl MockMedianTime {
    /// Create a new `MockMedianTime`.
    #[doc(hidden)]
    pub fn new(timestamps: Vec<u64>) -> Self {
        Self { timestamps }
    }

    /// Return the block hash from block height number.
    #[doc(hidden)]
    pub fn get_block_hash(block_number: BlockNumber) -> Byte32 {
        let vec: Vec<u8> = (0..32).map(|_| block_number as u8).collect();
        Byte32::from_slice(vec.as_slice()).unwrap()
    }

    /// Return transaction info corresponding to the block number, block epoch and transaction index.
    #[doc(hidden)]
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
