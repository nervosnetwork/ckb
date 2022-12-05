use crate::mock_sync::mock_synchronizer::MockSynchronizer;
use ckb_constant::sync::{INIT_BLOCKS_IN_TRANSIT_PER_PEER, MAX_HEADERS_LEN};
use ckb_logger::debug;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_sync::{attempt, block_status::BlockStatus, utils::send_message, Status, StatusCode};
use ckb_types::{packed, prelude::*};
use std::collections::HashSet;

pub struct GetBlocksProcess<'a> {
    message: packed::GetBlocksReader<'a>,
    synchronizer: &'a MockSynchronizer,
    nc: &'a dyn CKBProtocolContext,
    peer: PeerIndex,
}

impl<'a> GetBlocksProcess<'a> {
    pub fn new(
        message: packed::GetBlocksReader<'a>,
        synchronizer: &'a MockSynchronizer,
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
        // use MAX_HEADERS_LEN as limit, we may increase the value of INIT_BLOCKS_IN_TRANSIT_PER_PEER in the future
        if block_hashes.len() > MAX_HEADERS_LEN {
            return StatusCode::ProtocolMessageIsMalformed.with_context(format!(
                "BlockHashes count({}) > MAX_HEADERS_LEN({})",
                block_hashes.len(),
                MAX_HEADERS_LEN,
            ));
        }
        let active_chain = self.synchronizer.shared.active_chain();

        let iter = block_hashes.iter().take(INIT_BLOCKS_IN_TRANSIT_PER_PEER);

        let mut dedup = HashSet::new();
        for block_hash in iter {
            debug!("get_blocks {} from peer {:?}", block_hash, self.peer);
            let block_hash = block_hash.to_entity();

            if block_hash == self.synchronizer.shared().consensus().genesis_hash() {
                return StatusCode::RequestGenesis.with_context("Request genesis block");
            }

            if !dedup.insert(block_hash.clone()) {
                return StatusCode::RequestDuplicate.with_context("Request duplicate block");
            }

            if !active_chain.contains_block_status(&block_hash, BlockStatus::BLOCK_VALID) {
                debug!(
                    "ignoring get_block {} request from peer={} for unverified",
                    block_hash, self.peer
                );
                continue;
            }

            if let Some(block) = active_chain.get_block(&block_hash) {
                debug!(
                    "respond_block {} {} to peer {:?}",
                    block.number(),
                    block.hash(),
                    self.peer,
                );
                let content = packed::SendBlock::new_builder().block(block.data()).build();
                let message = packed::SyncMessage::new_builder().set(content).build();

                attempt!(send_message(
                    self.nc.protocol_id(),
                    self.nc,
                    self.peer,
                    &message
                ));
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
