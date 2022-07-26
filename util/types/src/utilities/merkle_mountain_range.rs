//! Types for variable difficulty Merkle Mountain Range (MMR) in CKB.
//!
//! Since CKB doesn't record MMR data in headers since the genesis block, we use an activation
//! block number to enable MMR and create all MMR nodes before that block to make sure that the
//! index of MMR leaf is EQUAL to the block number.
//!
//! ```text
//!          height                position
//!
//!             3                     14
//!                                  /  \
//!                                 /    \
//!                                /      \
//!                               /        \
//!                              /          \
//!                             /            \
//!             2              6             13
//!                           / \            / \
//!                          /   \          /   \
//!                         /     \        /     \
//!             1          2       5       9      12      17
//!                       / \     / \     / \    /  \    /  \
//!             0        0   1   3   4   7   8  10  11  15  16   18
//!         --------------------------------------------------------
//! index                0   1   2   3   4   5   6   7   8   9   10 ... N
//!         --------------------------------------------------------
//! number               0   1   2   3   4   5   6   7   8   9   10 ... N
//!         --------------------------------------------------------
//! ```
//!
//! - `height`: the MMR node height.
//! - `position`: the MMR node position.
//! - `index`: the MMR leaf index; same as the block height.
//! - `number`: the block height.
//! - `N`: the activation block number; the block number of the last block which doesn't records MMR root hash.
//!
//! There are three kind of blocks base on its MMR data:
//!
//! - The genesis block
//!
//!   First node, also first leaf node in MMR; no chain root;
//!
//! - The blocks which height is less than `N`
//!
//!   No chain root in blocks but store them in database.
//!
//! - The blocks which height is equal to or greater than `N`
//!
//!   Has chain root in blocks.
//!
//! There are two kinds of MMR nodes: leaf node and non-leaf node.
//!
//! Each MMR node is defined as follows:
//!
//! - `hash`
//!
//!   - For leaf node, it's an empty hash (`0x0000...0000`).
//!
//!   - For non-leaf node, it's the hash of it's child nodes' hashes (concatenate serialized data).
//!
//! - `total_difficulty`
//!
//!  - For leaf node, it's the difficulty it took to mine the current block.
//!
//!  - For non-leaf node, it's the sum of `total_difficulty` in it's child nodes.
//!
//! - `start_*`
//!
//!   Such as `start_number`, `start_epoch`, `start_timestamp`, `start_compact_target`.
//!
//!   - For leaf node, it's the data of current block.
//!
//!   - For non-leaf node, it's the `start_*` of left node.
//!
//! - `end_*`
//!
//!   Such as `end_number`, `end_epoch`, `end_timestamp`, `end_compact_target`.
//!
//!   - For leaf node, it's the data of current block.
//!
//!   - For non-leaf node, it's the `end_*` of right node.
//!
//! ## References
//!
//! - [Peter Todd, Merkle mountain range.](https://github.com/opentimestamps/opentimestamps-server/blob/master/doc/merkle-mountain-range.md).

use ckb_hash::new_blake2b;
use ckb_merkle_mountain_range::{Error as MMRError, Merge, MerkleProof, Result as MMRResult, MMR};

use crate::{
    core,
    core::{BlockNumber, EpochNumber, EpochNumberWithFraction, ExtraHashView, HeaderView},
    packed,
    prelude::*,
    utilities::compact_to_difficulty,
    U256,
};

/// A struct to implement MMR `Merge` trait
pub struct MergeHeaderDigest;
/// MMR root
pub type ChainRootMMR<S> = MMR<packed::HeaderDigest, MergeHeaderDigest, S>;
/// MMR proof
pub type MMRProof = MerkleProof<packed::HeaderDigest, MergeHeaderDigest>;

/// A Header and the fields which are used to do verification for its extra hash.
#[derive(Debug, Clone)]
pub struct VerifiableHeader {
    header: HeaderView,
    uncles_hash: packed::Byte32,
    extension: Option<packed::Bytes>,
}

impl core::BlockView {
    /// Get the MMR header digest of the block
    pub fn digest(&self) -> packed::HeaderDigest {
        self.header().digest()
    }
}

impl core::HeaderView {
    /// Get the MMR header digest of the header
    pub fn digest(&self) -> packed::HeaderDigest {
        self.data().digest()
    }
}

impl packed::Header {
    /// Get the MMR header digest of the header
    pub fn digest(&self) -> packed::HeaderDigest {
        let raw = self.raw();
        packed::HeaderDigest::new_builder()
            .total_difficulty(self.difficulty().pack())
            .start_number(raw.number())
            .end_number(raw.number())
            .start_epoch(raw.epoch())
            .end_epoch(raw.epoch())
            .start_timestamp(raw.timestamp())
            .end_timestamp(raw.timestamp())
            .start_compact_target(raw.compact_target())
            .end_compact_target(raw.compact_target())
            .build()
    }
}

impl packed::HeaderDigest {
    /// Verify the MMR header digest
    pub fn verify(&self) -> Result<(), String> {
        // 1. Check block numbers.
        let start_number: BlockNumber = self.start_number().unpack();
        let end_number: BlockNumber = self.end_number().unpack();
        if start_number > end_number {
            let errmsg = format!(
                "failed since the start block number is bigger than the end ([{},{}])",
                start_number, end_number
            );
            return Err(errmsg);
        }

        // 2. Check epochs.
        let start_epoch: EpochNumberWithFraction = self.start_epoch().unpack();
        let end_epoch: EpochNumberWithFraction = self.end_epoch().unpack();
        let start_epoch_number = start_epoch.number();
        let end_epoch_number = end_epoch.number();
        if start_epoch != end_epoch
            && ((start_epoch_number > end_epoch_number)
                || (start_epoch_number == end_epoch_number
                    && start_epoch.index() > end_epoch.index()))
        {
            let errmsg = format!(
                "failed since the start epoch is bigger than the end ([{:#},{:#}])",
                start_epoch, end_epoch
            );
            return Err(errmsg);
        }

        // 3. Check difficulties when in the same epoch.
        let start_compact_target: u32 = self.start_compact_target().unpack();
        let end_compact_target: u32 = self.end_compact_target().unpack();
        let total_difficulty: U256 = self.total_difficulty().unpack();
        if start_epoch_number == end_epoch_number {
            if start_compact_target != end_compact_target {
                // In the same epoch, all compact targets should be same.
                let errmsg = format!(
                    "failed since the compact targets should be same during epochs ([{:#},{:#}])",
                    start_epoch, end_epoch
                );
                return Err(errmsg);
            } else {
                // Sum all blocks difficulties to check total difficulty.
                let blocks_count = end_number - start_number + 1;
                let block_difficulty = compact_to_difficulty(start_compact_target);
                let total_difficulty_calculated = block_difficulty * blocks_count;
                if total_difficulty != total_difficulty_calculated {
                    let errmsg = format!(
                        "failed since total difficulty is {} but the calculated is {} \
                        during epochs ([{:#},{:#}])",
                        total_difficulty, total_difficulty_calculated, start_epoch, end_epoch
                    );
                    return Err(errmsg);
                }
            }
        }

        Ok(())
    }
}

impl Merge for MergeHeaderDigest {
    type Item = packed::HeaderDigest;

    fn merge(lhs: &Self::Item, rhs: &Self::Item) -> MMRResult<Self::Item> {
        let hash = {
            let mut hasher = new_blake2b();
            let mut hash = [0u8; 32];
            hasher.update(&lhs.calc_mmr_hash().raw_data());
            hasher.update(&rhs.calc_mmr_hash().raw_data());
            hasher.finalize(&mut hash);
            hash
        };

        let total_difficulty = {
            let l: U256 = lhs.total_difficulty().unpack();
            let r: U256 = rhs.total_difficulty().unpack();
            l + r
        };

        // 1. Check block numbers.
        let lhs_end_number: BlockNumber = lhs.end_number().unpack();
        let rhs_start_number: BlockNumber = rhs.start_number().unpack();
        if lhs_end_number + 1 != rhs_start_number {
            let errmsg = format!(
                "failed since the blocks isn't continuous ([-,{}], [{},-])",
                lhs_end_number, rhs_start_number
            );
            return Err(MMRError::MergeError(errmsg));
        }

        // 2. Check epochs.
        let lhs_end_epoch: EpochNumberWithFraction = lhs.end_epoch().unpack();
        let rhs_start_epoch: EpochNumberWithFraction = rhs.start_epoch().unpack();
        if !rhs_start_epoch.is_successor_of(lhs_end_epoch) && !lhs_end_epoch.is_genesis() {
            let errmsg = format!(
                "failed since the epochs isn't continuous ([-,{:#}], [{:#},-])",
                lhs_end_epoch, rhs_start_epoch
            );
            return Err(MMRError::MergeError(errmsg));
        }

        Ok(Self::Item::new_builder()
            .hash(hash.pack())
            .total_difficulty(total_difficulty.pack())
            .start_number(lhs.start_number())
            .start_epoch(lhs.start_epoch())
            .start_timestamp(lhs.start_timestamp())
            .start_compact_target(lhs.start_compact_target())
            .end_number(rhs.end_number())
            .end_epoch(rhs.end_epoch())
            .end_timestamp(rhs.end_timestamp())
            .end_compact_target(rhs.end_compact_target())
            .build())
    }

    fn merge_peaks(lhs: &Self::Item, rhs: &Self::Item) -> MMRResult<Self::Item> {
        Self::merge(rhs, lhs)
    }
}

impl PartialEq for VerifiableHeader {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.header == other.header
            && self.uncles_hash == other.uncles_hash
            && self.extension.is_none() == other.extension.is_none()
            && (self.extension.is_none()
                || self
                    .extension
                    .as_ref()
                    .expect("checked: is not none")
                    .as_slice()
                    == other
                        .extension
                        .as_ref()
                        .expect("checked: is not none")
                        .as_slice())
    }
}

impl Eq for VerifiableHeader {}

impl From<packed::VerifiableHeader> for VerifiableHeader {
    fn from(raw: packed::VerifiableHeader) -> Self {
        Self::new(
            raw.header().into_view(),
            raw.uncles_hash(),
            raw.extension().to_opt(),
        )
    }
}

impl VerifiableHeader {
    /// Creates a new verifiable header.
    pub fn new(
        header: HeaderView,
        uncles_hash: packed::Byte32,
        extension: Option<packed::Bytes>,
    ) -> Self {
        Self {
            header,
            uncles_hash,
            extension,
        }
    }

    /// Creates a new verifiable header from a header with chain root.
    pub fn new_from_header_with_chain_root(
        header_with_chain_root: packed::HeaderWithChainRoot,
        mmr_activated_epoch: EpochNumber,
    ) -> Self {
        let header = header_with_chain_root.header().into_view();
        let uncles_hash = header_with_chain_root.uncles_hash();
        let extension = if header.epoch().number() >= mmr_activated_epoch {
            let bytes = header_with_chain_root
                .chain_root()
                .calc_mmr_hash()
                .as_bytes()
                .pack();
            Some(bytes)
        } else {
            None
        };
        Self::new(header, uncles_hash, extension)
    }

    /// Checks if the current verifiable header is valid.
    pub fn is_valid(
        &self,
        mmr_activated_epoch: EpochNumber,
        expected_root_hash_opt: Option<&packed::Byte32>,
    ) -> bool {
        let has_chain_root = self.header().epoch().number() >= mmr_activated_epoch;
        if has_chain_root {
            let is_extension_beginning_with_mmr_chain_root = self
                .extension()
                .map(|extension| {
                    let actual_extension_data = extension.raw_data();
                    actual_extension_data.len() < 32
                        || expected_root_hash_opt
                            .map(|hash| actual_extension_data.slice(..32) != hash.as_slice())
                            .unwrap_or(false)
                })
                .unwrap_or(false);
            if !is_extension_beginning_with_mmr_chain_root {
                return false;
            }
        }

        let expected_extension_hash = self
            .extension()
            .map(|extension| extension.calc_raw_data_hash());
        let extra_hash_view = ExtraHashView::new(self.uncles_hash(), expected_extension_hash);
        let expected_extra_hash = extra_hash_view.extra_hash();
        let actual_extra_hash = self.header().extra_hash();
        expected_extra_hash == actual_extra_hash
    }

    /// Returns the header.
    pub fn header(&self) -> &HeaderView {
        &self.header
    }

    /// Returns the uncles hash.
    pub fn uncles_hash(&self) -> packed::Byte32 {
        self.uncles_hash.clone()
    }

    /// Returns the extension.
    pub fn extension(&self) -> Option<packed::Bytes> {
        self.extension.clone()
    }
}
