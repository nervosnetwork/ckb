use crate::relayer::Relayer;
use ckb_protocol::{BlockProposal, FlatbuffersVectorIterator};
use ckb_shared::index::ChainIndex;

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
        FlatbuffersVectorIterator::new(self.message.transactions().unwrap()).for_each(|tx| {
            let _ = self.relayer.shared.add_tx_to_pool(tx.into());
        })
    }
}
