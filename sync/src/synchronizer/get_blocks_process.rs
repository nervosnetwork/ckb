use crate::synchronizer::Synchronizer;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{cast, GetBlocks, SyncMessage};
use ckb_shared::index::ChainIndex;
use ckb_util::TryInto;
use failure::Error as FailureError;
use flatbuffers::FlatBufferBuilder;
use log::{debug, warn};

pub struct GetBlocksProcess<'a, CI: ChainIndex + 'a> {
    message: &'a GetBlocks<'a>,
    synchronizer: &'a Synchronizer<CI>,
    nc: &'a mut CKBProtocolContext,
    peer: PeerIndex,
}

impl<'a, CI> GetBlocksProcess<'a, CI>
where
    CI: ChainIndex + 'a,
{
    pub fn new(
        message: &'a GetBlocks,
        synchronizer: &'a Synchronizer<CI>,
        peer: PeerIndex,
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
                if let Some(filter) = self
                    .synchronizer
                    .peers
                    .transaction_filters
                    .read()
                    .get(&self.peer)
                {
                    let transactions_index = block
                        .commit_transactions()
                        .iter()
                        .enumerate()
                        .filter(|(_index, tx)| filter.contains(tx))
                        .map(|ti| ti.0)
                        .collect::<Vec<_>>();

                    let fbb = &mut FlatBufferBuilder::new();
                    let message =
                        SyncMessage::build_filtered_block(fbb, &block, &transactions_index);
                    fbb.finish(message, None);
                    let ret = self.nc.send(self.peer, fbb.finished_data().to_vec());
                    if ret.is_err() {
                        warn!(target: "relay", "response GetBlocks error {:?}", ret);
                    }
                } else {
                    let fbb = &mut FlatBufferBuilder::new();
                    let message = SyncMessage::build_block(fbb, &block);
                    fbb.finish(message, None);
                    let ret = self.nc.send(self.peer, fbb.finished_data().to_vec());
                    if ret.is_err() {
                        warn!(target: "relay", "response GetBlocks error {:?}", ret);
                    }
                }
            } else {
                // TODO response not found
                // TODO add timeout check in synchronizer
            }
        }

        Ok(())
    }
}
