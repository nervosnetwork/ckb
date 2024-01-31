//! CKB chain service.
//!
//! [`ChainService`] background base on database, handle block importing,
//! the [`ChainController`] is responsible for receive the request and returning response
//!
//! [`ChainService`]: chain/struct.ChainService.html
//! [`ChainController`]: chain/struct.ChainController.html
use ckb_error::{is_internal_db_error, Error};
use ckb_logger::{debug, error};
use ckb_network::PeerIndex;
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

    /// This block is received from which peer
    pub peer_id: PeerIndex,
}

/// LonelyBlock is the block which we have not check weather its parent is stored yet
pub struct LonelyBlock {
    /// block
    pub block: Arc<BlockView>,

    /// This block is received from which peer
    pub peer_id: Option<PeerIndex>,

    /// The Switch to control the verification process
    pub switch: Option<Switch>,

    /// The optional verify_callback
    pub verify_callback: Option<VerifyCallback>,
}

/// LonelyBlock is the block which we have not check weather its parent is stored yet
pub struct LonelyBlockHash {
    /// block
    pub block_number_and_hash: BlockNumberAndHash,

    /// This block is received from which peer
    pub peer_id: Option<PeerIndex>,

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
            peer_id: val.peer_id,
            switch: val.switch,
            verify_callback: val.verify_callback,
        }
    }
}

impl LonelyBlock {
    pub(crate) fn block(&self) -> &Arc<BlockView> {
        &self.block
    }

    pub fn peer_id(&self) -> Option<PeerIndex> {
        self.peer_id
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

    pub fn peer_id(&self) -> Option<PeerIndex> {
        self.lonely_block.peer_id()
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

pub(crate) fn tell_synchronizer_to_punish_the_bad_peer(
    verify_failed_blocks_tx: tokio::sync::mpsc::UnboundedSender<VerifyFailedBlockInfo>,
    peer_id: Option<PeerIndex>,
    block_hash: Byte32,
    err: &Error,
) {
    let is_internal_db_error = is_internal_db_error(err);
    match peer_id {
        Some(peer_id) => {
            let verify_failed_block_info = VerifyFailedBlockInfo {
                block_hash,
                peer_id,
                reason: err.to_string(),
                is_internal_db_error,
            };
            if let Err(_err) = verify_failed_blocks_tx.send(verify_failed_block_info) {
                error!("ChainService failed to send verify failed block info to Synchronizer, the receiver side may have been closed, this shouldn't happen")
            }
        }
        _ => {
            debug!("Don't know which peer to punish, or don't have a channel Sender to Synchronizer, skip it")
        }
    }
}
