use crate::{attempt, synchronizer::Synchronizer, Status, StatusCode};
use ckb_core::block::Block;
use ckb_logger::debug;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::Block as PBlock;
use std::convert::TryInto;

pub struct BlockProcess<'a> {
    message: &'a PBlock<'a>,
    synchronizer: &'a Synchronizer,
    peer: PeerIndex,
    _nc: &'a CKBProtocolContext,
}

impl<'a> BlockProcess<'a> {
    pub fn new(
        message: &'a PBlock,
        synchronizer: &'a Synchronizer,
        peer: PeerIndex,
        _nc: &'a CKBProtocolContext,
    ) -> Self {
        BlockProcess {
            message,
            synchronizer,
            peer,
            _nc,
        }
    }

    pub fn execute(self) -> Status {
        let block: Block = attempt!(TryInto::<Block>::try_into(*self.message));
        let block_hash = block.header().hash().to_owned();
        debug!(
            "received Block {} {:#x}",
            block.header().number(),
            block_hash
        );

        if self.synchronizer.shared().new_block_received(&block) {
            if let Err(err) = self.synchronizer.process_new_block(self.peer, block) {
                return StatusCode::InvalidBlock.with_context(err);
            }
        }

        Status::ok()
    }
}
