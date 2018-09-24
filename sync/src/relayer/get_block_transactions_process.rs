use bigint::H256;
use ckb_chain::chain::ChainProvider;
use ckb_chain::PowEngine;
use ckb_protocol::{
    build_transaction_args, BlockTransactions, BlockTransactionsArgs, GetBlockTransactions,
    RelayMessage, RelayMessageArgs, RelayPayload, Transaction,
};
use flatbuffers::FlatBufferBuilder;
use network::{NetworkContext, PeerId};
use relayer::Relayer;

pub struct GetBlockTransactionsProcess<'a, C: 'a, P: 'a> {
    message: &'a GetBlockTransactions<'a>,
    relayer: &'a Relayer<C, P>,
    peer: PeerId,
    nc: &'a NetworkContext,
}

impl<'a, C, P> GetBlockTransactionsProcess<'a, C, P>
where
    C: ChainProvider + 'a,
    P: PowEngine + 'a,
{
    pub fn new(
        message: &'a GetBlockTransactions,
        relayer: &'a Relayer<C, P>,
        peer: PeerId,
        nc: &'a NetworkContext,
    ) -> Self {
        GetBlockTransactionsProcess {
            message,
            nc,
            peer,
            relayer,
        }
    }

    pub fn execute(self) {
        let hash = H256::from_slice(self.message.hash().unwrap());
        debug!(target: "relay", "get_block_transactions {:?}", hash);

        if let Some(block) = self.relayer.get_block(&hash) {
            let builder = &mut FlatBufferBuilder::new();
            {
                let vec = self
                    .message
                    .indexes()
                    .unwrap()
                    .safe_slice()
                    .iter()
                    .filter_map(|i| block.commit_transactions.get(*i as usize))
                    .map(|transaction| {
                        let transaction_args = build_transaction_args(builder, transaction);
                        Transaction::create(builder, &transaction_args)
                    }).collect::<Vec<_>>();
                let transactions = Some(builder.create_vector(&vec));
                let hash = Some(builder.create_vector(&hash));
                let payload = Some(
                    BlockTransactions::create(
                        builder,
                        &BlockTransactionsArgs { hash, transactions },
                    ).as_union_value(),
                );

                let payload_type = RelayPayload::BlockTransactions;
                let message = RelayMessage::create(
                    builder,
                    &RelayMessageArgs {
                        payload_type,
                        payload,
                    },
                );
                builder.finish(message, None);
            }

            self.nc.send(self.peer, 0, builder.finished_data().to_vec());
        }
    }
}
