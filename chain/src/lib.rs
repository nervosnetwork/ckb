//! CKB chain service.
//!
//! [`ChainService`] background base on database, handle block importing,
//! the [`ChainController`] is responsible for receive the request and returning response
//!
//! [`ChainService`]: chain/struct.ChainService.html
//! [`ChainController`]: chain/struct.ChainController.html
use ckb_error::Error;
use ckb_shared::types::{BlockNumberAndHash, VerifyFailedBlockInfo};
use ckb_types::core::service::Request;
use ckb_types::core::{BlockNumber, BlockView, HeaderView};
use ckb_types::packed::Byte32;
use ckb_verification_traits::Switch;
use std::sync::Arc;

mod chain_controller;
mod chain_service;
mod consume_orphan;
mod consume_unverified;
#[cfg(test)]
mod tests;
mod utils;

pub use chain_controller::ChainController;
pub use chain_service::start_chain_services;
pub use consume_orphan::store_unverified_block;

type ProcessBlockRequest = Request<LonelyBlock, ()>;
type TruncateRequest = Request<Byte32, Result<(), Error>>;

/// VerifyResult is the result type to represent the result of block verification
///
/// Ok(true) : it's a newly verified block
/// Ok(false): it's a block which has been verified before
/// Err(err) : it's a block which failed to verify
pub type VerifyResult = Result<bool, Error>;

/// VerifyCallback is the callback type to be called after block verification
pub type VerifyCallback = Box<dyn FnOnce(VerifyResult) + Send + Sync>;

/// RemoteBlock is received from ckb-sync and ckb-relayer
pub struct RemoteBlock {
    /// block
    pub block: Arc<BlockView>,

    /// Relayer and Synchronizer will have callback to ban peer
    pub verify_callback: VerifyCallback,
}

/// LonelyBlock is the block which we have not check weather its parent is stored yet
pub struct LonelyBlock {
    /// block
    pub block: Arc<BlockView>,

    /// The Switch to control the verification process
    pub switch: Option<Switch>,

    /// The optional verify_callback
    pub verify_callback: Option<VerifyCallback>,
}

/// LonelyBlock is the block which we have not check weather its parent is stored yet
pub struct LonelyBlockHash {
    /// block
    pub block_number_and_hash: BlockNumberAndHash,

    /// The Switch to control the verification process
    pub switch: Option<Switch>,

    /// The optional verify_callback
    pub verify_callback: Option<VerifyCallback>,
}

impl LonelyBlockHash {
    pub(crate) fn execute_callback(self, verify_result: VerifyResult) {
        if let Some(verify_callback) = self.verify_callback {
            verify_callback(verify_result);
        }
    }
}

impl From<LonelyBlock> for LonelyBlockHash {
    fn from(val: LonelyBlock) -> Self {
        LonelyBlockHash {
            block_number_and_hash: BlockNumberAndHash {
                number: val.block.number(),
                hash: val.block.hash(),
            },
            switch: val.switch,
            verify_callback: val.verify_callback,
        }
    }
}

impl LonelyBlock {
    pub(crate) fn block(&self) -> &Arc<BlockView> {
        &self.block
    }

    pub fn switch(&self) -> Option<Switch> {
        self.switch
    }

    pub fn execute_callback(self, verify_result: VerifyResult) {
        if let Some(verify_callback) = self.verify_callback {
            verify_callback(verify_result);
        }
    }
}

pub(crate) struct UnverifiedBlock {
    pub lonely_block: LonelyBlock,
    pub parent_header: HeaderView,
}

impl UnverifiedBlock {
    pub(crate) fn block(&self) -> &Arc<BlockView> {
        self.lonely_block.block()
    }

    pub fn switch(&self) -> Option<Switch> {
        self.lonely_block.switch()
    }

    pub fn execute_callback(self, verify_result: VerifyResult) {
        self.lonely_block.execute_callback(verify_result)
    }
}

pub(crate) struct GlobalIndex {
    pub(crate) number: BlockNumber,
    pub(crate) hash: Byte32,
    pub(crate) unseen: bool,
}

impl GlobalIndex {
    pub(crate) fn new(number: BlockNumber, hash: Byte32, unseen: bool) -> GlobalIndex {
        GlobalIndex {
            number,
            hash,
            unseen,
        }
    }

    pub(crate) fn forward(&mut self, hash: Byte32) {
        self.number -= 1;
        self.hash = hash;
    }
}
