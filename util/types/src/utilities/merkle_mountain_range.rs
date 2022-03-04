//! Types for variable difficulty Merkle Mountain Range (MMR) in CKB.
//!
//! Since CKB doesn't record MMR data in headers since the genesis block, so the index of MMR leaf
//! is NOT equal to the block number.
//!
//! ```
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
//! index                0   1   2   3   4   5   6   7   8   9   10
//!         --------------------------------------------------------
//! number               N  N+1 N+2 N+3 N+4 N+5 N+6 N+7 N+8 N+9 N+10
//!         --------------------------------------------------------
//! ```
//!
//! - `height`: the MMR node height.
//! - `position`: the MMR node position.
//! - `index`: the MMR leaf index.
//! - `number`: the block height.
//! - `N`: the activation block number; the block number of the last block which doesn't records MMR root hash.
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
//! - `blocks_count`
//!
//!   - For leaf node, it's `1`.
//!
//!   - For non-leaf node, it's the sum of `blocks_count` in it's child nodes.
//!
//! - `total_difficulty`
//!
//!   - For leaf node, it's the difficulty it took to mine the current block.
//!
//!   - For non-leaf node, it's the sum of `total_difficulty` in it's child nodes.
//!
//! TODO(light-client) Add more fields in MMR node.
//!
//! ## References
//!
//! - [Peter Todd, Merkle mountain range.](https://github.com/opentimestamps/opentimestamps-server/blob/master/doc/merkle-mountain-range.md).

use ckb_hash::new_blake2b;
use ckb_merkle_mountain_range::{Error, Merge, MerkleProof, MMR};

use crate::{core, packed, prelude::*, U256};

pub struct MergeHeaderDigest;

pub type ChainRootMMR<S> = MMR<packed::HeaderDigest, MergeHeaderDigest, S>;
pub type MMRProof = MerkleProof<packed::HeaderDigest, MergeHeaderDigest>;

impl core::BlockView {
    pub fn digest(&self) -> packed::HeaderDigest {
        self.header().digest()
    }
}

impl core::HeaderView {
    pub fn digest(&self) -> packed::HeaderDigest {
        self.data().digest()
    }
}

impl packed::Header {
    pub fn digest(&self) -> packed::HeaderDigest {
        let raw = self.raw();
        packed::HeaderDigest::new_builder()
            .blocks_count(1u64.pack())
            .total_difficulty(raw.difficulty().pack())
            .build()
    }
}

impl Merge for MergeHeaderDigest {
    type Item = packed::HeaderDigest;
    fn merge(lhs: &Self::Item, rhs: &Self::Item) -> Result<Self::Item, Error> {
        let hash = {
            let mut hasher = new_blake2b();
            let mut hash = [0u8; 32];
            hasher.update(&lhs.calc_mmr_hash().raw_data());
            hasher.update(&rhs.calc_mmr_hash().raw_data());
            hasher.finalize(&mut hash);
            hash
        };
        let blocks_count = {
            let l: u64 = lhs.blocks_count().unpack();
            let r: u64 = rhs.blocks_count().unpack();
            l + r
        };
        let total_difficulty = {
            let l: U256 = lhs.total_difficulty().unpack();
            let r: U256 = rhs.total_difficulty().unpack();
            l + r
        };
        Ok(Self::Item::new_builder()
            .hash(hash.pack())
            .blocks_count(blocks_count.pack())
            .total_difficulty(total_difficulty.pack())
            .build())
    }
}
