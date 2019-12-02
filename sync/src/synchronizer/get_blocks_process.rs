use crate::block_status::BlockStatus;
use crate::synchronizer::Synchronizer;
use crate::{Status, StatusCode, MAX_BLOCKS_IN_TRANSIT_PER_PEER, MAX_HEADERS_LEN};
use ckb_logger::debug;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{packed, prelude::*};

pub struct GetBlocksProcess<'a> {
    message: packed::GetBlocksReader<'a>,
    synchronizer: &'a Synchronizer,
    nc: &'a dyn CKBProtocolContext,
    peer: PeerIndex,
}

impl<'a> GetBlocksProcess<'a> {
    pub fn new(
        message: packed::GetBlocksReader<'a>,
        synchronizer: &'a Synchronizer,
        peer: PeerIndex,
        nc: &'a dyn CKBProtocolContext,
    ) -> Self {
        GetBlocksProcess {
            peer,
            message,
            nc,
            synchronizer,
        }
    }

    pub fn execute(self) -> Status {
        let block_hashes = self.message.block_hashes();
        // use MAX_HEADERS_LEN as limit, we may increase the value of MAX_BLOCKS_IN_TRANSIT_PER_PEER in the future
        if block_hashes.len() > MAX_HEADERS_LEN {
            return StatusCode::MalformedProtocolMessage.with_context(format!(
                "BlockHashes count({}) > MAX_HEADERS_LEN({})",
                block_hashes.len(),
                MAX_HEADERS_LEN,
            ));
        }
        let snapshot = self.synchronizer.shared.snapshot();

        for block_hash in block_hashes.iter().take(MAX_BLOCKS_IN_TRANSIT_PER_PEER) {
            debug!("get_blocks {} from peer {:?}", block_hash, self.peer);
            let block_hash = block_hash.to_entity();

            if !snapshot.contains_block_status(&block_hash, BlockStatus::BLOCK_VALID) {
                debug!(
                    "ignoring get_block {} request from peer={} for unverified",
                    block_hash, self.peer
                );
                continue;
            }

            if self.nc.send_paused() {
                debug!(
                    "Session send buffer is full, stop send blocks to peer {:?}",
                    self.peer
                );
                break;
            }

            if let Some(block) = snapshot.get_block(&block_hash) {
                debug!(
                    "respond_block {} {} to peer {:?}",
                    block.number(),
                    block.hash(),
                    self.peer,
                );
                let content = packed::SendBlock::new_builder().block(block.data()).build();
                let message = packed::SyncMessage::new_builder().set(content).build();
                let data = message.as_slice().into();
                if let Err(err) = self.nc.send_message_to(self.peer, data) {
                    return StatusCode::Network
                        .with_context(format!("Send SendBlock error: {:?}", err));
                }
            } else {
                // TODO response not found
                // TODO add timeout check in synchronizer

                // We expect that `block_hashes` is sorted descending by height.
                // So if we cannot find the current one from local, we cannot find
                // the next either.
                debug!("getblocks stopping since {} is not found", block_hash);
                break;
            }
        }

        Status::ok()
    }
}
