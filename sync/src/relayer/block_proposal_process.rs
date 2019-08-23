use crate::relayer::Relayer;
use ckb_logger::warn_target;
use ckb_types::{core, packed, prelude::*};
use failure::Error as FailureError;
use futures::future::Future;

pub struct BlockProposalProcess<'a> {
    message: packed::BlockProposalReader<'a>,
    relayer: &'a Relayer,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Status {
    NoUnknown,
    NoAsked,
    Ok,
}

impl<'a> BlockProposalProcess<'a> {
    pub fn new(message: packed::BlockProposalReader<'a>, relayer: &'a Relayer) -> Self {
        BlockProposalProcess { message, relayer }
    }

    pub fn execute(self) -> Result<Status, FailureError> {
        let unknown_txs: Vec<core::TransactionView> = self
            .message
            .transactions()
            .iter()
            .map(|x| x.to_entity().into_view())
            .filter(|tx| !self.relayer.shared().already_known_tx(&tx.hash()))
            .collect();

        if unknown_txs.is_empty() {
            return Ok(Status::NoUnknown);
        }

        let proposals: Vec<packed::ProposalShortId> = unknown_txs
            .iter()
            .map(|tx| packed::ProposalShortId::from_tx_hash(&tx.hash()))
            .collect();
        let removes = self.relayer.shared().remove_inflight_proposals(&proposals);
        let mut asked_txs = Vec::new();
        for (previously_in, tx) in removes.into_iter().zip(unknown_txs) {
            if previously_in {
                self.relayer.shared().mark_as_known_tx(tx.hash());
                asked_txs.push(tx);
            }
        }

        if asked_txs.is_empty() {
            return Ok(Status::NoAsked);
        }

        let tx_pool = self.relayer.shared.shared().tx_pool_controller();
        if let Err(err) = tx_pool.submit_txs(asked_txs).unwrap().wait().unwrap() {
            warn_target!(
                crate::LOG_TARGET_RELAY,
                "BlockProposal submit_txs error: {:?}",
                err,
            );
        }

        Ok(Status::Ok)
    }
}
