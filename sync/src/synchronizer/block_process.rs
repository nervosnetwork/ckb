use crate::{synchronizer::Synchronizer, BAD_MESSAGE_BAN_TIME};
use ckb_core::block::Block;
use ckb_logger::{debug, info};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::Block as PBlock;
use ckb_store::ChainStore;
use failure::Error as FailureError;
use std::convert::TryInto;

pub struct BlockProcess<'a, CS: ChainStore + 'a> {
    message: &'a PBlock<'a>,
    synchronizer: &'a Synchronizer<CS>,
    peer: PeerIndex,
    nc: &'a CKBProtocolContext,
}

impl<'a, CS> BlockProcess<'a, CS>
where
    CS: ChainStore + 'a,
{
    pub fn new(
        message: &'a PBlock,
        synchronizer: &'a Synchronizer<CS>,
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
        debug!(
            "BlockProcess received block {} {:x}",
            block.header().number(),
            block.header().hash()
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
            self.nc.ban_peer(self.peer, BAD_MESSAGE_BAN_TIME);
        }

        Ok(())
    }
}
