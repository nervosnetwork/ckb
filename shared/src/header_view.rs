//! Wrapped core::HeaderView with total_difficulty and skip_hash
use ckb_types::{
    core::{self, BlockNumber},
    packed::{self, Byte32},
    prelude::*,
    U256,
};

/// Wrapped core::HeaderView with total_difficulty and skip_hash
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderView {
    inner: core::HeaderView,
    total_difficulty: U256,
    // pointer to the index of some further predecessor of this block
    skip_hash: Option<Byte32>,
}

impl HeaderView {
    /// Initialize a HeaderView
    pub fn new(inner: core::HeaderView, total_difficulty: U256) -> Self {
        HeaderView {
            inner,
            total_difficulty,
            skip_hash: None,
        }
    }

    /// Get BlockNumber of HeaderView
    pub fn number(&self) -> BlockNumber {
        self.inner.number()
    }

    /// Get hash of HeaderView
    pub fn hash(&self) -> Byte32 {
        self.inner.hash()
    }

    /// Get parent hash of HeaderView
    pub fn parent_hash(&self) -> Byte32 {
        self.inner.data().raw().parent_hash()
    }

    /// Get skip hash of HeaderView
    pub fn skip_hash(&self) -> Option<&Byte32> {
        self.skip_hash.as_ref()
    }

    /// Get timestamp of HeaderView
    pub fn timestamp(&self) -> u64 {
        self.inner.timestamp()
    }

    /// Get total_difficulty of HeaderView
    pub fn total_difficulty(&self) -> &U256 {
        &self.total_difficulty
    }

    /// Get inner value of HeaderView
    pub fn inner(&self) -> &core::HeaderView {
        &self.inner
    }

    /// Get inner value of HeaderView
    pub fn into_inner(self) -> core::HeaderView {
        self.inner
    }

    /// Build skip_hash
    pub fn build_skip<F, G>(&mut self, tip_number: BlockNumber, get_header_view: F, fast_scanner: G)
    where
        F: FnMut(&Byte32, Option<bool>) -> Option<HeaderView>,
        G: Fn(BlockNumber, &HeaderView) -> Option<HeaderView>,
    {
        if self.inner.is_genesis() {
            return;
        }
        self.skip_hash = self
            .clone()
            .get_ancestor(
                tip_number,
                get_skip_height(self.number()),
                get_header_view,
                fast_scanner,
            )
            .map(|header| header.hash());
    }

    /// NOTE: get_header_view may change source state, for cache or for tests
    pub fn get_ancestor<F, G>(
        self,
        tip_number: BlockNumber,
        number: BlockNumber,
        mut get_header_view: F,
        fast_scanner: G,
    ) -> Option<core::HeaderView>
    where
        F: FnMut(&Byte32, Option<bool>) -> Option<HeaderView>,
        G: Fn(BlockNumber, &HeaderView) -> Option<HeaderView>,
    {
        let mut current = self;
        if number > current.number() {
            return None;
        }

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
                    current = get_header_view(hash, Some(store_first))?;
                    number_walk = number_skip;
                }
                _ => {
                    current = get_header_view(&current.parent_hash(), Some(store_first))?;
                    number_walk -= 1;
                }
            }
            if let Some(target) = fast_scanner(number, &current) {
                current = target;
                break;
            }
        }
        Some(current).map(HeaderView::into_inner)
    }

    /// Check if this HeaderView is better than target total_difficulty
    pub fn is_better_than(&self, total_difficulty: &U256) -> bool {
        self.total_difficulty() > total_difficulty
    }

    pub(crate) fn from_slice_should_be_ok(slice: &[u8]) -> Self {
        let len_size = packed::Uint32Reader::TOTAL_SIZE;
        if slice.len() < len_size {
            panic!("failed to unpack item in header map: header part is broken");
        }
        let mut idx = 0;
        let inner_len = {
            let reader = packed::Uint32Reader::from_slice_should_be_ok(&slice[idx..idx + len_size]);
            Unpack::<u32>::unpack(&reader) as usize
        };
        idx += len_size;
        let total_difficulty_len = packed::Uint256Reader::TOTAL_SIZE;
        if slice.len() < len_size + inner_len + total_difficulty_len {
            panic!("failed to unpack item in header map: body part is broken");
        }
        let inner = {
            let reader =
                packed::HeaderViewReader::from_slice_should_be_ok(&slice[idx..idx + inner_len]);
            Unpack::<core::HeaderView>::unpack(&reader)
        };
        idx += inner_len;
        let total_difficulty = {
            let reader = packed::Uint256Reader::from_slice_should_be_ok(
                &slice[idx..idx + total_difficulty_len],
            );
            Unpack::<U256>::unpack(&reader)
        };
        idx += total_difficulty_len;
        let skip_hash = {
            packed::Byte32OptReader::from_slice_should_be_ok(&slice[idx..])
                .to_entity()
                .to_opt()
        };
        Self {
            inner,
            total_difficulty,
            skip_hash,
        }
    }

    pub(crate) fn to_vec(&self) -> Vec<u8> {
        let mut v = Vec::new();
        let inner: packed::HeaderView = self.inner.pack();
        let total_difficulty: packed::Uint256 = self.total_difficulty.pack();
        let skip_hash: packed::Byte32Opt = Pack::pack(&self.skip_hash);
        let inner_len: packed::Uint32 = (inner.as_slice().len() as u32).pack();
        v.extend_from_slice(inner_len.as_slice());
        v.extend_from_slice(inner.as_slice());
        v.extend_from_slice(total_difficulty.as_slice());
        v.extend_from_slice(skip_hash.as_slice());
        v
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
