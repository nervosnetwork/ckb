use ckb_chain::chain::ChainProvider;
use ckb_chain::PowEngine;
use ckb_protocol::{
    build_transaction_args, BlockProposal, BlockProposalArgs, GetBlockProposal, RelayMessage,
    RelayMessageArgs, RelayPayload, Transaction,
};
use core::transaction::ProposalShortId;
use flatbuffers::FlatBufferBuilder;
use network::{NetworkContext, PeerId};
use relayer::Relayer;

pub struct GetBlockProposalProcess<'a, C: 'a, P: 'a> {
    message: &'a GetBlockProposal<'a>,
    relayer: &'a Relayer<C, P>,
    peer: PeerId,
    nc: &'a NetworkContext,
}

impl<'a, C, P> GetBlockProposalProcess<'a, C, P>
where
    C: ChainProvider + 'static,
    P: PowEngine + 'static,
{
    pub fn new(
        message: &'a GetBlockProposal,
        relayer: &'a Relayer<C, P>,
        peer: PeerId,
        nc: &'a NetworkContext,
    ) -> Self {
        GetBlockProposalProcess {
            message,
            nc,
            relayer,
            peer,
        }
    }

    pub fn execute(self) {
        let mut pending_proposals_request = self.relayer.state.pending_proposals_request.lock();

        let builder = &mut FlatBufferBuilder::new();
        {
            let vec = self
                .message
                .proposal_transactions()
                .unwrap()
                .chunks(10)
                .filter_map(|s| ProposalShortId::from_slice(s))
                .filter_map(|short_id| {
                    if let Some(transaction) = self.relayer.tx_pool.get(&short_id) {
                        let transaction_args = build_transaction_args(builder, &transaction);
                        Some(Transaction::create(builder, &transaction_args))
                    } else {
                        let mut peer_set = pending_proposals_request
                            .entry(short_id)
                            .or_insert_with(Default::default);
                        peer_set.insert(self.peer);
                        None
                    }
                }).collect::<Vec<_>>();

            let transactions = Some(builder.create_vector(&vec));

            let payload = Some(
                BlockProposal::create(builder, &BlockProposalArgs { transactions })
                    .as_union_value(),
            );
            let payload_type = RelayPayload::BlockProposal;
            let message = RelayMessage::create(
                builder,
                &RelayMessageArgs {
                    payload_type,
                    payload,
                },
            );
            builder.finish(message, None);
        }

        self.nc.respond(0, builder.finished_data().to_vec());
    }
}
