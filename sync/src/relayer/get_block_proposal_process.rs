use ckb_core::transaction::ProposalShortId;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{FlatbuffersVectorIterator, GetBlockProposal, RelayMessage};
use ckb_shared::index::ChainIndex;
use flatbuffers::FlatBufferBuilder;
use relayer::Relayer;

pub struct GetBlockProposalProcess<'a, CI: ChainIndex + 'a> {
    message: &'a GetBlockProposal<'a>,
    relayer: &'a Relayer<CI>,
    peer: PeerIndex,
    nc: &'a CKBProtocolContext,
}

impl<'a, CI> GetBlockProposalProcess<'a, CI>
where
    CI: ChainIndex + 'static,
{
    pub fn new(
        message: &'a GetBlockProposal,
        relayer: &'a Relayer<CI>,
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
                .filter_map(|short_id| {
                    self.relayer.tx_pool.get_transaction(short_id).or({
                        pending_proposals_request
                            .entry(short_id)
                            .or_insert_with(Default::default)
                            .insert(self.peer);
                        None
                    })
                }).map(Into::into)
                .collect::<Vec<_>>()
        };

        let fbb = &mut FlatBufferBuilder::new();
        let message = RelayMessage::build_block_proposal(fbb, &transactions);
        fbb.finish(message, None);

        let _ = self.nc.send(self.peer, fbb.finished_data().to_vec());
    }
}
