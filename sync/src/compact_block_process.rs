use ckb_chain::chain::ChainProvider;
use ckb_protocol;
use network::{NetworkContext, PeerId};
use synchronizer::Synchronizer;

pub struct CompactBlockProcess<'a, C: 'a> {
    message: &'a ckb_protocol::CompactBlock,
    synchronizer: &'a Synchronizer<C>,
    peer: PeerId,
    nc: &'a NetworkContext,
}

impl<'a, C> CompactBlockProcess<'a, C>
where
    C: ChainProvider + 'a,
{
    pub fn new(
        message: &'a ckb_protocol::CompactBlock,
        synchronizer: &'a Synchronizer<C>,
        peer: &PeerId,
        nc: &'a NetworkContext,
    ) -> Self {
        CompactBlockProcess {
            message,
            nc,
            synchronizer,
            peer: *peer,
        }
    }

    pub fn execute(self) {}
}
