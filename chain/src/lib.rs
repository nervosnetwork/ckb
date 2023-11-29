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

pub type VerifyResult = Result<VerifiedBlockStatus, Error>;

pub type VerifyCallback = Box<dyn FnOnce(VerifyResult) + Send + Sync>;

/// VerifiedBlockStatus is
#[derive(Debug, Clone, PartialEq)]
pub enum VerifiedBlockStatus {
    // The block is being seen for the first time, and VM have verified it
    FirstSeenAndVerified,

    // The block is being seen for the first time
    // but VM have not verified it since its a uncle block
    UncleBlockNotVerified,

    // The block has been verified before.
    PreviouslySeenAndVerified,
}

#[derive(Clone)]
pub struct LonelyBlock {
    pub block: Arc<BlockView>,
    pub peer_id: Option<PeerIndex>,
    pub switch: Option<Switch>,
}

impl LonelyBlock {
    pub fn with_callback(self, verify_callback: Option<VerifyCallback>) -> LonelyBlockWithCallback {
        LonelyBlockWithCallback {
            lonely_block: self,
            verify_callback,
        }
    }

    pub fn without_callback(self) -> LonelyBlockWithCallback {
        self.with_callback(None)
    }
}

pub struct LonelyBlockWithCallback {
    pub lonely_block: LonelyBlock,
    pub verify_callback: Option<VerifyCallback>,
}

impl LonelyBlockWithCallback {
    pub(crate) fn execute_callback(self, verify_result: VerifyResult) {
        match self.verify_callback {
            Some(verify_callback) => {
                verify_callback(verify_result);
            }
            None => {}
        }
    }

    pub fn block(&self) -> &Arc<BlockView> {
        &self.lonely_block.block
    }
    pub fn peer_id(&self) -> Option<PeerIndex> {
        self.lonely_block.peer_id
    }
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

    pub fn peer_id(&self) -> Option<PeerIndex> {
        self.unverified_block.peer_id()
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
    peer_id: Option<PeerIndex>,
    block_hash: Byte32,
    err: &Error,
) {
    let is_internal_db_error = is_internal_db_error(&err);
    match peer_id {
        Some(peer_id) => {
            let verify_failed_block_info = VerifyFailedBlockInfo {
                block_hash,
                peer_id,
                message_bytes: 0,
                reason: err.to_string(),
                is_internal_db_error,
            };
            match verify_failed_blocks_tx.send(verify_failed_block_info) {
                Err(_err) => {
                    error!("ChainService failed to send verify failed block info to Synchronizer, the receiver side may have been closed, this shouldn't happen")
                }
                _ => {}
            }
        }
        _ => {
            debug!("Don't know which peer to punish, or don't have a channel Sender to Synchronizer, skip it")
        }
    }
}
