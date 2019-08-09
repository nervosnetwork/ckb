use crate::{
    synchronizer::{BlockStatus, Synchronizer},
    BAD_MESSAGE_BAN_TIME,
};
use ckb_core::block::Block;
use ckb_logger::{debug, info};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::Block as PBlock;
use failure::Error as FailureError;
use std::convert::TryInto;

pub struct BlockProcess<'a> {
    message: &'a PBlock<'a>,
    synchronizer: &'a Synchronizer,
    peer: PeerIndex,
    nc: &'a CKBProtocolContext,
}

impl<'a> BlockProcess<'a> {
    pub fn new(
        message: &'a PBlock,
        synchronizer: &'a Synchronizer,
        peer: PeerIndex,
        nc: &'a CKBProtocolContext,
    ) -> Self {
        BlockProcess {
            message,
            synchronizer,
            peer,
            nc,
        }
    }

    pub fn execute(self) -> Result<(), FailureError> {
        let block: Block = (*self.message).try_into()?;
        let block_hash = block.header().hash().to_owned();
        debug!(
            "BlockProcess received block {} {:x}",
            block.header().number(),
            block_hash
        );

        if self.synchronizer.shared().new_block_received(&block)
            && self
                .synchronizer
                .process_new_block(self.peer, block)
                .is_err()
        {
            info!(
                "Ban peer {:?} for {} seconds because send us a invalid block",
                self.peer,
                BAD_MESSAGE_BAN_TIME.as_secs()
            );
            self.synchronizer
                .shared()
                .insert_block_status(block_hash, BlockStatus::BLOCK_INVALID);
            self.nc.ban_peer(self.peer, BAD_MESSAGE_BAN_TIME);
        }

        Ok(())
    }
}
