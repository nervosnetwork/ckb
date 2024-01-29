use crate::synchronizer::Synchronizer;
use ckb_logger::debug;
use ckb_network::PeerIndex;
use ckb_types::{packed, prelude::*};

pub struct BlockProcess<'a> {
    message: packed::SendBlockReader<'a>,
    synchronizer: &'a Synchronizer,
    peer: PeerIndex,
}

impl<'a> BlockProcess<'a> {
    pub fn new(
        message: packed::SendBlockReader<'a>,
        synchronizer: &'a Synchronizer,
        peer: PeerIndex,
    ) -> Self {
        BlockProcess {
            message,
            synchronizer,
            peer,
        }
    }

    pub fn execute(self) -> crate::Status {
        let block = self.message.block().to_entity().into_view();
        debug!(
            "BlockProcess received block {} {}",
            block.number(),
            block.hash(),
        );
        let shared = self.synchronizer.shared();

        if shared.new_block_received(&block) {
            self.synchronizer
                .asynchronous_process_new_block(block.clone(), self.peer);
        }

        // block process is asynchronous, so we only return ignored here
        crate::Status::ignored()
    }

    #[cfg(test)]
    pub fn blocking_execute(self) -> crate::Status {
        let block = self.message.block().to_entity().into_view();
        debug!(
            "BlockProcess received block {} {}",
            block.number(),
            block.hash(),
        );
        let shared = self.synchronizer.shared();

        if shared.new_block_received(&block) {
            if let Err(err) = self
                .synchronizer
                .blocking_process_new_block(block.clone(), self.peer)
            {
                if !ckb_error::is_internal_db_error(&err) {
                    return crate::StatusCode::BlockIsInvalid.with_context(format!(
                        "{}, error: {}",
                        block.hash(),
                        err,
                    ));
                }
            }
        }
        crate::Status::ok()
    }
}
