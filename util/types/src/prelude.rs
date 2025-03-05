//! This module includes several traits.
//!
//! Few traits are re-exported from other crates, few are used as aliases and others are syntactic sugar.
//!

pub use crate::utilities::merkle_mountain_range::ProverMessageBuilder;
use crate::{
    U256,
    core::{
        BlockBuilder, BlockView, ExtraHashView, HeaderBuilder, HeaderView, TransactionBuilder,
        TransactionView, UncleBlockView,
    },
    packed,
};

pub use ckb_gen_types::prelude::*;

use std::collections::HashSet;

/// Trait for converting types into `TransactionView`.
pub trait IntoTransactionView {
    /// Converts the implementing type into a `TransactionView`.
    fn into_view(self) -> TransactionView;
}

/// Trait for converting types into `HeaderView`.
pub trait IntoHeaderView {
    /// Converts the implementing type into a `HeaderView`.
    fn into_view(self) -> HeaderView;
}

/// Trait for converting types into `UncleBlockView`.
pub trait IntoUncleBlockView {
    /// Converts the implementing type into an `UncleBlockView`.
    fn into_view(self) -> UncleBlockView;
}

/// Trait for converting types into `BlockView`.
pub trait IntoBlockView {
    /// Converts the implementing type into a `BlockView` without resetting the header.
    fn into_view_without_reset_header(self) -> BlockView;

    /// Converts the implementing type into a `BlockView`.
    fn into_view(self) -> BlockView;

    /// Converts a packed block and associated data into a `BlockView`.
    fn block_into_view_internal(
        block: packed::Block,
        tx_hashes: Vec<packed::Byte32>,
        tx_witness_hashes: Vec<packed::Byte32>,
    ) -> BlockView;
}

/// Trait for obtaining an advanced builder for `BlockView`.
pub trait AsBlockBuilder {
    /// Creates a new advanced builder for `BlockView`.
    fn new_advanced_builder() -> BlockBuilder;

    /// Gets an advanced builder from the implementing type.
    fn as_advanced_builder(&self) -> BlockBuilder;
}

/// Trait for obtaining an advanced builder for `TransactionView`.
pub trait AsTransactionBuilder {
    /// Gets an advanced builder for `TransactionView` from the implementing type.
    fn as_advanced_builder(&self) -> TransactionBuilder;
}

/// Trait for obtaining an advanced builder for `HeaderView`.
pub trait AsHeaderBuilder {
    /// Gets an advanced builder for `HeaderView` from the implementing type.
    fn as_advanced_builder(&self) -> HeaderBuilder;
}

/// Trait for calculating difficulty.
pub trait Difficulty {
    /// Calculates and returns the difficulty value as a `U256`.
    fn difficulty(&self) -> U256;
}

/// Trait for building a compact block from a `BlockView`.
pub trait BuildCompactBlock {
    /// Builds a compact block from a `BlockView` and a set of prefilled transaction indexes.
    fn build_from_block(
        block: &BlockView,
        prefilled_transactions_indexes: &HashSet<usize>,
    ) -> packed::CompactBlock;

    /// Returns the short IDs of the transactions in the compact block.
    fn block_short_ids(&self) -> Vec<Option<packed::ProposalShortId>>;

    /// Returns the indexes of the short IDs in the compact block.
    fn short_id_indexes(&self) -> Vec<usize>;
}

/// Trait for resetting the header of a packed block.
pub trait ResetBlock {
    /// Resets the header of the packed block.
    fn reset_header(self) -> packed::Block;

    /// Resets the header of the packed block with given transaction hashes and witness hashes.
    fn reset_header_with_hashes(
        self,
        tx_hashes: &[packed::Byte32],
        tx_witness_hashes: &[packed::Byte32],
    ) -> packed::Block;
}

/// Trait for calculating the extra hash of a block.
pub trait CalcExtraHash {
    /// Calculates and returns the extra hash of the block as an `ExtraHashView`.
    fn calc_extra_hash(&self) -> ExtraHashView;
}
