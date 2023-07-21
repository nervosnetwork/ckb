//! This module includes several traits.
//!
//! Few traits are re-exported from other crates, few are used as aliases and others are syntactic sugar.
//!

pub use crate::utilities::merkle_mountain_range::ProverMessageBuilder;
use crate::{
    core::{
        BlockBuilder, BlockView, HeaderBuilder, HeaderView, TransactionBuilder, TransactionView,
        UncleBlockView,
    },
    packed, U256,
};

pub use ckb_gen_types::prelude::*;

use std::collections::HashSet;

pub trait IntoTransactionView {
    fn into_view(self) -> TransactionView;
}

pub trait IntoHeaderView {
    fn into_view(self) -> HeaderView;
}

pub trait IntoUncleBlockView {
    fn into_view(self) -> UncleBlockView;
}

pub trait IntoBlockView {
    fn into_view_without_reset_header(self) -> BlockView;
    fn into_view(self) -> BlockView;
    fn block_into_view_internal(
        block: packed::Block,
        tx_hashes: Vec<packed::Byte32>,
        tx_witness_hashes: Vec<packed::Byte32>,
    ) -> BlockView;
}

pub trait AsBlockBuilder {
    fn new_advanced_builder() -> BlockBuilder;
    fn as_advanced_builder(&self) -> BlockBuilder;
}
pub trait AsTransactionBuilder {
    fn as_advanced_builder(&self) -> TransactionBuilder;
}

pub trait AsHeaderBuilder {
    fn as_advanced_builder(&self) -> HeaderBuilder;
}

pub trait Difficulty {
    fn difficulty(&self) -> U256;
}

pub trait BuildCompactBlock {
    fn build_from_block(
        block: &BlockView,
        prefilled_transactions_indexes: &HashSet<usize>,
    ) -> packed::CompactBlock;
    fn block_short_ids(&self) -> Vec<Option<packed::ProposalShortId>>;
    fn short_id_indexes(&self) -> Vec<usize>;
}

pub trait ResetBlock {
    fn reset_header(self) -> packed::Block;
    fn reset_header_with_hashes(
        self,
        tx_hashes: &[packed::Byte32],
        tx_witness_hashes: &[packed::Byte32],
    ) -> packed::Block;
}
