use crate::synchronizer::Synchronizer;
use ckb_core::block::Block;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::Block as PBlock;
use ckb_shared::index::ChainIndex;
use ckb_util::TryInto;
use failure::Error as FailureError;
use log::debug;

pub struct BlockProcess<'a, CI: ChainIndex + 'a> {
    message: &'a PBlock<'a>,
    synchronizer: &'a Synchronizer<CI>,
    peer: PeerIndex,
}

impl<'a, CI> BlockProcess<'a, CI>
where
    CI: ChainIndex + 'a,
{
    pub fn new(
        message: &'a PBlock,
        synchronizer: &'a Synchronizer<CI>,
        peer: PeerIndex,
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
