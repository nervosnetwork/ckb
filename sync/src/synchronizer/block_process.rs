use crate::synchronizer::Synchronizer;
use ckb_core::block::Block;
use ckb_network::{CKBProtocolContext, SessionId};
use ckb_protocol::Block as PBlock;
use ckb_shared::store::ChainStore;
use failure::Error as FailureError;
use log::debug;
use std::convert::TryInto;

pub struct BlockProcess<'a, CS: ChainStore + 'a> {
    message: &'a PBlock<'a>,
    synchronizer: &'a Synchronizer<CS>,
    peer: SessionId,
}

impl<'a, CS> BlockProcess<'a, CS>
where
    CS: ChainStore + 'a,
{
    pub fn new(
        message: &'a PBlock,
        synchronizer: &'a Synchronizer<CS>,
        peer: SessionId,
        _nc: &'a CKBProtocolContext,
    ) -> Self {
        BlockProcess {
            message,
            synchronizer,
            peer,
        }
    }

    pub fn execute(self) -> Result<(), FailureError> {
        let block: Block = (*self.message).try_into()?;
        debug!(target: "sync", "BlockProcess received block {} {:x}", block.header().number(), block.header().hash());

        self.synchronizer.peers.block_received(self.peer, &block);
        self.synchronizer.process_new_block(self.peer, block);
        Ok(())
    }
}
