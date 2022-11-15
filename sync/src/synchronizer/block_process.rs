use crate::synchronizer::SendBlockMsgInfo;
use crate::{synchronizer::Synchronizer, utils::is_internal_db_error, Status, StatusCode};
use ckb_logger::{debug, error, info};
use ckb_network::PeerIndex;
use ckb_types::core::BlockView;
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

    pub fn execute(self) -> Status {
        let block = self.message.block().to_entity().into_view();
        debug!(
            "BlockProcess received block {} {}",
            block.number(),
            block.hash(),
        );
        let shared = self.synchronizer.shared();
        let state = shared.state();

        if !shared.active_chain().is_initial_block_download() {
            // determine should we drop the queue and consumer handle

            let mut queue_is_empty = false;
            if let Some(queue) = self
                .synchronizer
                .block_queue
                .read()
                .expect("BlockProcess wants to acquire read lock on block_queue to check if block_queue is empty, but it has poisoned")
                .as_ref()
            {
                if queue.is_empty() {
                    queue_is_empty = true;
                }
            }

            if queue_is_empty {
                // drop block_queue and consumer handle
                let block_queue = self.synchronizer.block_queue.write().expect("BlockProcess wants to acquire write lock on block_queue to drop block_queue, but it has poisoned").take();
                drop(block_queue);

                let consumer_handle = self
                    .synchronizer
                    .block_queue_consumer_handle
                    .write()
                    .expect("BlockProcess wants to acquire write lock on block_queue_consumer_handle to drop block_queue_consumer_handle , but it has poisoned")
                    .take();
                drop(consumer_handle);

                info!("both block queue and consumer handle are dropped");
            }

            // not in IBD mode, consume block by internal_process_block
            return self.internal_process_block(block);
        }

        match self.synchronizer.block_queue.read().expect("BlockProcess wants to acquire read lock on block_queue to push blocks to block_queue, but it has poisoned").as_ref() {
            Some(queue) => {
                if state.new_block_received(&block) {
                    let msg_info = SendBlockMsgInfo {
                        peer: self.peer,
                        item_name: "SendBlock".to_string(),
                        item_bytes_length: self.message.as_slice().len() as u64,
                        item_id: 2,
                    };
                    if let Err(not_pushed_block) = queue.push((block, msg_info)) {
                        // block_queue is full, so Process the block now
                        // This rarely happens
                        let hash = not_pushed_block.0.hash();
                        if let Err(err) = self.synchronizer.process_new_block(not_pushed_block.0) {
                            if !is_internal_db_error(&err) {
                                error!("block {} is invalid: {}", hash, err);
                                return StatusCode::BlockIsInvalid
                                    .with_context(format!("{}, error: {}", hash, err,));
                            }
                        }
                    }
                }
                Status::ignored()
            }
            None => self.internal_process_block(block),
        }
    }

    fn internal_process_block(&self, block: BlockView) -> Status {
        if self
            .synchronizer
            .shared()
            .state()
            .new_block_received(&block)
        {
            let hash = block.hash();
            if let Err(err) = self.synchronizer.process_new_block(block) {
                if !is_internal_db_error(&err) {
                    return StatusCode::BlockIsInvalid
                        .with_context(format!("{}, error: {}", hash, err,));
                }
            }
        }

        Status::ok()
    }
}
