use ckb_network::PeerId;
use ckb_types::core::{BlockNumber, EpochNumberWithFraction};
use ckb_types::packed::Byte32;
use ckb_types::prelude::{Entity, FromSliceShouldBeOk, Reader};
use ckb_types::{packed, U256};

pub mod header_map;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderIndexView {
    hash: Byte32,
    number: BlockNumber,
    epoch: EpochNumberWithFraction,
    timestamp: u64,
    parent_hash: Byte32,
    total_difficulty: U256,
    skip_hash: Option<Byte32>,
}

impl HeaderIndexView {
    pub fn new(
        hash: Byte32,
        number: BlockNumber,
        epoch: EpochNumberWithFraction,
        timestamp: u64,
        parent_hash: Byte32,
        total_difficulty: U256,
    ) -> Self {
        HeaderIndexView {
            hash,
            number,
            epoch,
            timestamp,
            parent_hash,
            total_difficulty,
            skip_hash: None,
        }
    }

    pub fn hash(&self) -> Byte32 {
        self.hash.clone()
    }

    pub fn number(&self) -> BlockNumber {
        self.number
    }

    pub fn epoch(&self) -> EpochNumberWithFraction {
        self.epoch
    }

    pub fn timestamp(&self) -> u64 {
        self.timestamp
    }

    pub fn total_difficulty(&self) -> &U256 {
        &self.total_difficulty
    }

    pub fn parent_hash(&self) -> Byte32 {
        self.parent_hash.clone()
    }

    pub fn skip_hash(&self) -> Option<&Byte32> {
        self.skip_hash.as_ref()
    }

    // deserialize from bytes
    fn from_slice_should_be_ok(hash: &[u8], slice: &[u8]) -> Self {
        let hash = packed::Byte32Reader::from_slice_should_be_ok(hash).to_entity();
        let number = BlockNumber::from_le_bytes(slice[0..8].try_into().expect("stored slice"));
        let epoch = EpochNumberWithFraction::from_full_value(u64::from_le_bytes(
            slice[8..16].try_into().expect("stored slice"),
        ));
        let timestamp = u64::from_le_bytes(slice[16..24].try_into().expect("stored slice"));
        let parent_hash = packed::Byte32Reader::from_slice_should_be_ok(&slice[24..56]).to_entity();
        let total_difficulty = U256::from_little_endian(&slice[56..88]).expect("stored slice");
        let skip_hash = if slice.len() == 120 {
            Some(packed::Byte32Reader::from_slice_should_be_ok(&slice[88..120]).to_entity())
        } else {
            None
        };
        Self {
            hash,
            number,
            epoch,
            timestamp,
            parent_hash,
            total_difficulty,
            skip_hash,
        }
    }

    // serialize all fields except `hash` to bytes
    fn to_vec(&self) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(self.number.to_le_bytes().as_slice());
        v.extend_from_slice(self.epoch.full_value().to_le_bytes().as_slice());
        v.extend_from_slice(self.timestamp.to_le_bytes().as_slice());
        v.extend_from_slice(self.parent_hash.as_slice());
        v.extend_from_slice(self.total_difficulty.to_le_bytes().as_slice());
        if let Some(ref skip_hash) = self.skip_hash {
            v.extend_from_slice(skip_hash.as_slice());
        }
        v
    }

    pub fn build_skip<F, G>(&mut self, tip_number: BlockNumber, get_header_view: F, fast_scanner: G)
    where
        F: Fn(&Byte32, bool) -> Option<HeaderIndexView>,
        G: Fn(BlockNumber, BlockNumberAndHash) -> Option<HeaderIndexView>,
    {
        if self.number == 0 {
            return;
        }
        self.skip_hash = self
            .get_ancestor(
                tip_number,
                get_skip_height(self.number()),
                get_header_view,
                fast_scanner,
            )
            .map(|header| header.hash());
    }

    pub fn get_ancestor<F, G>(
        &self,
        tip_number: BlockNumber,
        number: BlockNumber,
        get_header_view: F,
        fast_scanner: G,
    ) -> Option<HeaderIndexView>
    where
        F: Fn(&Byte32, bool) -> Option<HeaderIndexView>,
        G: Fn(BlockNumber, BlockNumberAndHash) -> Option<HeaderIndexView>,
    {
        if number > self.number() {
            return None;
        }

        let mut current = self.clone();
        let mut number_walk = current.number();
        while number_walk > number {
            let number_skip = get_skip_height(number_walk);
            let number_skip_prev = get_skip_height(number_walk - 1);
            let store_first = current.number() <= tip_number;
            match current.skip_hash {
                Some(ref hash)
                    if number_skip == number
                        || (number_skip > number
                            && !(number_skip_prev + 2 < number_skip
                                && number_skip_prev >= number)) =>
                {
                    // Only follow skip if parent->skip isn't better than skip->parent
                    current = get_header_view(hash, store_first)?;
                    number_walk = number_skip;
                }
                _ => {
                    current = get_header_view(&current.parent_hash(), store_first)?;
                    number_walk -= 1;
                }
            }
            if let Some(target) = fast_scanner(number, (current.number(), current.hash()).into()) {
                current = target;
                break;
            }
        }
        Some(current)
    }

    pub fn as_header_index(&self) -> HeaderIndex {
        HeaderIndex::new(self.number(), self.hash(), self.total_difficulty().clone())
    }

    pub fn number_and_hash(&self) -> BlockNumberAndHash {
        (self.number(), self.hash()).into()
    }

    pub fn is_better_than(&self, total_difficulty: &U256) -> bool {
        self.total_difficulty() > total_difficulty
    }
}

impl From<(ckb_types::core::HeaderView, U256)> for HeaderIndexView {
    fn from((header, total_difficulty): (ckb_types::core::HeaderView, U256)) -> Self {
        HeaderIndexView {
            hash: header.hash(),
            number: header.number(),
            epoch: header.epoch(),
            timestamp: header.timestamp(),
            parent_hash: header.parent_hash(),
            total_difficulty,
            skip_hash: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderIndex {
    number: BlockNumber,
    hash: Byte32,
    total_difficulty: U256,
}

impl HeaderIndex {
    pub fn new(number: BlockNumber, hash: Byte32, total_difficulty: U256) -> Self {
        HeaderIndex {
            number,
            hash,
            total_difficulty,
        }
    }

    pub fn number(&self) -> BlockNumber {
        self.number
    }

    pub fn hash(&self) -> Byte32 {
        self.hash.clone()
    }

    pub fn total_difficulty(&self) -> &U256 {
        &self.total_difficulty
    }

    pub fn number_and_hash(&self) -> BlockNumberAndHash {
        (self.number(), self.hash()).into()
    }

    pub fn is_better_chain(&self, other: &Self) -> bool {
        self.is_better_than(other.total_difficulty())
    }

    pub fn is_better_than(&self, other_total_difficulty: &U256) -> bool {
        self.total_difficulty() > other_total_difficulty
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockNumberAndHash {
    pub number: BlockNumber,
    pub hash: Byte32,
}

impl BlockNumberAndHash {
    pub fn new(number: BlockNumber, hash: Byte32) -> Self {
        Self { number, hash }
    }

    pub fn number(&self) -> BlockNumber {
        self.number
    }

    pub fn hash(&self) -> Byte32 {
        self.hash.clone()
    }
}

impl From<(BlockNumber, Byte32)> for BlockNumberAndHash {
    fn from(inner: (BlockNumber, Byte32)) -> Self {
        Self {
            number: inner.0,
            hash: inner.1,
        }
    }
}

impl From<&ckb_types::core::HeaderView> for BlockNumberAndHash {
    fn from(header: &ckb_types::core::HeaderView) -> Self {
        Self {
            number: header.number(),
            hash: header.hash(),
        }
    }
}

impl From<ckb_types::core::HeaderView> for BlockNumberAndHash {
    fn from(header: ckb_types::core::HeaderView) -> Self {
        Self {
            number: header.number(),
            hash: header.hash(),
        }
    }
}

// Compute what height to jump back to with the skip pointer.
fn get_skip_height(height: BlockNumber) -> BlockNumber {
    // Turn the lowest '1' bit in the binary representation of a number into a '0'.
    fn invert_lowest_one(n: i64) -> i64 {
        n & (n - 1)
    }

    if height < 2 {
        return 0;
    }

    // Determine which height to jump back to. Any number strictly lower than height is acceptable,
    // but the following expression seems to perform well in simulations (max 110 steps to go back
    // up to 2**18 blocks).
    if (height & 1) > 0 {
        invert_lowest_one(invert_lowest_one(height as i64 - 1)) as u64 + 1
    } else {
        invert_lowest_one(height as i64) as u64
    }
}

pub const SHRINK_THRESHOLD: usize = 300;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifyFailedBlockInfo {
    pub block_hash: Byte32,
    pub peer_id: PeerId,
}
