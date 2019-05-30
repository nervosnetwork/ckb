use crate::relayer::Relayer;
use ckb_core::transaction::{ProposalShortId, Transaction};
use ckb_network::CKBProtocolContext;
use ckb_protocol::{cast, BlockProposal, FlatbuffersVectorIterator};
use ckb_store::ChainStore;
use failure::Error as FailureError;
use futures::{self, future::FutureResult, lazy};
use log::warn;
use numext_fixed_hash::H256;
use std::convert::TryInto;
use std::sync::Arc;

pub struct BlockProposalProcess<'a, CS> {
    message: &'a BlockProposal<'a>,
    relayer: &'a Relayer<CS>,
    nc: Arc<dyn CKBProtocolContext + Sync>,
}

impl<'a, CS: ChainStore + 'static> BlockProposalProcess<'a, CS> {
    pub fn new(
        message: &'a BlockProposal,
        relayer: &'a Relayer<CS>,
        nc: Arc<dyn CKBProtocolContext + Sync>,
    ) -> Self {
        BlockProposalProcess {
            message,
            relayer,
            nc,
        }
    }

    pub fn execute(self) -> Result<(), FailureError> {
        let txs: Vec<Transaction> =
            FlatbuffersVectorIterator::new(cast!(self.message.transactions())?)
                .map(TryInto::try_into)
                .collect::<Result<Vec<Transaction>, _>>()?;

        let unknown_txs: Vec<(H256, Transaction)> = txs
            .into_iter()
            .filter_map(|tx| {
                let tx_hash = tx.hash();
                if self.relayer.already_known_tx(&tx_hash) {
                    None
                } else {
                    Some((tx_hash.to_owned(), tx))
                }
            })
            .collect();
        if unknown_txs.is_empty() {
            return Ok(());
        }
        let mut inflight = self.relayer.inflight_proposals.lock();
        // filter txs that we ask for download
        let asked_txs = unknown_txs
            .into_iter()
            .filter_map(|(tx_hash, tx)| {
                if inflight.remove(&ProposalShortId::from_tx_hash(&tx_hash)) {
                    // mark as known
                    self.relayer.mark_as_known_tx(tx_hash);
                    Some(tx)
                } else {
                    None
                }
            })
            .collect();
        self.nc.future_task({
            let tx_pool_executor = Arc::clone(&self.relayer.tx_pool_executor);
            Box::new(lazy(move || -> FutureResult<(), ()> {
                let ret = tx_pool_executor.verify_and_add_txs_to_pool(asked_txs);
                if ret.is_err() {
                    warn!(target: "relay", "BlockProposal add_tx_to_pool error {:?}", ret)
                }
                futures::future::ok(())
            }))
        });
        Ok(())
    }
}
