use crate::synchronizer::Synchronizer;
use crate::BLOCK_DOWNLOAD_WINDOW;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{cast, GetBlocks, SyncMessage};
use ckb_store::ChainStore;
use failure::Error as FailureError;
use flatbuffers::FlatBufferBuilder;
use log::{debug, warn};
use std::cmp::min;
use std::convert::TryInto;

pub struct GetBlocksProcess<'a, CS: ChainStore + 'a> {
    message: &'a GetBlocks<'a>,
    synchronizer: &'a Synchronizer<CS>,
    nc: &'a CKBProtocolContext,
    peer: PeerIndex,
}

impl<'a, CS> GetBlocksProcess<'a, CS>
where
    CS: ChainStore + 'a,
{
    pub fn new(
        message: &'a GetBlocks,
        synchronizer: &'a Synchronizer<CS>,
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

    pub fn execute(self) -> Result<(), FailureError> {
        let block_hashes = cast!(self.message.block_hashes())?;

        // bitcoin limits 500
        let n_limit = min(BLOCK_DOWNLOAD_WINDOW as usize, block_hashes.len());
        for fbs_h256 in block_hashes.iter().take(n_limit) {
            let block_hash = fbs_h256.try_into()?;
            debug!(target: "sync", "get_blocks {:x}", block_hash);
            if let Some(block) = self.synchronizer.shared.get_block(&block_hash) {
                debug!(target: "sync", "respond_block {} {:x}", block.header().number(), block.header().hash());
                let fbb = &mut FlatBufferBuilder::new();
                let message = SyncMessage::build_block(fbb, &block);
                fbb.finish(message, None);
                self.nc
                    .send_message_to(self.peer, fbb.finished_data().into());
            } else {
                // TODO response not found
                // TODO add timeout check in synchronizer

                // We expect that `block_hashes` is sorted descending by height.
                // So if we cannot find the current one from local, we cannot find
                // the next either.
                debug!(target: "sync", "getblocks stopping since {:x} is not found", block_hash);
                break;
            }
        }

        if n_limit < block_hashes.len() {
            warn!(target: "sync", "getblocks stopping at limit {}", n_limit);
        }

        Ok(())
    }
}
