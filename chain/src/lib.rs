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
use ckb_shared::types::VerifyFailedBlockInfo;
use ckb_types::core::service::Request;
use ckb_types::core::{BlockNumber, BlockView, HeaderView};
use ckb_types::packed::Byte32;
use ckb_verification_traits::Switch;
use std::sync::Arc;
mod chain_service;
mod consume_orphan;
mod consume_unverified;
#[cfg(test)]
mod tests;
mod utils;

pub use chain_service::{start_chain_services, ChainController};

type ProcessBlockRequest = Request<LonelyBlockWithCallback, ()>;
type TruncateRequest = Request<Byte32, Result<(), Error>>;

/// VerifyResult is the result type to represent the result of block verification
pub type VerifyResult = Result<VerifiedBlockStatus, Error>;

/// VerifyCallback is the callback type to be called after block verification
pub type VerifyCallback = Box<dyn FnOnce(VerifyResult) + Send + Sync>;

/// VerifiedBlockStatus is
#[derive(Debug, Clone, PartialEq)]
pub enum VerifiedBlockStatus {
    /// The block is being seen for the first time, and VM have verified it
    FirstSeenAndVerified,

    /// The block is being seen for the first time
    /// but VM have not verified it since its a uncle block
    UncleBlockNotVerified,

    /// The block has been verified before.
    PreviouslySeenAndVerified,
}

/// LonelyBlock is the block which we have not check weather its parent is stored yet
#[derive(Clone)]
pub struct LonelyBlock {
    /// block
    pub block: Arc<BlockView>,

    /// This block is received from which peer, and the message bytes size
    pub peer_id_with_msg_bytes: Option<(PeerIndex, u64)>,

    /// The Switch to control the verification process
    pub switch: Option<Switch>,
}

impl LonelyBlock {
    /// Combine with verify_callback, convert it to LonelyBlockWithCallback
    pub fn with_callback(self, verify_callback: Option<VerifyCallback>) -> LonelyBlockWithCallback {
        LonelyBlockWithCallback {
            lonely_block: self,
            verify_callback,
        }
    }

    /// Combine with empty verify_callback, convert it to LonelyBlockWithCallback
    pub fn without_callback(self) -> LonelyBlockWithCallback {
        self.with_callback(None)
    }
}

/// LonelyBlockWithCallback Combine LonelyBlock with an optional verify_callback
pub struct LonelyBlockWithCallback {
    /// The LonelyBlock
    pub lonely_block: LonelyBlock,
    /// The optional verify_callback
    pub verify_callback: Option<VerifyCallback>,
}

impl LonelyBlockWithCallback {
    pub(crate) fn execute_callback(self, verify_result: VerifyResult) {
        if let Some(verify_callback) = self.verify_callback {
            verify_callback(verify_result);
        }
    }

    /// Get reference to block
    pub fn block(&self) -> &Arc<BlockView> {
        &self.lonely_block.block
    }

    /// get peer_id and msg_bytes
    pub fn peer_id_with_msg_bytes(&self) -> Option<(PeerIndex, u64)> {
        self.lonely_block.peer_id_with_msg_bytes
    }

    /// get switch param
    pub fn switch(&self) -> Option<Switch> {
        self.lonely_block.switch
    }
}

impl LonelyBlockWithCallback {
    pub(crate) fn combine_parent_header(self, parent_header: HeaderView) -> UnverifiedBlock {
        UnverifiedBlock {
            unverified_block: self,
            parent_header,
        }
    }
}

pub(crate) struct UnverifiedBlock {
    pub unverified_block: LonelyBlockWithCallback,
    pub parent_header: HeaderView,
}

impl UnverifiedBlock {
    pub(crate) fn block(&self) -> &Arc<BlockView> {
        self.unverified_block.block()
    }

    pub fn peer_id_with_msg_bytes(&self) -> Option<(PeerIndex, u64)> {
        self.unverified_block.peer_id_with_msg_bytes()
    }

    pub fn execute_callback(self, verify_result: VerifyResult) {
        self.unverified_block.execute_callback(verify_result)
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
    peer_id_with_msg_bytes: Option<(PeerIndex, u64)>,
    block_hash: Byte32,
    err: &Error,
) {
    let is_internal_db_error = is_internal_db_error(err);
    match peer_id_with_msg_bytes {
        Some((peer_id, msg_bytes)) => {
            let verify_failed_block_info = VerifyFailedBlockInfo {
                block_hash,
                peer_id,
                msg_bytes,
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
