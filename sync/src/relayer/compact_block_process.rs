use super::compact_block::CompactBlock;
use ckb_chain::chain::ChainProvider;
use ckb_chain::PowEngine;
use ckb_protocol::{
    CompactBlock as FbsCompactBlock, GetBlockTransactions, GetBlockTransactionsArgs, RelayMessage,
    RelayMessageArgs, RelayPayload,
};
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
                    //     let compact_block = CompactBlockBuilder::new(block, &HashSet::new()).build();
                    //     self.nc.send(peer_id, 0, compact_block.to_vec());
                    // }
                }
                (_, Some(missing_indexes)) => {
                    let builder = &mut FlatBufferBuilder::new();
                    {
                        let hash = Some(builder.create_vector(&compact_block.header.hash()));
                        let indexes = Some(
                            builder.create_vector(
                                &missing_indexes
                                    .into_iter()
                                    .map(|i| i as u32)
                                    .collect::<Vec<u32>>(),
                            ),
                        );
                        let payload = Some(
                            GetBlockTransactions::create(
                                builder,
                                &GetBlockTransactionsArgs { hash, indexes },
                            ).as_union_value(),
                        );
                        let payload_type = RelayPayload::GetBlockTransactions;
                        let message = RelayMessage::create(
                            builder,
                            &RelayMessageArgs {
                                payload_type,
                                payload,
                            },
                        );
                        builder.finish(message, None);
                    }

                    self.relayer
                        .state
                        .pending_compact_blocks
                        .lock()
                        .insert(compact_block.header.hash(), compact_block);

                    self.nc.respond(0, builder.finished_data().to_vec());
                }
                (None, None) => {
                    // TODO fail to reconstruct block, downgrade to header first?
                }
            }
        }
    }
}
