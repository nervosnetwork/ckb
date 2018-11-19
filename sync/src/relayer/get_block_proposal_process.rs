use ckb_chain::chain::ChainProvider;
use ckb_protocol::{FlatbuffersVectorIterator, GetBlockProposal, RelayMessage};
use core::transaction::ProposalShortId;
use flatbuffers::FlatBufferBuilder;
use network::{CKBProtocolContext, PeerIndex};
use relayer::Relayer;

pub struct GetBlockProposalProcess<'a, C: 'a> {
    message: &'a GetBlockProposal<'a>,
    relayer: &'a Relayer<C>,
    peer: PeerIndex,
    nc: &'a CKBProtocolContext,
}

impl<'a, C> GetBlockProposalProcess<'a, C>
where
    C: ChainProvider + 'static,
{
    pub fn new(
        message: &'a GetBlockProposal,
        relayer: &'a Relayer<C>,
        peer: PeerIndex,
        nc: &'a CKBProtocolContext,
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

        let transactions = {
            FlatbuffersVectorIterator::new(self.message.proposal_transactions().unwrap())
                .filter_map(|bytes| ProposalShortId::from_slice(bytes.seq().unwrap()))
                .filter_map({
                    |short_id| {
                        self.relayer.tx_pool.get(&short_id).or({
                            pending_proposals_request
                                .entry(short_id)
                                .or_insert_with(Default::default)
                                .insert(self.peer);
                            None
                        })
                    }
                }).map(Into::into)
                .collect::<Vec<_>>()
        };

        let fbb = &mut FlatBufferBuilder::new();
        let message = RelayMessage::build_block_proposal(fbb, &transactions);
        fbb.finish(message, None);

        let _ = self.nc.send(self.peer, fbb.finished_data().to_vec());
    }
}
