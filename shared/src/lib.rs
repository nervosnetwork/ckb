//! # The Chain Library
//!
//! This Library contains the `ChainProvider` traits and `Chain` implement:
//!
//! - [ChainProvider](chain::chain::ChainProvider) provide index
//!   and store interface.
//! - [Chain](chain::chain::Chain) represent a struct which
//!   implement `ChainProvider`

pub mod cell_set;
pub mod chain_state;
pub mod error;
pub mod shared;
pub mod tx_pool;
mod tx_proposal_table;

#[cfg(test)]
mod tests;

use ckb_db::Col;

pub const COLUMNS: u32 = 11;
pub const COLUMN_INDEX: Col = 0;
pub const COLUMN_BLOCK_HEADER: Col = 1;
pub const COLUMN_BLOCK_BODY: Col = 2;
pub const COLUMN_BLOCK_UNCLE: Col = 3;
pub const COLUMN_META: Col = 4;
pub const COLUMN_TRANSACTION_ADDR: Col = 5;
pub const COLUMN_EXT: Col = 6;
pub const COLUMN_BLOCK_TRANSACTION_ADDRESSES: Col = 7;
pub const COLUMN_BLOCK_PROPOSAL_IDS: Col = 8;
pub const COLUMN_BLOCK_EPOCH: Col = 9;
pub const COLUMN_EPOCH: Col = 10;
