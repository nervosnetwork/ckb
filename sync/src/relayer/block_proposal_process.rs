use crate::relayer::Relayer;
use ckb_core::transaction::{ProposalShortId, Transaction};
use ckb_protocol::{cast, BlockProposal, FlatbuffersVectorIterator};
use ckb_store::ChainStore;
use failure::Error as FailureError;
use log::warn;
use numext_fixed_hash::H256;
use std::convert::TryInto;

pub struct BlockProposalProcess<'a, CS> {
    message: &'a BlockProposal<'a>,
    relayer: &'a Relayer<CS>,
}

impl<'a, CS: ChainStore> BlockProposalProcess<'a, CS> {
    pub fn new(message: &'a BlockProposal, relayer: &'a Relayer<CS>) -> Self {
        BlockProposalProcess { message, relayer }
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
                if self.relayer.state.already_known(&tx_hash) {
                    None
                } else {
                    Some((tx_hash, tx))
                }
            })
            .collect();
        if unknown_txs.is_empty() {
            return Ok(());
        }
        let chain_state = self.relayer.shared.chain_state().lock();
        let mut inflight = self.relayer.state.inflight_proposals.lock();
        for (tx_hash, tx) in unknown_txs {
            if inflight.remove(&ProposalShortId::from_tx_hash(&tx_hash)) {
                self.relayer.state.insert_tx(tx_hash);
                let ret = chain_state.add_tx_to_pool(tx);
                if ret.is_err() {
                    warn!(target: "relay", "BlockProposal add_tx_to_pool error {:?}", ret)
                }
            }
        }
        Ok(())
    }
}
