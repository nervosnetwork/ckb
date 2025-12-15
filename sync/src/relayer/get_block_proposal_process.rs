use crate::relayer::{MAX_RELAY_TXS_BYTES_PER_BATCH, Relayer};
use crate::utils::async_send_message_to;
use crate::{Status, StatusCode, attempt};
use ckb_logger::debug_target;
use ckb_network::{CKBProtocolContext, PeerIndex};
use ckb_types::{packed, prelude::*};
use std::collections::HashSet;
use std::sync::Arc;

pub struct GetBlockProposalProcess<'a> {
    message: packed::GetBlockProposalReader<'a>,
    relayer: &'a Relayer,
    nc: Arc<dyn CKBProtocolContext + Sync>,
    peer: PeerIndex,
}

impl<'a> GetBlockProposalProcess<'a> {
    pub fn new(
        message: packed::GetBlockProposalReader<'a>,
        relayer: &'a Relayer,
        nc: Arc<dyn CKBProtocolContext + Sync>,
        peer: PeerIndex,
    ) -> Self {
        GetBlockProposalProcess {
            message,
            nc,
            relayer,
            peer,
        }
    }

    pub async fn execute(self) -> Status {
        let shared = self.relayer.shared();
        let message_len = self.message.proposals().len();
        {
            // The block proposal request is separate from uncles,
            // so here the limit is only used to calculate the maximum value of uncles
            let limit = shared.consensus().max_block_proposals_limit()
                * (shared.consensus().max_uncles_num() as u64);
            if message_len as u64 > limit {
                return StatusCode::ProtocolMessageIsMalformed.with_context(format!(
                    "GetBlockProposal proposals count({message_len}) > consensus max_block_proposals_limit({limit})"
                ));
            }
        }

        let proposals: HashSet<packed::ProposalShortId> =
            self.message.proposals().to_entity().into_iter().collect();

        if proposals.len() != message_len {
            return StatusCode::RequestDuplicate.with_context("Request duplicate proposal");
        }

        let fetched_transactions = {
            let tx_pool = self.relayer.shared.shared().tx_pool_controller();
            let fetch_txs = tx_pool.fetch_txs(proposals.clone()).await;
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
        // Transactions that do not exist on this node
        let not_exist_proposals: Vec<packed::ProposalShortId> = proposals
            .into_iter()
            .filter(|short_id| !fetched_transactions.contains_key(short_id))
            .collect();

        // Cache request, try process on timer
        self.relayer
            .shared()
            .state()
            .insert_get_block_proposals(self.peer, not_exist_proposals);

        let mut relay_bytes = 0;
        let mut relay_proposals = Vec::new();
        for (_, tx) in fetched_transactions {
            let data = tx.data();
            let tx_size = data.total_size();
            if relay_bytes + tx_size > MAX_RELAY_TXS_BYTES_PER_BATCH {
                self.send_block_proposals(std::mem::take(&mut relay_proposals))
                    .await;
                relay_bytes = tx_size;
            } else {
                relay_bytes += tx_size;
            }
            relay_proposals.push(data);
        }
        if !relay_proposals.is_empty() {
            attempt!(self.send_block_proposals(relay_proposals).await);
        }
        Status::ok()
    }

    async fn send_block_proposals(&self, txs: Vec<packed::Transaction>) -> Status {
        let content = packed::BlockProposal::new_builder()
            .transactions(txs)
            .build();
        let message = packed::RelayMessage::new_builder().set(content).build();
        async_send_message_to(&self.nc, self.peer, &message).await
    }
}
