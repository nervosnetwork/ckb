use crate::relayer::Relayer;
use ckb_network::{CKBProtocolContext, SessionId};
use ckb_protocol::{cast, GetBlockProposal, RelayMessage};
use ckb_shared::store::ChainStore;
use failure::Error as FailureError;
use flatbuffers::FlatBufferBuilder;
use log::warn;
use std::convert::TryInto;

pub struct GetBlockProposalProcess<'a, CS> {
    message: &'a GetBlockProposal<'a>,
    relayer: &'a Relayer<CS>,
    peer: SessionId,
    nc: &'a mut CKBProtocolContext,
}

impl<'a, CS: ChainStore> GetBlockProposalProcess<'a, CS> {
    pub fn new(
        message: &'a GetBlockProposal,
        relayer: &'a Relayer<CS>,
        peer: SessionId,
        nc: &'a mut CKBProtocolContext,
    ) -> Self {
        GetBlockProposalProcess {
            message,
            nc,
            relayer,
            peer,
        }
    }

    pub fn execute(self) -> Result<(), FailureError> {
        let mut pending_proposals_request = self.relayer.state.pending_proposals_request.lock();
        let proposal_transactions = cast!(self.message.proposal_transactions())?;

        let transactions = {
            let chain_state = self.relayer.shared.chain_state().lock();
            let tx_pool = chain_state.tx_pool();

            let proposals = proposal_transactions
                .iter()
                .map(TryInto::try_into)
                .collect::<Result<Vec<_>, FailureError>>();

            proposals?
                .into_iter()
                .filter_map(|short_id| {
                    tx_pool.get_tx(&short_id).or({
                        pending_proposals_request
                            .entry(short_id)
                            .or_insert_with(Default::default)
                            .insert(self.peer);
                        None
                    })
                })
                .collect::<Vec<_>>()
        };

        let fbb = &mut FlatBufferBuilder::new();
        let message = RelayMessage::build_block_proposal(fbb, &transactions);
        fbb.finish(message, None);

        let ret = self.nc.send(self.peer, fbb.finished_data().to_vec());
        if ret.is_err() {
            warn!(target: "relay", "GetBlockProposalProcess response error {:?}", ret);
        }
        Ok(())
    }
}
