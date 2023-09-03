//! Types for variable difficulty Merkle Mountain Range (MMR) in CKB.
//!
//! ## References
//!
//! - [CKB RFC 0044](https://github.com/nervosnetwork/rfcs/blob/master/rfcs/0044-ckb-light-client/0044-ckb-light-client.md)

use ckb_hash::new_blake2b;
use ckb_merkle_mountain_range::{Error as MMRError, Merge, MerkleProof, Result as MMRResult, MMR};

use crate::{
    core,
    core::{BlockNumber, EpochNumber, EpochNumberWithFraction, ExtraHashView, HeaderView},
    packed::{self, OutPoint},
    prelude::*,
    utilities::compact_to_difficulty,
    H256, U256,
};

/// A struct to implement MMR `Merge` trait, which is used for MMR of chain
pub struct MergeHeaderDigest;
/// MMR for chain root
pub type ChainRootMMR<S> = MMR<packed::HeaderDigest, MergeHeaderDigest, S>;
/// MMR proof for chain root
pub type ChainRootMMRProof = MerkleProof<packed::HeaderDigest, MergeHeaderDigest>;

/// A struct to implement MMR `Merge` trait, which is used for MMR of transaction output
pub struct MergeH256;
/// MMR for transaction output root
pub type CellsRootMMR<S> = MMR<H256, MergeH256, S>;
/// MMR proof for transaction output root
pub type CellsRootMMRProof = MerkleProof<H256, MergeH256>;
/// A struct represents the status of a transaction output
#[derive(Debug, Clone)]
pub struct CellStatus {
    /// The position of the transaction output in MMR
    pub mmr_position: u64,
    /// The block number when the transaction output is created
    pub created_by: BlockNumber,
    /// The block number when the transaction output is consumed, default is `BlockNumber::MAX`, which means the transaction output is live
    pub consumed_by: BlockNumber,
}

/// A Header and the fields which are used to do verification for its extra hash.
#[derive(Debug, Clone)]
pub struct VerifiableHeader {
    header: HeaderView,
    uncles_hash: packed::Byte32,
    extension: Option<packed::Bytes>,
    parent_chain_root: packed::HeaderDigest,
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
        let raw = self.data().raw();
        packed::HeaderDigest::new_builder()
            .children_hash(self.hash())
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

/// Trait for representing a header digest.
pub trait HeaderDigest {
    /// Verify the header digest
    fn verify(&self) -> Result<(), String>;
}

impl HeaderDigest for packed::HeaderDigest {
    /// Verify the MMR header digest
    fn verify(&self) -> Result<(), String> {
        // 1. Check block numbers.
        let start_number: BlockNumber = self.start_number().unpack();
        let end_number: BlockNumber = self.end_number().unpack();
        if start_number > end_number {
            let errmsg = format!(
                "failed since the start block number is bigger than the end ([{start_number},{end_number}])"
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
                "failed since the start epoch is bigger than the end ([{start_epoch:#},{end_epoch:#}])"
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
                    "failed since the compact targets should be same during epochs ([{start_epoch:#},{end_epoch:#}])"
                );
                return Err(errmsg);
            } else {
                // Sum all blocks difficulties to check total difficulty.
                let blocks_count = end_number - start_number + 1;
                let block_difficulty = compact_to_difficulty(start_compact_target);
                let total_difficulty_calculated = block_difficulty * blocks_count;
                if total_difficulty != total_difficulty_calculated {
                    let errmsg = format!(
                        "failed since total difficulty is {total_difficulty} but the calculated is {total_difficulty_calculated} \
                        during epochs ([{start_epoch:#},{end_epoch:#}])"
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
        let children_hash = {
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
                "failed since the blocks isn't continuous ([-,{lhs_end_number}], [{rhs_start_number},-])"
            );
            return Err(MMRError::MergeError(errmsg));
        }

        // 2. Check epochs.
        let lhs_end_epoch: EpochNumberWithFraction = lhs.end_epoch().unpack();
        let rhs_start_epoch: EpochNumberWithFraction = rhs.start_epoch().unpack();
        if !rhs_start_epoch.is_successor_of(lhs_end_epoch) && !lhs_end_epoch.is_genesis() {
            let errmsg = format!(
                "failed since the epochs isn't continuous ([-,{lhs_end_epoch:#}], [{rhs_start_epoch:#},-])",
            );
            return Err(MMRError::MergeError(errmsg));
        }

        Ok(Self::Item::new_builder()
            .children_hash(children_hash.pack())
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

impl From<packed::VerifiableHeader> for VerifiableHeader {
    fn from(raw: packed::VerifiableHeader) -> Self {
        Self::new(
            raw.header().into_view(),
            raw.uncles_hash(),
            raw.extension().to_opt(),
            raw.parent_chain_root(),
        )
    }
}

impl VerifiableHeader {
    /// Creates a new verifiable header.
    pub fn new(
        header: HeaderView,
        uncles_hash: packed::Byte32,
        extension: Option<packed::Bytes>,
        parent_chain_root: packed::HeaderDigest,
    ) -> Self {
        Self {
            header,
            uncles_hash,
            extension,
            parent_chain_root,
        }
    }

    /// Checks if the current verifiable header is valid.
    pub fn is_valid(&self, mmr_activated_epoch: EpochNumber) -> bool {
        let has_chain_root = self.header().epoch().number() >= mmr_activated_epoch;
        if has_chain_root {
            if self.header().is_genesis() {
                if !self.parent_chain_root().is_default() {
                    return false;
                }
            } else {
                let is_extension_beginning_with_chain_root_hash = self
                    .extension()
                    .map(|extension| {
                        let actual_extension_data = extension.raw_data();
                        let parent_chain_root_hash = self.parent_chain_root().calc_mmr_hash();
                        actual_extension_data.starts_with(parent_chain_root_hash.as_slice())
                    })
                    .unwrap_or(false);
                if !is_extension_beginning_with_chain_root_hash {
                    return false;
                }
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

    /// Returns the chain root for its parent block.
    pub fn parent_chain_root(&self) -> packed::HeaderDigest {
        self.parent_chain_root.clone()
    }

    /// Returns the total difficulty.
    pub fn total_difficulty(&self) -> U256 {
        let parent_total_difficulty: U256 = self.parent_chain_root.total_difficulty().unpack();
        let block_difficulty = compact_to_difficulty(self.header.compact_target());
        parent_total_difficulty + block_difficulty
    }
}

/// A builder which builds the content of a message that used for proving.
pub trait ProverMessageBuilder: Builder
where
    Self::Entity: Into<packed::LightClientMessageUnion>,
{
    /// The type of the proved items.
    type ProvedItems;
    /// The type of the missing items.
    type MissingItems;
    /// Set the verifiable header which includes the chain root.
    fn set_last_header(self, last_header: packed::VerifiableHeader) -> Self;
    /// Set the proof for all items which require verifying.
    fn set_proof(self, proof: packed::HeaderDigestVec) -> Self;
    /// Set the proved items.
    fn set_proved_items(self, items: Self::ProvedItems) -> Self;
    /// Set the missing items.
    fn set_missing_items(self, items: Self::MissingItems) -> Self;
}

impl ProverMessageBuilder for packed::SendLastStateProofBuilder {
    type ProvedItems = packed::VerifiableHeaderVec;
    type MissingItems = ();
    fn set_last_header(self, last_header: packed::VerifiableHeader) -> Self {
        self.last_header(last_header)
    }
    fn set_proof(self, proof: packed::HeaderDigestVec) -> Self {
        self.proof(proof)
    }
    fn set_proved_items(self, items: Self::ProvedItems) -> Self {
        self.headers(items)
    }
    fn set_missing_items(self, _: Self::MissingItems) -> Self {
        self
    }
}

impl ProverMessageBuilder for packed::SendBlocksProofBuilder {
    type ProvedItems = packed::HeaderVec;
    type MissingItems = packed::Byte32Vec;
    fn set_last_header(self, last_header: packed::VerifiableHeader) -> Self {
        self.last_header(last_header)
    }
    fn set_proof(self, proof: packed::HeaderDigestVec) -> Self {
        self.proof(proof)
    }
    fn set_proved_items(self, items: Self::ProvedItems) -> Self {
        self.headers(items)
    }
    fn set_missing_items(self, items: Self::MissingItems) -> Self {
        self.missing_block_hashes(items)
    }
}

impl ProverMessageBuilder for packed::SendTransactionsProofBuilder {
    type ProvedItems = packed::FilteredBlockVec;
    type MissingItems = packed::Byte32Vec;
    fn set_last_header(self, last_header: packed::VerifiableHeader) -> Self {
        self.last_header(last_header)
    }
    fn set_proof(self, proof: packed::HeaderDigestVec) -> Self {
        self.proof(proof)
    }
    fn set_proved_items(self, items: Self::ProvedItems) -> Self {
        self.filtered_blocks(items)
    }
    fn set_missing_items(self, items: Self::MissingItems) -> Self {
        self.missing_tx_hashes(items)
    }
}

impl Merge for MergeH256 {
    type Item = H256;

    fn merge(lhs: &Self::Item, rhs: &Self::Item) -> MMRResult<Self::Item> {
        let mut hasher = new_blake2b();
        let mut hash = [0u8; 32];
        hasher.update(&lhs.0);
        hasher.update(&rhs.0);
        hasher.finalize(&mut hash);
        Ok(H256(hash))
    }

    fn merge_peaks(lhs: &Self::Item, rhs: &Self::Item) -> MMRResult<Self::Item> {
        Self::merge(rhs, lhs)
    }
}

impl CellStatus {
    /// Creates a new `CellStatus`, the default `CellStatus` is live
    pub fn new(mmr_position: u64, created_by: BlockNumber) -> Self {
        Self {
            mmr_position,
            created_by,
            consumed_by: BlockNumber::MAX,
        }
    }

    /// Creates a new `CellStatus` from slice, the length of slice must be 24
    pub fn new_unchecked(data: &[u8]) -> Self {
        let mmr_position =
            u64::from_le_bytes(data[0..8].try_into().expect("slice with incorrect length"));
        let created_by =
            u64::from_le_bytes(data[8..16].try_into().expect("slice with incorrect length"));
        let consumed_by = u64::from_le_bytes(
            data[16..24]
                .try_into()
                .expect("slice with incorrect length"),
        );
        Self {
            mmr_position,
            created_by,
            consumed_by,
        }
    }

    /// Mark the `CellStatus` as dead
    pub fn mark_as_consumed(&mut self, consumed_by: BlockNumber) {
        self.consumed_by = consumed_by;
    }

    /// Mark the `CellStatus` as live
    pub fn mark_as_live(&mut self) {
        self.consumed_by = BlockNumber::MAX;
    }

    /// Convert `CellStatus` to bytes
    pub fn to_vec(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(24);
        buf.extend_from_slice(&self.mmr_position.to_le_bytes());
        buf.extend_from_slice(&self.created_by.to_le_bytes());
        buf.extend_from_slice(&self.consumed_by.to_le_bytes());
        buf
    }
}

/// Hash out_point and status, the result is used as mmr leaf node
pub fn hash_out_point_and_status(
    out_point: &OutPoint,
    created_by: BlockNumber,
    consumed_by: BlockNumber,
) -> H256 {
    let mut hasher = new_blake2b();
    hasher.update(out_point.as_slice());
    hasher.update(created_by.to_le_bytes().as_slice());
    hasher.update(consumed_by.to_le_bytes().as_slice());
    let mut hash = [0u8; 32];
    hasher.finalize(&mut hash);
    H256(hash)
}
