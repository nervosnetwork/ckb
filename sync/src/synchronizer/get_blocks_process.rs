use crate::synchronizer::Synchronizer;
use crate::MAX_BLOCKS_IN_TRANSIT_PER_PEER;
use ckb_logger::{debug, warn};
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{cast, GetBlocks, SyncMessage};
use ckb_store::ChainStore;
use failure::Error as FailureError;
use flatbuffers::FlatBufferBuilder;
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

        let n_limit = min(MAX_BLOCKS_IN_TRANSIT_PER_PEER as usize, block_hashes.len());
        for fbs_h256 in block_hashes.iter().take(n_limit) {
            let block_hash = fbs_h256.try_into()?;
            debug!("get_blocks {:x} from peer {:?}", block_hash, self.peer);
            if let Some(block) = self.synchronizer.shared.store().get_block(&block_hash) {
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
                    debug!("synchronizer send Block error: {:?}", err);
                    break;
                }
            } else {
                // TODO response not found
                // TODO add timeout check in synchronizer

                // We expect that `block_hashes` is sorted descending by height.
                // So if we cannot find the current one from local, we cannot find
                // the next either.
                debug!("getblocks stopping since {:x} is not found", block_hash);
                break;
            }
        }

        if n_limit < block_hashes.len() {
            warn!("getblocks stopping at limit {}", n_limit);
        }

        Ok(())
    }
}
