use ckb_protocol::Block;
use ckb_shared::index::ChainIndex;
use network::{CKBProtocolContext, PeerIndex};
use synchronizer::Synchronizer;

pub struct BlockProcess<'a, CI: ChainIndex + 'a> {
    message: &'a Block<'a>,
    synchronizer: &'a Synchronizer<CI>,
    peer: PeerIndex,
    // nc: &'a CKBProtocolContext,
}

impl<'a, CI> BlockProcess<'a, CI>
where
    CI: ChainIndex + 'a,
{
    pub fn new(
        message: &'a Block,
        synchronizer: &'a Synchronizer<CI>,
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
