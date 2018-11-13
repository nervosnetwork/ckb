use super::compact_block::CompactBlock;
use ckb_protocol::{CompactBlock as FbsCompactBlock, RelayMessage};
use ckb_shared::index::ChainIndex;
use flatbuffers::FlatBufferBuilder;
use network::{CKBProtocolContext, PeerIndex};
use relayer::Relayer;
use std::collections::HashSet;

pub struct CompactBlockProcess<'a, CI: ChainIndex + 'a> {
    message: &'a FbsCompactBlock<'a>,
    relayer: &'a Relayer<CI>,
    peer: PeerIndex,
    nc: &'a CKBProtocolContext,
}

impl<'a, CI> CompactBlockProcess<'a, CI>
where
    CI: ChainIndex + 'static,
{
    pub fn new(
        message: &'a FbsCompactBlock,
        relayer: &'a Relayer<CI>,
        peer: PeerIndex,
        nc: &'a CKBProtocolContext,
    ) -> Self {
        CompactBlockProcess {
            message,
            nc,
            relayer,
            peer,
        }
    }

    pub fn execute(self) {
        let compact_block: CompactBlock = (*self.message).into();

        if self
            .relayer
            .state
            .received_blocks
            .lock()
            .insert(compact_block.header.hash())
        {
            self.relayer
                .request_proposal_txs(self.nc, self.peer, &compact_block);

            match self.relayer.reconstruct_block(&compact_block, Vec::new()) {
                (Some(block), _) => {
                    if self.relayer.accept_block(self.peer, block.clone()).is_ok() {
                        let fbb = &mut FlatBufferBuilder::new();
                        let message =
                            RelayMessage::build_compact_block(fbb, &block, &HashSet::new());
                        fbb.finish(message, None);

                        for peer_id in self.nc.connected_peers() {
                            if peer_id != self.peer {
                                let _ = self.nc.send(peer_id, fbb.finished_data().to_vec());
                            }
                        }
                    }
                }
                (_, Some(missing_indexes)) => {
                    let hash = compact_block.header.hash();
                    self.relayer
                        .state
                        .pending_compact_blocks
                        .lock()
                        .insert(hash, compact_block);

                    let fbb = &mut FlatBufferBuilder::new();
                    let message = RelayMessage::build_get_block_transactions(
                        fbb,
                        &hash,
                        &missing_indexes
                            .into_iter()
                            .map(|i| i as u32)
                            .collect::<Vec<_>>(),
                    );
                    fbb.finish(message, None);
                    let _ = self.nc.send(self.peer, fbb.finished_data().to_vec());
                }
                (None, None) => {
                    // TODO fail to reconstruct block, downgrade to header first?
                }
            }
        }
    }
}
