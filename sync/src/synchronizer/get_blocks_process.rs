use crate::block_status::BlockStatus;
use crate::synchronizer::Synchronizer;
use crate::types::Version;
use crate::utils::send_message_to;
use crate::{attempt, Status, StatusCode};
use ckb_constant::sync::{
    INIT_BLOCKS_IN_TRANSIT_PER_PEER, MAX_HEADERS_LEN, NEW_INIT_BLOCKS_IN_TRANSIT_PER_PEER,
};
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
        // use MAX_HEADERS_LEN as limit, we may increase the value of INIT_BLOCKS_IN_TRANSIT_PER_PEER in the future
        if block_hashes.len() > MAX_HEADERS_LEN {
            return StatusCode::ProtocolMessageIsMalformed.with_context(format!(
                "BlockHashes count({}) > MAX_HEADERS_LEN({})",
                block_hashes.len(),
                MAX_HEADERS_LEN,
            ));
        }
        let active_chain = self.synchronizer.shared.active_chain();

        let iter = match active_chain
            .shared()
            .state()
            .peers()
            .get_version(self.peer)
            .unwrap_or(Version::New)
        {
            Version::Old => block_hashes.iter().take(INIT_BLOCKS_IN_TRANSIT_PER_PEER),
            Version::New => block_hashes
                .iter()
                .take(NEW_INIT_BLOCKS_IN_TRANSIT_PER_PEER),
        };
        for block_hash in iter {
            debug!("get_blocks {} from peer {:?}", block_hash, self.peer);
            let block_hash = block_hash.to_entity();

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

                attempt!(send_message_to(self.nc, self.peer, &message));
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
