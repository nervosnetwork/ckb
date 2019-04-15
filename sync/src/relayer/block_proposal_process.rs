use crate::relayer::Relayer;
use ckb_protocol::{cast, BlockProposal, FlatbuffersVectorIterator};
use ckb_shared::store::ChainStore;
use ckb_traits::chain_provider::ChainProvider;
use failure::Error as FailureError;
use log::warn;
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
        let chain_state = self.relayer.shared.chain_state().lock();
        let txs = FlatbuffersVectorIterator::new(cast!(self.message.transactions())?);
        for tx in txs {
            let ret = chain_state.add_tx_to_pool(
                TryInto::try_into(tx)?,
                self.relayer.shared.consensus().max_block_cycles(),
            );
            if ret.is_err() {
                warn!(target: "relay", "BlockProposal add_tx_to_pool error {:?}", ret)
            }
        }
        Ok(())
    }
}
