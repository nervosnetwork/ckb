use ckb_core::cell::BlockInfo;
use ckb_core::{BlockNumber, EpochNumber};
use ckb_traits::BlockMedianTimeContext;
use numext_fixed_hash::H256;

pub struct MockMedianTime {
    timestamps: Vec<u64>,
}

impl BlockMedianTimeContext for MockMedianTime {
    fn median_block_count(&self) -> u64 {
        11
    }

    fn timestamp_and_parent(&self, block_hash: &H256) -> (u64, BlockNumber, H256) {
        for i in 0..self.timestamps.len() {
            if &Self::get_block_hash(i as u64) == block_hash {
                if i == 0 {
                    return (self.timestamps[i], i as u64, H256::zero());
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
    pub fn new(timestamps: Vec<u64>) -> Self {
        Self { timestamps }
    }

    pub fn get_block_hash(block_number: BlockNumber) -> H256 {
        let vec: Vec<u8> = (0..32).map(|_| block_number as u8).collect();
        H256::from_slice(vec.as_slice()).unwrap()
    }

    pub fn get_block_info(block_number: BlockNumber, epoch_number: EpochNumber) -> BlockInfo {
        let block_hash = Self::get_block_hash(block_number);
        BlockInfo::new(block_number, epoch_number, block_hash)
    }
}
