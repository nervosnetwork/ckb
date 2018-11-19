use ckb_chain::chain::ChainProvider;
use ckb_protocol::Block;
use network::{CKBProtocolContext, PeerIndex};
use synchronizer::Synchronizer;

pub struct BlockProcess<'a, C: 'a> {
    message: &'a Block<'a>,
    synchronizer: &'a Synchronizer<C>,
    peer: PeerIndex,
    // nc: &'a CKBProtocolContext,
}

impl<'a, C> BlockProcess<'a, C>
where
    C: ChainProvider + 'a,
{
    pub fn new(
        message: &'a Block,
        synchronizer: &'a Synchronizer<C>,
        peer: PeerIndex,
        _nc: &'a CKBProtocolContext,
    ) -> Self {
        BlockProcess {
            message,
            synchronizer,
            peer,
        }
    }

    pub fn execute(self) {
        let block = (*self.message).into();

        self.synchronizer.peers.block_received(self.peer, &block);
        self.synchronizer.process_new_block(self.peer, block);
    }
}
