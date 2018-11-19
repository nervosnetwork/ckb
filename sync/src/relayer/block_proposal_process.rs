use ckb_chain::chain::ChainProvider;
use ckb_chain::PowEngine;
use ckb_protocol::{BlockProposal, FlatbuffersVectorIterator};
use relayer::Relayer;

pub struct BlockProposalProcess<'a, C: 'a, P: 'a> {
    message: &'a BlockProposal<'a>,
    relayer: &'a Relayer<C, P>,
}

impl<'a, C, P> BlockProposalProcess<'a, C, P>
where
    C: ChainProvider + 'static,
    P: PowEngine + 'static,
{
    pub fn new(message: &'a BlockProposal, relayer: &'a Relayer<C, P>) -> Self {
        BlockProposalProcess { message, relayer }
    }

    pub fn execute(self) {
        FlatbuffersVectorIterator::new(self.message.transactions().unwrap()).for_each(|tx| {
            let _ = self.relayer.tx_pool.add_transaction(tx.into());
        })
    }
}
