use super::compact_block::CompactBlock;
use ckb_chain::chain::ChainProvider;
use ckb_chain::PowEngine;
use ckb_protocol::{CompactBlock as FbsCompactBlock, RelayMessage};
use flatbuffers::FlatBufferBuilder;
use network::{NetworkContext, PeerId};
use relayer::Relayer;

pub struct CompactBlockProcess<'a, C: 'a, P: 'a> {
    message: &'a FbsCompactBlock<'a>,
    relayer: &'a Relayer<C, P>,
    peer: PeerId,
    nc: &'a NetworkContext,
}

impl<'a, C, P> CompactBlockProcess<'a, C, P>
where
    C: ChainProvider + 'static,
    P: PowEngine + 'static,
{
    pub fn new(
        message: &'a FbsCompactBlock,
        relayer: &'a Relayer<C, P>,
        peer: PeerId,
        nc: &'a NetworkContext,
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

        if !self
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
                    let _ = self.relayer.accept_block(self.peer, &block);
                    // TODO PENDING new api NetworkContext#connected_peers
                    // for peer_id in self.nc.connected_peers() {
                    //     let fbb = &mut FlatBufferBuilder::new();
                    //     let message = RelayMessage::build_compact_block(fbb, &block, &HashSet::new());
                    //     fbb.finish(message, None);
                    //     self.nc.send(peer_id, 0, fbb.finished_data().to_vec());
                    // }
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
                    self.nc.respond(0, fbb.finished_data().to_vec());
                }
                (None, None) => {
                    // TODO fail to reconstruct block, downgrade to header first?
                }
            }
        }
    }
}
