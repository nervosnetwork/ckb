#![allow(missing_docs)]

//! CKB chain service.
//!
//! [`ChainService`] background base on database, handle block importing,
//! the [`ChainController`] is responsible for receive the request and returning response
//!
//! [`ChainService`]: chain/struct.ChainService.html
//! [`ChainController`]: chain/struct.ChainController.html
use ckb_error::Error;
use ckb_types::core::service::Request;
use ckb_types::core::{BlockNumber, BlockView, EpochNumber, HeaderView};
use ckb_types::packed::Byte32;
use ckb_verification_traits::Switch;
use std::sync::Arc;

mod chain_controller;
mod chain_service;
mod init;
mod init_load_unverified;
mod orphan_broker;
mod preload_unverified_blocks_channel;
#[cfg(test)]
mod tests;
mod utils;
pub mod verify;

pub use chain_controller::ChainController;
use ckb_logger::{error, info};
use ckb_store::{ChainDB, ChainStore};
use ckb_types::{BlockNumberAndHash, H256};
pub use init::{ChainServiceScope, build_chain_services, start_chain_services};

type ProcessBlockRequest = Request<LonelyBlock, ()>;
type TruncateRequest = Request<Byte32, Result<(), Error>>;

/// VerifyResult is the result type to represent the result of block verification
///
/// Ok(true) : it's a newly verified block
/// Ok(false): it's a block is a uncle block, not verified
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

    pub parent_hash: Byte32,

    pub epoch_number: EpochNumber,

    /// The Switch to control the verification process
    pub switch: Option<Switch>,

    /// The optional verify_callback
    pub verify_callback: Option<VerifyCallback>,
}

impl From<LonelyBlock> for LonelyBlockHash {
    fn from(val: LonelyBlock) -> Self {
        let LonelyBlock {
            block,
            switch,
            verify_callback,
        } = val;
        let block_hash_h256: H256 = block.hash().into();
        let block_number: BlockNumber = block.number();
        let parent_hash_h256: H256 = block.parent_hash().into();
        let block_hash = block_hash_h256.into();
        let parent_hash = parent_hash_h256.into();

        let epoch_number: EpochNumber = block.epoch().number();

        LonelyBlockHash {
            block_number_and_hash: BlockNumberAndHash {
                number: block_number,
                hash: block_hash,
            },
            parent_hash,
            epoch_number,
            switch,
            verify_callback,
        }
    }
}

impl LonelyBlockHash {
    pub fn execute_callback(self, verify_result: VerifyResult) {
        if let Some(verify_callback) = self.verify_callback {
            verify_callback(verify_result);
        }
    }

    pub fn number_hash(&self) -> BlockNumberAndHash {
        self.block_number_and_hash.clone()
    }

    pub fn epoch_number(&self) -> EpochNumber {
        self.epoch_number
    }

    pub fn hash(&self) -> Byte32 {
        self.block_number_and_hash.hash()
    }

    pub fn parent_hash(&self) -> Byte32 {
        self.parent_hash.clone()
    }

    pub fn number(&self) -> BlockNumber {
        self.block_number_and_hash.number()
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

/// UnverifiedBlock will be consumed by ConsumeUnverified thread
struct UnverifiedBlock {
    // block
    block: Arc<BlockView>,
    // the switch to control the verification process
    switch: Option<Switch>,
    // verify callback
    verify_callback: Option<VerifyCallback>,
    // parent header
    parent_header: HeaderView,
}

pub(crate) fn delete_unverified_block(
    store: &ChainDB,
    block_hash: Byte32,
    block_number: BlockNumber,
    parent_hash: Byte32,
) {
    info!(
        "parent: {}, deleting this block {}-{}",
        parent_hash, block_number, block_hash,
    );

    let db_txn = store.begin_transaction();
    let block_op: Option<BlockView> = db_txn.get_block(&block_hash);
    match block_op {
        Some(block) => {
            if let Err(err) = db_txn.delete_block(&block) {
                error!(
                    "delete block {}-{} failed {:?}",
                    block_number, block_hash, err
                );
                return;
            }
            if let Err(err) = db_txn.commit() {
                error!(
                    "commit delete block {}-{} failed {:?}",
                    block_number, block_hash, err
                );
                return;
            }

            info!(
                "parent: {}, deleted this block {}-{}",
                parent_hash, block_number, block_hash,
            );
        }
        None => {
            error!(
                "want to delete block {}-{}, but it not found in db",
                block_number, block_hash
            );
        }
    }
}
