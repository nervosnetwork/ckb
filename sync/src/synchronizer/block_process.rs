use crate::{
    synchronizer::{BlockStatus, Synchronizer},
    BAD_MESSAGE_BAN_TIME,
};
use ckb_logger::{debug, info};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{packed, prelude::*};
use failure::Error as FailureError;

pub struct BlockProcess<'a> {
    message: packed::SendBlockReader<'a>,
    synchronizer: &'a Synchronizer,
    peer: PeerIndex,
    nc: &'a CKBProtocolContext,
}

impl<'a> BlockProcess<'a> {
    pub fn new(
        message: packed::SendBlockReader<'a>,
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
        let block = self.message.block().to_entity().into_view();
        debug!(
            "BlockProcess received block {} {}",
            block.number(),
            block.hash(),
        );

        if self.synchronizer.shared().new_block_received(&block)
            && self
                .synchronizer
                .process_new_block(self.peer, block.clone())
                .is_err()
        {
            info!(
                "Ban peer {:?} for {} seconds, reason: it sent us an invalid block",
                self.peer,
                BAD_MESSAGE_BAN_TIME.as_secs()
            );
            self.synchronizer
                .shared()
                .insert_block_status(block.hash(), BlockStatus::BLOCK_INVALID);
            self.nc.ban_peer(self.peer, BAD_MESSAGE_BAN_TIME);
        }

        Ok(())
    }
}
