use crate::block_status::BlockStatus;
use crate::synchronizer::Synchronizer;
use crate::{attempt, Status, StatusCode, MAX_BLOCKS_IN_TRANSIT_PER_PEER};
use ckb_logger::debug;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{cast, GetBlocks, SyncMessage};
use ckb_store::ChainStore;
use flatbuffers::FlatBufferBuilder;
use numext_fixed_hash::H256;
use std::cmp::min;
use std::convert::TryInto;

pub struct GetBlocksProcess<'a> {
    message: &'a GetBlocks<'a>,
    synchronizer: &'a Synchronizer,
    nc: &'a CKBProtocolContext,
    peer: PeerIndex,
}

impl<'a> GetBlocksProcess<'a> {
    pub fn new(
        message: &'a GetBlocks,
        synchronizer: &'a Synchronizer,
        peer: PeerIndex,
        nc: &'a CKBProtocolContext,
    ) -> Self {
        GetBlocksProcess {
            peer,
            message,
            nc,
            synchronizer,
        }
    }

    pub fn execute(self) -> Status {
        let block_hashes = attempt!(cast!(self.message.block_hashes()));
        let store = self.synchronizer.shared.store();

        let n_limit = min(MAX_BLOCKS_IN_TRANSIT_PER_PEER as usize, block_hashes.len());
        for fbs_h256 in block_hashes.iter().take(n_limit) {
            let block_hash = attempt!(TryInto::<H256>::try_into(fbs_h256));
            debug!("get_blocks {:x} from peer {:?}", block_hash, self.peer);

            if !self
                .synchronizer
                .shared()
                .contains_block_status(&block_hash, BlockStatus::BLOCK_VALID)
            {
                debug!(
                    "ignoring get_block {:x} request from peer={} for unverified",
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

            if let Some(block) = store.get_block(&block_hash) {
                debug!(
                    "respond_block {} {:x} to peer {:?}",
                    block.header().number(),
                    block.header().hash(),
                    self.peer,
                );
                let fbb = &mut FlatBufferBuilder::new();
                let message = SyncMessage::build_block(fbb, &block);
                fbb.finish(message, None);
                if let Err(err) = self
                    .nc
                    .send_message_to(self.peer, fbb.finished_data().into())
                {
                    return StatusCode::Network
                        .with_context(format!("send Block error: {:?}", err,));
                }
            } else {
                // TODO response not found
                // TODO add timeout check in synchronizer
                // We expect that `block_hashes` is sorted descending by height.
                // So if we cannot find the current one from local, we cannot find
                // the next either.
                return StatusCode::MissingBlocks.with_context(format!(
                    "receive GetBlocks but cannot find the specific blocks in local store {:#x}",
                    block_hash,
                ));
            }
        }

        if n_limit < block_hashes.len() {
            return StatusCode::TooLengthyGetBlocks.with_context(format!(
                "receive too lengthy GetBlocks, max: {}, actual: {}",
                n_limit,
                block_hashes.len(),
            ));
        }

        Status::ok()
    }
}
