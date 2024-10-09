use ckb_traits::{HeaderFields, HeaderFieldsProvider};
use ckb_types::{
    core::{BlockNumber, EpochNumberWithFraction, HeaderBuilder, HeaderView, TransactionInfo},
    packed::Byte32,
    prelude::*,
};
use ckb_util::LinkedHashMap;
use std::sync::Arc;

/// There's a consensus rule to verify that the block timestamp must be larger than
/// the median timestamp of the previous 37 blocks.
///
/// `MockMedianTime` is a mock for the median time in testing.
/// And the number of previous blocks for calculating median timestamp is set as 11.
#[doc(hidden)]
#[derive(Clone)]
pub struct MockMedianTime {
    headers: Arc<LinkedHashMap<Byte32, HeaderView>>,
}

#[doc(hidden)]
pub const MOCK_MEDIAN_TIME_COUNT: usize = 11;

#[doc(hidden)]
pub const MOCK_EPOCH_LENGTH: BlockNumber = 1000;

impl HeaderFieldsProvider for MockMedianTime {
    fn get_header_fields(&self, hash: &Byte32) -> Option<HeaderFields> {
        self.headers.get(hash).cloned().map(|header| HeaderFields {
            hash: header.hash(),
            number: header.number(),
            epoch: header.epoch(),
            timestamp: header.timestamp(),
            parent_hash: header.parent_hash(),
        })
    }
}

impl MockMedianTime {
    /// Create a new `MockMedianTime`.
    #[doc(hidden)]
    pub fn new(timestamps: Vec<u64>) -> Self {
        let mut parent_hash = Byte32::zero();
        Self {
            headers: Arc::new(
                timestamps
                    .iter()
                    .enumerate()
                    .map(|(idx, timestamp)| {
                        let number = idx as BlockNumber;
                        let header = HeaderBuilder::default()
                            .timestamp(timestamp.pack())
                            .number(number.pack())
                            .epoch(
                                EpochNumberWithFraction::new(
                                    number % MOCK_EPOCH_LENGTH,
                                    number / MOCK_EPOCH_LENGTH,
                                    MOCK_EPOCH_LENGTH,
                                )
                                .pack(),
                            )
                            .parent_hash(parent_hash.clone())
                            .build();
                        parent_hash = header.hash();
                        (header.hash(), header)
                    })
                    .collect(),
            ),
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
