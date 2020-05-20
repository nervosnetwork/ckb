use crate::relayer::Relayer;
use crate::utils::send_blockproposal;
use crate::{Status, StatusCode};
use ckb_logger::debug_target;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{packed, prelude::*};
use std::sync::Arc;

pub struct GetBlockProposalProcess<'a> {
    get_block_proposal: packed::GetBlockProposal,
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
        let get_block_proposal = message.to_entity();
        GetBlockProposalProcess {
            get_block_proposal,
            nc,
            relayer,
            peer,
        }
    }

    pub fn execute(self) -> Status {
        let shared = self.relayer.shared();
        {
            let get_block_proposal = &self.get_block_proposal;
            let limit = shared.consensus().max_block_proposals_limit()
                * (shared.consensus().max_uncles_num() as u64);
            if (get_block_proposal.proposals().len() as u64) > limit {
                return StatusCode::ProtocolMessageIsMalformed.with_context(format!(
                    "GetBlockProposal proposals count({}) > consensus max_block_proposals_limit({})",
                    get_block_proposal.proposals().len(), limit,
                ));
            }
        }

        let proposals: Vec<packed::ProposalShortId> =
            self.get_block_proposal.proposals().into_iter().collect();

        let fetched_transactions = {
            let tx_pool = self.relayer.shared.shared().tx_pool_controller();
            let fetch_txs = tx_pool.fetch_txs(proposals.clone());
            if let Err(e) = fetch_txs {
                debug_target!(
                    crate::LOG_TARGET_RELAY,
                    "relayer tx_pool_controller send fetch_txs error: {:?}",
                    e
                );
                return Status::ok();
            }
            fetch_txs.unwrap()
        };
        let fresh_proposals: Vec<packed::ProposalShortId> = proposals
            .into_iter()
            .filter(|short_id| fetched_transactions.get(&short_id).is_none())
            .collect();

        self.relayer
            .shared()
            .state()
            .insert_get_block_proposals(self.peer, fresh_proposals);

        if let Err(err) = send_blockproposal(
            self.nc.as_ref(),
            self.peer,
            fetched_transactions
                .into_iter()
                .map(|(_, tx)| tx.data())
                .collect(),
        ) {
            return StatusCode::Network
                .with_context(format!("send_blockproposal error: {:?}", err,));
        }
        Status::ok()
    }
}
