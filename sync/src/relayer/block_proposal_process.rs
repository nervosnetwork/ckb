use crate::relayer::Relayer;
use crate::{Status, StatusCode};
use ckb_logger::warn_target;
use ckb_types::{core, packed, prelude::*};

pub struct BlockProposalProcess<'a> {
    message: packed::BlockProposalReader<'a>,
    relayer: &'a Relayer,
}

impl<'a> BlockProposalProcess<'a> {
    pub fn new(message: packed::BlockProposalReader<'a>, relayer: &'a Relayer) -> Self {
        BlockProposalProcess { message, relayer }
    }

    pub fn execute(self) -> Status {
        let shared = self.relayer.shared();
        let sync_state = shared.state();
        {
            let block_proposals = self.message;
            let limit = shared.consensus().max_block_proposals_limit()
                * (shared.consensus().max_uncles_num() as u64);
            if (block_proposals.transactions().len() as u64) > limit {
                return StatusCode::ProtocolMessageIsMalformed.with_context(format!(
                    "Transactions count({}) > consensus max_block_proposals_limit({}) * max_uncles_num({})",
                    block_proposals.transactions().len(),
                    shared.consensus().max_block_proposals_limit(),
                    shared.consensus().max_uncles_num(),
                ));
            }
        }

        let unknown_txs: Vec<core::TransactionView> = self
            .message
            .transactions()
            .iter()
            .map(|x| x.to_entity().into_view())
            .filter(|tx| !sync_state.already_known_tx(&tx.hash()))
            .collect();

        if unknown_txs.is_empty() {
            return Status::ignored();
        }

        let proposals: Vec<packed::ProposalShortId> = unknown_txs
            .iter()
            .map(|tx| packed::ProposalShortId::from_tx_hash(&tx.hash()))
            .collect();
        let removes = sync_state.remove_inflight_proposals(&proposals);
        let mut asked_txs = Vec::new();
        for (previously_in, tx) in removes.into_iter().zip(unknown_txs) {
            if previously_in {
                sync_state.mark_as_known_tx(tx.hash());
                asked_txs.push(tx);
            }
        }

        if asked_txs.is_empty() {
            return Status::ignored();
        }

        let tx_pool = self.relayer.shared.shared().tx_pool_controller();
        if let Err(err) = tx_pool.notify_txs(asked_txs, None) {
            warn_target!(
                crate::LOG_TARGET_RELAY,
                "BlockProposal notify_txs error: {:?}",
                err,
            );
        }
        Status::ok()
    }
}
