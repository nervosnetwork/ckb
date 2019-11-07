use crate::relayer::Relayer;
use ckb_logger::{warn, warn_target};
use ckb_network::PeerIndex;
use ckb_types::{core, packed, prelude::*};
use failure::{err_msg, Error as FailureError};

pub struct BlockProposalProcess<'a> {
    message: packed::BlockProposalReader<'a>,
    relayer: &'a Relayer,
    peer: PeerIndex,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Status {
    NoUnknown,
    NoAsked,
    Ok,
}

impl<'a> BlockProposalProcess<'a> {
    pub fn new(
        message: packed::BlockProposalReader<'a>,
        relayer: &'a Relayer,
        peer: PeerIndex,
    ) -> Self {
        BlockProposalProcess {
            message,
            relayer,
            peer,
        }
    }

    pub fn execute(self) -> Result<Status, FailureError> {
        let snapshot = self.relayer.shared().snapshot();
        {
            let block_proposals = self.message;
            let limit = snapshot.consensus().max_block_proposals_limit()
                * (snapshot.consensus().max_uncles_num() as u64);
            if (block_proposals.transactions().len() as u64) > limit {
                warn!("Peer {} sends us an invalid message, BlockProposal transactions size ({}) is greater than consensus limit ({})",
                    self.peer, block_proposals.transactions().len(), limit);
                return Err(err_msg(
                    "BlockProposal transactions size is greater than consensus limit".to_owned(),
                ));
            }
        }

        let unknown_txs: Vec<core::TransactionView> = self
            .message
            .transactions()
            .iter()
            .map(|x| x.to_entity().into_view())
            .filter(|tx| !snapshot.state().already_known_tx(&tx.hash()))
            .collect();

        if unknown_txs.is_empty() {
            return Ok(Status::NoUnknown);
        }

        let proposals: Vec<packed::ProposalShortId> = unknown_txs
            .iter()
            .map(|tx| packed::ProposalShortId::from_tx_hash(&tx.hash()))
            .collect();
        let removes = snapshot.state().remove_inflight_proposals(&proposals);
        let mut asked_txs = Vec::new();
        for (previously_in, tx) in removes.into_iter().zip(unknown_txs) {
            if previously_in {
                snapshot.state().mark_as_known_tx(tx.hash());
                asked_txs.push(tx);
            }
        }

        if asked_txs.is_empty() {
            return Ok(Status::NoAsked);
        }

        let tx_pool = self.relayer.shared.shared().tx_pool_controller();
        if let Err(err) = tx_pool.notify_txs(asked_txs, None) {
            warn_target!(
                crate::LOG_TARGET_RELAY,
                "BlockProposal notify_txs error: {:?}",
                err,
            );
        }
        Ok(Status::Ok)
    }
}
