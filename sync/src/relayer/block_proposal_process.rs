use ckb_chain::chain::ChainProvider;
use ckb_protocol::{BlockProposal, FlatbuffersVectorIterator};
use relayer::Relayer;

pub struct BlockProposalProcess<'a, C: 'a> {
    message: &'a BlockProposal<'a>,
    relayer: &'a Relayer<C>,
}

impl<'a, C> BlockProposalProcess<'a, C>
where
    C: ChainProvider + 'static,
{
    pub fn new(message: &'a BlockProposal, relayer: &'a Relayer<C>) -> Self {
        BlockProposalProcess { message, relayer }
    }

    pub fn execute(self) {
        FlatbuffersVectorIterator::new(self.message.transactions().unwrap()).for_each(|tx| {
            let _ = self.relayer.tx_pool.add_transaction(tx.into());
        })
    }
}
