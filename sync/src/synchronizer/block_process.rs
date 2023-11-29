use crate::synchronizer::Synchronizer;
use ckb_chain::VerifyResult;
use ckb_logger::debug;
use ckb_network::PeerIndex;
use ckb_types::{packed, prelude::*};

pub struct BlockProcess<'a> {
    message: packed::SendBlockReader<'a>,
    synchronizer: &'a Synchronizer,
    peer: PeerIndex,
    message_bytes: u64,
}

impl<'a> BlockProcess<'a> {
    pub fn new(
        message: packed::SendBlockReader<'a>,
        synchronizer: &'a Synchronizer,
        peer: PeerIndex,
        message_bytes: u64,
    ) -> Self {
        BlockProcess {
            message,
            synchronizer,
            peer,
            message_bytes,
        }
    }

    pub fn execute(self) {
        let block = self.message.block().to_entity().into_view();
        debug!(
            "BlockProcess received block {} {}",
            block.number(),
            block.hash(),
        );
        let shared = self.synchronizer.shared();

        if shared.new_block_received(&block) {
            self.synchronizer.asynchronous_process_new_block(
                block.clone(),
                self.peer,
                self.message_bytes,
            );
        }
    }
}
