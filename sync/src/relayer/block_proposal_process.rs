use crate::relayer::Relayer;
use ckb_protocol::{BlockProposal, FlatbuffersVectorIterator};
use ckb_shared::index::ChainIndex;
use ckb_traits::chain_provider::ChainProvider;

pub struct BlockProposalProcess<'a, CI: ChainIndex + 'a> {
    message: &'a BlockProposal<'a>,
    relayer: &'a Relayer<CI>,
}

impl<'a, CI> BlockProposalProcess<'a, CI>
where
    CI: ChainIndex + 'static,
{
    pub fn new(message: &'a BlockProposal, relayer: &'a Relayer<CI>) -> Self {
        BlockProposalProcess { message, relayer }
    }

    pub fn execute(self) {
        let chain_state = self.relayer.shared.chain_state().lock();
        FlatbuffersVectorIterator::new(self.message.transactions().unwrap()).for_each(|tx| {
            let _ = chain_state.add_tx_to_pool(
                tx.into(),
                self.relayer.shared.consensus().max_block_cycles(),
            );
        })
    }
}
