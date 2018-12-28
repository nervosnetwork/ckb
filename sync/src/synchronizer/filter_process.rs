use crate::synchronizer::Synchronizer;
use crate::types::TransactionFilter;
use ckb_network::PeerIndex;
use ckb_protocol::{AddFilter, SetFilter};
use ckb_shared::index::ChainIndex;

pub struct SetFilterProcess<'a, CI: ChainIndex + 'a> {
    message: &'a SetFilter<'a>,
    synchronizer: &'a Synchronizer<CI>,
    peer: PeerIndex,
}

impl<'a, CI> SetFilterProcess<'a, CI>
where
    CI: ChainIndex + 'a,
{
    pub fn new(
        message: &'a SetFilter,
        synchronizer: &'a Synchronizer<CI>,
        peer: PeerIndex,
    ) -> Self {
        Self {
            peer,
            message,
            synchronizer,
        }
    }

    pub fn execute(self) {
        // TODO add filter size and num_hashes max value checking
        let mut filters = self.synchronizer.peers.transaction_filters.write();
        filters.entry(self.peer).or_insert_with(|| {
            TransactionFilter::new(
                self.message.filter().unwrap(),
                self.message.num_hashes() as usize,
                self.message.hash_seed() as usize,
            )
        });
    }
}

pub struct AddFilterProcess<'a, CI: ChainIndex + 'a> {
    message: &'a AddFilter<'a>,
    synchronizer: &'a Synchronizer<CI>,
    peer: PeerIndex,
}

impl<'a, CI> AddFilterProcess<'a, CI>
where
    CI: ChainIndex + 'a,
{
    pub fn new(
        message: &'a AddFilter,
        synchronizer: &'a Synchronizer<CI>,
        peer: PeerIndex,
    ) -> Self {
        Self {
            peer,
            message,
            synchronizer,
        }
    }

    pub fn execute(self) {
        let mut filters = self.synchronizer.peers.transaction_filters.write();
        filters
            .entry(self.peer)
            .and_modify(|filter| filter.update(self.message.filter().unwrap()));
    }
}

pub struct ClearFilterProcess<'a, CI: ChainIndex + 'a> {
    synchronizer: &'a Synchronizer<CI>,
    peer: PeerIndex,
}

impl<'a, CI> ClearFilterProcess<'a, CI>
where
    CI: ChainIndex + 'a,
{
    pub fn new(synchronizer: &'a Synchronizer<CI>, peer: PeerIndex) -> Self {
        Self { peer, synchronizer }
    }

    pub fn execute(self) {
        let mut filters = self.synchronizer.peers.transaction_filters.write();
        filters.remove(&self.peer);
    }
}
