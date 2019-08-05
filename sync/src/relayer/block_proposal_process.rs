use crate::relayer::Relayer;
use ckb_logger::{debug_target, warn_target};
use ckb_network::CKBProtocolContext;
use ckb_types::{core, packed, prelude::*};
use failure::Error as FailureError;
use futures::{self, future::FutureResult, lazy};
use std::sync::Arc;

pub struct BlockProposalProcess<'a> {
    message: packed::BlockProposalReader<'a>,
    relayer: &'a Relayer,
    nc: Arc<dyn CKBProtocolContext>,
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
        nc: Arc<dyn CKBProtocolContext>,
    ) -> Self {
        BlockProposalProcess {
            message,
            relayer,
            nc,
        }
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
            .map(|tx| packed::ProposalShortId::from_tx_hash(&tx.hash().unpack()))
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

        if let Err(err) = self.nc.future_task(
            {
                let tx_pool_executor = Arc::clone(&self.relayer.tx_pool_executor);
                Box::new(lazy(move || -> FutureResult<(), ()> {
                    let ret = tx_pool_executor.verify_and_add_txs_to_pool(asked_txs, None);
                    if ret.is_err() {
                        warn_target!(
                            crate::LOG_TARGET_RELAY,
                            "BlockProposal add_tx_to_pool error {:?}",
                            ret
                        )
                    }
                    futures::future::ok(())
                }))
            },
            true,
        ) {
            debug_target!(
                crate::LOG_TARGET_RELAY,
                "relayer send future task error: {:?}",
                err,
            );
        }
        Ok(Status::Ok)
    }
}
