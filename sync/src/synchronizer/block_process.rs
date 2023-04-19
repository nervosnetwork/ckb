use crate::{
    block_status::BlockStatus, synchronizer::Synchronizer, utils::is_internal_db_error, Status,
    StatusCode,
};
use ckb_logger::debug;
use ckb_network::PeerIndex;
use ckb_types::{packed, prelude::*};

pub struct BlockProcess<'a> {
    message: packed::SendBlockReader<'a>,
    synchronizer: &'a Synchronizer,
    _peer: PeerIndex,
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
            _peer: peer,
        }
    }

    pub fn execute(self) -> Status {
        let block = self.message.block().to_entity().into_view();
        debug!(
            "BlockProcess received block {} {}",
            block.number(),
            block.hash(),
        );
        let shared = self.synchronizer.shared();
        let state = shared.state();

        let status = shared.active_chain().get_block_status(&block.hash());
        if matches!(status, BlockStatus::UNKNOWN | BlockStatus::BLOCK_INVALID) {
            return StatusCode::BlockIsInvalid.with_context(format!(
                "BlockProcess refuse this block: block status is {:?}, block_hash: {}",
                status,
                block.hash(),
            ));
        }

        if state.new_block_received(&block) {
            if let Err(err) = self.synchronizer.process_new_block(block.clone()) {
                if !is_internal_db_error(&err) {
                    return StatusCode::BlockIsInvalid.with_context(format!(
                        "{}, error: {}",
                        block.hash(),
                        err,
                    ));
                }
            }
        }

        Status::ok()
    }
}
