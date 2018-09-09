use ckb_chain::chain::ChainProvider;
use ckb_chain::PowEngine;
use ckb_protocol;
use network::{NetworkContext, PeerId};
use synchronizer::Synchronizer;

pub struct CompactBlockProcess<'a, C: 'a, P: 'a> {
    message: &'a ckb_protocol::CompactBlock,
    synchronizer: &'a Synchronizer<C, P>,
    peer: PeerId,
    nc: &'a NetworkContext,
}

impl<'a, C, P> CompactBlockProcess<'a, C, P>
where
    C: ChainProvider + 'a,
    P: PowEngine + 'a,
{
    pub fn new(
        message: &'a ckb_protocol::CompactBlock,
        synchronizer: &'a Synchronizer<C, P>,
        peer: PeerId,
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
