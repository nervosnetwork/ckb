use crate::{
    synchronizer::{BlockStatus, Synchronizer},
    Status, StatusCode,
};
use ckb_logger::debug;
use ckb_network::PeerIndex;
use ckb_types::core::BlockView;
use ckb_types::{packed, prelude::*};

pub struct BlockProcess<'a> {
    block: BlockView,
    synchronizer: &'a Synchronizer,
    peer: PeerIndex,
}

impl<'a> BlockProcess<'a> {
    pub fn new(
        message: packed::SendBlockReader<'a>,
        synchronizer: &'a Synchronizer,
        peer: PeerIndex,
    ) -> Self {
        let block = message.block().to_entity().into_view();
        BlockProcess {
            block,
            synchronizer,
            peer,
        }
    }

    pub fn execute(self) -> Status {
        {
            fail::fail_point!("recv_sendblock", |_| {
                let number = self.block.number();
                let block_hash = self.block.hash();
                ckb_logger::debug!(
                    "[failpoint] recv_sendblock(number={}, block_hash={:?}) from {}",
                    number,
                    block_hash,
                    self.peer
                );
                Status::ignored()
            })
        }

        let block = self.block;
        debug!(
            "BlockProcess received block {} {}",
            block.number(),
            block.hash(),
        );
        let shared = self.synchronizer.shared();
        let state = shared.state();

        if state.new_block_received(&block) {
            if let Err(err) = self
                .synchronizer
                .process_new_block(self.peer, block.clone())
            {
                state.insert_block_status(block.hash(), BlockStatus::BLOCK_INVALID);
                return StatusCode::BlockIsInvalid.with_context(format!(
                    "{}, error: {}",
                    block.hash(),
                    err,
                ));
            }
        } else if shared
            .active_chain()
            .contains_block_status(&block.hash(), BlockStatus::BLOCK_STORED)
        {
            state
                .peers()
                .set_last_common_header(self.peer, block.header());
        }

        Status::ok()
    }
}
