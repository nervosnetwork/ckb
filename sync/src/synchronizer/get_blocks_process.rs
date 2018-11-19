use bigint::H256;
use ckb_chain::chain::ChainProvider;
use ckb_chain::PowEngine;
use ckb_protocol::{build_block_args, Block, GetBlocks, SyncMessage, SyncMessageArgs, SyncPayload};
use flatbuffers::FlatBufferBuilder;
use network::{NetworkContext, PeerId};
use synchronizer::Synchronizer;

pub struct GetBlocksProcess<'a, C: 'a, P: 'a> {
    message: &'a GetBlocks<'a>,
    synchronizer: &'a Synchronizer<C, P>,
    nc: &'a NetworkContext,
}

impl<'a, C, P> GetBlocksProcess<'a, C, P>
where
    C: ChainProvider + 'a,
    P: PowEngine + 'a,
{
    pub fn new(
        message: &'a GetBlocks,
        synchronizer: &'a Synchronizer<C, P>,
        _peer: PeerId,
        nc: &'a NetworkContext,
    ) -> Self {
        GetBlocksProcess {
            message,
            nc,
            synchronizer,
        }
    }

    pub fn execute(self) {
        self.message
            .block_hashes()
            .unwrap()
            .chunks(32)
            .map(H256::from)
            .for_each(|block_hash| {
                debug!(target: "sync", "get_blocks {:?}", block_hash);
                if let Some(ref block) = self.synchronizer.get_block(&block_hash) {
                    debug!(target: "sync", "respond_block {} {:?}", block.number(), block.hash());

                    let builder = &mut FlatBufferBuilder::new();
                    {
                        let block_args = build_block_args(builder, block);
                        let payload = Some(Block::create(builder, &block_args).as_union_value());
                        let payload_type = SyncPayload::Block;
                        let message = SyncMessage::create(
                            builder,
                            &SyncMessageArgs {
                                payload_type,
                                payload,
                            },
                        );
                        builder.finish(message, None);
                    }

                    self.nc.respond(0, builder.finished_data().to_vec());
                } else {
                    // TODO response not found
                    // TODO add timeout check in synchronizer
                }
            })
    }
}
