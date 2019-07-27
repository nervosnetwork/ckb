use crate::relayer::Relayer;
use ckb_core::transaction::{ProposalShortId, Transaction};
use ckb_logger::debug_target;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_protocol::{cast, GetBlockProposal, RelayMessage};
use ckb_store::ChainStore;
use failure::Error as FailureError;
use flatbuffers::FlatBufferBuilder;
use std::convert::TryInto;
use std::sync::Arc;

pub struct GetBlockProposalProcess<'a, CS> {
    message: &'a GetBlockProposal<'a>,
    relayer: &'a Relayer<CS>,
    nc: Arc<dyn CKBProtocolContext>,
    peer: PeerIndex,
}

impl<'a, CS: ChainStore + 'static> GetBlockProposalProcess<'a, CS> {
    pub fn new(
        message: &'a GetBlockProposal,
        relayer: &'a Relayer<CS>,
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
        let proposals: Vec<ProposalShortId> = {
            let proposals = cast!(self.message.proposals())?;
            proposals
                .iter()
                .map(TryInto::try_into)
                .collect::<Result<_, FailureError>>()?
        };
        let pool_transactions: Vec<Option<Transaction>> = {
            let chain_state = self.relayer.shared.lock_chain_state();
            let tx_pool = chain_state.tx_pool();
            proposals
                .iter()
                .map(|short_id| tx_pool.get_tx(short_id))
                .collect()
        };
        let fresh_proposals: Vec<ProposalShortId> = proposals
            .into_iter()
            .enumerate()
            .filter_map(|(index, short_id)| {
                if pool_transactions[index].is_none() {
                    Some(short_id)
                } else {
                    None
                }
            })
            .collect();
        let transactions: Vec<Transaction> = pool_transactions
            .into_iter()
            .filter_map(|pool_transaction| pool_transaction)
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
