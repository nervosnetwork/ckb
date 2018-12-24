use crate::synchronizer::Synchronizer;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{FlatbuffersVectorIterator, GetBlocks, SyncMessage};
use ckb_shared::index::ChainIndex;
use flatbuffers::FlatBufferBuilder;
use log::debug;
use numext_fixed_hash::H256;

pub struct GetBlocksProcess<'a, CI: ChainIndex + 'a> {
    message: &'a GetBlocks<'a>,
    synchronizer: &'a Synchronizer<CI>,
    nc: &'a CKBProtocolContext,
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
        nc: &'a CKBProtocolContext,
    ) -> Self {
        GetBlocksProcess {
            peer,
            message,
            nc,
            synchronizer,
        }
    }

    pub fn execute(self) {
        FlatbuffersVectorIterator::new(self.message.block_hashes().unwrap()).for_each(|bytes| {
            let block_hash = H256::from_slice(bytes.seq().unwrap()).unwrap();
            debug!(target: "sync", "get_blocks {:?}", block_hash);
            if let Some(block) = self.synchronizer.get_block(&block_hash) {
                debug!(target: "sync", "respond_block {} {:?}", block.header().number(), block.header().hash());
                if let Some(filter) = self.synchronizer.peers.transaction_filters.read().get(&self.peer) {
                    let transactions_index = block.commit_transactions().iter().enumerate().filter(|(_index, tx)|
                        filter.contains(tx)
                    ).map(|ti| ti.0).collect::<Vec<_>>();

                    let fbb = &mut FlatBufferBuilder::new();
                    let message = SyncMessage::build_filtered_block(fbb, &block, &transactions_index);
                    fbb.finish(message, None);
                    let _ = self.nc.send(self.peer, fbb.finished_data().to_vec());
                } else {
                    let fbb = &mut FlatBufferBuilder::new();
                    let message = SyncMessage::build_block(fbb, &block);
                    fbb.finish(message, None);
                    let _ = self.nc.send(self.peer, fbb.finished_data().to_vec());
                }
            } else {
                // TODO response not found
                // TODO add timeout check in synchronizer
            }
        })
    }
}
