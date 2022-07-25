use ckb_traits::HeaderProvider;
use ckb_types::{
    core::{BlockNumber, EpochNumberWithFraction, HeaderBuilder, HeaderView, TransactionInfo},
    packed::Byte32,
    prelude::*,
};
use ckb_util::LinkedHashMap;

/// There's a consensus rule to verify that the block timestamp must be larger than
/// the median timestamp of the previous 37 blocks.
///
/// `MockMedianTime` is a mock for the median time in testing.
/// And the number of previous blocks for calculating median timestamp is set as 11.
#[doc(hidden)]
pub struct MockMedianTime {
    headers: LinkedHashMap<Byte32, HeaderView>,
}

#[doc(hidden)]
pub const MOCK_MEDIAN_TIME_COUNT: usize = 11;

impl HeaderProvider for MockMedianTime {
    fn get_header(&self, hash: &Byte32) -> Option<HeaderView> {
        self.headers.get(hash).cloned()
    }

    fn timestamp_and_parent(&self, block_hash: &Byte32) -> (u64, BlockNumber, Byte32) {
        self.headers
            .get(block_hash)
            .map(|header| (header.timestamp(), header.number(), header.hash()))
            .unwrap()
    }
}

impl MockMedianTime {
    /// Create a new `MockMedianTime`.
    #[doc(hidden)]
    pub fn new(timestamps: Vec<u64>) -> Self {
        Self {
            headers: timestamps
                .iter()
                .enumerate()
                .map(|(idx, timestamp)| {
                    let header = HeaderBuilder::default()
                        .timestamp(timestamp.pack())
                        .number((idx as u64).pack())
                        .build();
                    (header.hash(), header)
                })
                .collect(),
        }
    }

    /// Return the last hash.
    #[doc(hidden)]
    pub fn get_last_block_hash(&self) -> Byte32 {
        self.headers.iter().last().unwrap().0.clone()
    }

    /// Return the block hash.
    #[doc(hidden)]
    pub fn get_block_hash(&self, block_number: BlockNumber) -> Byte32 {
        self.headers
            .iter()
            .nth(block_number as usize)
            .unwrap()
            .0
            .clone()
    }

    /// Return transaction info corresponding to the block number, block epoch and transaction index.
    #[doc(hidden)]
    pub fn get_transaction_info(
        &self,
        block_number: BlockNumber,
        block_epoch: EpochNumberWithFraction,
        index: usize,
    ) -> TransactionInfo {
        let block_hash = self.get_block_hash(block_number);
        TransactionInfo {
            block_number,
            block_epoch,
            block_hash,
            index,
        }
    }
}
