use crate::synchronizer::Synchronizer;
use crate::types::post_sync_process;
use crate::StatusCode;
use ckb_chain::RemoteBlock;
use ckb_error::is_internal_db_error;
use ckb_logger::{debug, info};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::packed::Byte32;
use ckb_types::{packed, prelude::*};
use std::sync::Arc;

pub struct BlockProcess<'a> {
    message: packed::SendBlockReader<'a>,
    synchronizer: &'a Synchronizer,
    peer: PeerIndex,
    nc: Arc<dyn CKBProtocolContext + Sync>,
}

impl<'a> BlockProcess<'a> {
    pub fn new(
        message: packed::SendBlockReader<'a>,
        synchronizer: &'a Synchronizer,
        peer: PeerIndex,
        nc: Arc<dyn CKBProtocolContext + Sync>,
    ) -> Self {
        BlockProcess {
            message,
            synchronizer,
            peer,
            nc,
        }
    }

    pub fn execute(self) -> crate::Status {
        let block = Arc::new(self.message.block().to_entity().into_view());
        debug!(
            "BlockProcess received block {} {}",
            block.number(),
            block.hash(),
        );
        let shared = self.synchronizer.shared();

        if shared.new_block_received(&block) {
            let verify_callback = {
                let nc: Arc<dyn CKBProtocolContext + Sync> = Arc::clone(&self.nc);
                let peer_id: PeerIndex = self.peer;
                let block_hash: Byte32 = block.hash();
                Box::new(move |verify_result: Result<bool, ckb_error::Error>| {
                    match verify_result {
                        Ok(_) => {}
                        Err(err) => {
                            let is_internal_db_error = is_internal_db_error(&err);
                            if is_internal_db_error {
                                return;
                            }

                            // punish the malicious peer
                            post_sync_process(
                                nc.as_ref(),
                                peer_id,
                                "SendBlock",
                                StatusCode::BlockIsInvalid.with_context(format!(
                                    "block {} is invalid, reason: {}",
                                    block_hash,
                                    err.to_string()
                                )),
                            );
                        }
                    };
                })
            };
            let remote_block = RemoteBlock {
                block,
                verify_callback,
            };
            self.synchronizer
                .asynchronous_process_remote_block(remote_block);
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
