use ckb_chain::chain::ChainProvider;
use ckb_protocol;
use core::block::IndexedBlock;
use network::{NetworkContext, PeerId};
use synchronizer::Synchronizer;

pub struct BlockProcess<'a, C: 'a> {
    message: &'a ckb_protocol::Block,
    synchronizer: &'a Synchronizer<C>,
    peer: PeerId,
    // nc: &'a NetworkContext,
}

impl<'a, C> BlockProcess<'a, C>
where
    C: ChainProvider + 'a,
{
    pub fn new(
        message: &'a ckb_protocol::Block,
        synchronizer: &'a Synchronizer<C>,
        peer: &PeerId,
        _nc: &'a NetworkContext,
    ) -> Self {
        BlockProcess {
            message,
            synchronizer,
            peer: *peer,
        }
    }

    pub fn execute(self) {
        let block: IndexedBlock = self.message.into();
        debug!(target: "sync", "handle_block from peer {} {:?}", self.peer, block);

        self.synchronizer.peers.block_received(self.peer, &block);

        self.synchronizer.process_new_block(self.peer, block);
    }
}
