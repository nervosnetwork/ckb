use crate::relayer::Relayer;
use ckb_logger::debug_target;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{core, packed, prelude::*};
use failure::Error as FailureError;
use std::sync::Arc;

pub struct GetBlockProposalProcess<'a> {
    message: packed::GetBlockProposalReader<'a>,
    relayer: &'a Relayer,
    nc: Arc<dyn CKBProtocolContext>,
    peer: PeerIndex,
}

impl<'a> GetBlockProposalProcess<'a> {
    pub fn new(
        message: packed::GetBlockProposalReader<'a>,
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
        let proposals: Vec<packed::ProposalShortId> =
            self.message.proposals().to_entity().into_iter().collect();
        let proposals_transactions: Vec<Option<core::TransactionView>> = {
            let chain_state = self.relayer.shared.lock_chain_state();
            proposals
                .iter()
                .map(|short_id| chain_state.get_tx_from_pool_or_store(short_id))
                .collect()
        };
        let fresh_proposals: Vec<packed::ProposalShortId> = proposals
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
        let transactions: Vec<packed::Transaction> = proposals_transactions
            .into_iter()
            .filter_map(|pt| pt.map(|x| x.data()))
            .collect();

        self.relayer
            .shared()
            .insert_get_block_proposals(self.peer, fresh_proposals);

        let content = packed::BlockProposal::new_builder()
            .transactions(transactions.pack())
            .build();
        let message = packed::RelayMessage::new_builder().set(content).build();
        let data = message.as_slice().into();

        if let Err(err) = self.nc.send_message_to(self.peer, data) {
            debug_target!(
                crate::LOG_TARGET_RELAY,
                "relayer send GetBlockProposal error: {:?}",
                err
            );
        }
        Ok(())
    }
}
