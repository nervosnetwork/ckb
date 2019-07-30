use crate::relayer::{compact_block::GetBlockProposal, Relayer};
use ckb_core::transaction::{ProposalShortId, Transaction};
use ckb_logger::debug_target;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{GetBlockProposal as GetBlockProposalMessage, RelayMessage};
use failure::Error as FailureError;
use flatbuffers::FlatBufferBuilder;
use std::convert::TryInto;
use std::sync::Arc;

pub struct GetBlockProposalProcess<'a> {
    message: &'a GetBlockProposalMessage<'a>,
    relayer: &'a Relayer,
    nc: Arc<dyn CKBProtocolContext>,
    peer: PeerIndex,
}

impl<'a> GetBlockProposalProcess<'a> {
    pub fn new(
        message: &'a GetBlockProposalMessage,
        relayer: &'a Relayer,
        nc: Arc<dyn CKBProtocolContext>,
        peer: PeerIndex,
    ) -> Self {
        GetBlockProposalProcess {
            message,
            nc,
            relayer,
            peer,
        }
    }

    pub fn execute(self) -> Result<(), FailureError> {
        let get_block_proposal: GetBlockProposal = (*self.message).try_into()?;
        let proposals = get_block_proposal.proposals;
        let proposals_transactions: Vec<Option<Transaction>> = {
            let chain_state = self.relayer.shared.lock_chain_state();
            proposals
                .iter()
                .map(|short_id| chain_state.get_tx_from_pool_or_store(short_id))
                .collect()
        };
        let fresh_proposals: Vec<ProposalShortId> = proposals
            .into_iter()
            .enumerate()
            .filter_map(|(index, short_id)| {
                if proposals_transactions[index].is_none() {
                    Some(short_id)
                } else {
                    None
                }
            })
            .collect();
        let transactions: Vec<Transaction> = proposals_transactions
            .into_iter()
            .filter_map(|pt| pt)
            .collect();

        self.relayer
            .shared()
            .insert_get_block_proposals(self.peer, fresh_proposals);

        let fbb = &mut FlatBufferBuilder::new();
        let message = RelayMessage::build_block_proposal(fbb, &transactions);
        fbb.finish(message, None);

        if let Err(err) = self
            .nc
            .send_message_to(self.peer, fbb.finished_data().into())
        {
            debug_target!(
                crate::LOG_TARGET_RELAY,
                "relayer send GetBlockProposal error: {:?}",
                err
            );
        }
        Ok(())
    }
}
