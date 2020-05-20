use crate::block_status::BlockStatus;
use crate::synchronizer::Synchronizer;
use crate::utils::send_sendblock;
use crate::{Status, StatusCode, INIT_BLOCKS_IN_TRANSIT_PER_PEER, MAX_HEADERS_LEN};
use ckb_logger::debug;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::packed::Byte32Vec;
use ckb_types::{packed, prelude::*};

pub struct GetBlocksProcess<'a> {
    block_hashes: Byte32Vec,
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
        let block_hashes = message.block_hashes().to_entity();
        GetBlocksProcess {
            block_hashes,
            peer,
            nc,
            synchronizer,
        }
    }

    pub fn execute(self) -> Status {
        // use MAX_HEADERS_LEN as limit, we may increase the value of INIT_BLOCKS_IN_TRANSIT_PER_PEER in the future
        if self.block_hashes.len() > MAX_HEADERS_LEN {
            return StatusCode::ProtocolMessageIsMalformed.with_context(format!(
                "BlockHashes count({}) > MAX_HEADERS_LEN({})",
                self.block_hashes.len(),
                MAX_HEADERS_LEN,
            ));
        }
        let active_chain = self.synchronizer.shared.active_chain();

        for block_hash in self
            .block_hashes
            .into_iter()
            .take(INIT_BLOCKS_IN_TRANSIT_PER_PEER)
        {
            debug!("get_blocks {} from peer {:?}", block_hash, self.peer);
            if !active_chain.contains_block_status(&block_hash, BlockStatus::BLOCK_VALID) {
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

            if let Some(block) = active_chain.get_block(&block_hash) {
                if let Err(err) = send_sendblock(self.nc, self.peer, block) {
                    return StatusCode::Network
                        .with_context(format!("send_sendblock error: {:?}", err));
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
