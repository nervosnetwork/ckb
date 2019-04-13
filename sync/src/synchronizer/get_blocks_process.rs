use crate::synchronizer::Synchronizer;
use ckb_network::{CKBProtocolContext, SessionId};
use ckb_protocol::{cast, GetBlocks, SyncMessage};
use ckb_shared::store::ChainStore;
use failure::Error as FailureError;
use flatbuffers::FlatBufferBuilder;
use log::{debug, warn};
use std::convert::TryInto;

pub struct GetBlocksProcess<'a, CS: ChainStore + 'a> {
    message: &'a GetBlocks<'a>,
    synchronizer: &'a Synchronizer<CS>,
    nc: &'a mut CKBProtocolContext,
    peer: SessionId,
}

impl<'a, CS> GetBlocksProcess<'a, CS>
where
    CS: ChainStore + 'a,
{
    pub fn new(
        message: &'a GetBlocks,
        synchronizer: &'a Synchronizer<CS>,
        peer: SessionId,
        nc: &'a mut CKBProtocolContext,
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

        for fbs_h256 in block_hashes {
            let block_hash = fbs_h256.try_into()?;
            debug!(target: "sync", "get_blocks {:x}", block_hash);
            if let Some(block) = self.synchronizer.get_block(&block_hash) {
                debug!(target: "sync", "respond_block {} {:x}", block.header().number(), block.header().hash());
                let fbb = &mut FlatBufferBuilder::new();
                let message = SyncMessage::build_block(fbb, &block);
                fbb.finish(message, None);
                let ret = self.nc.send(self.peer, fbb.finished_data().to_vec());
                if ret.is_err() {
                    warn!(target: "relay", "response GetBlocks error {:?}", ret);
                }
            } else {
                // TODO response not found
                // TODO add timeout check in synchronizer
            }
        }

        Ok(())
    }
}
