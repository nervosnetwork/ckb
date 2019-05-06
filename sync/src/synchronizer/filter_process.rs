use crate::synchronizer::Synchronizer;
use crate::types::TransactionFilter;
use ckb_network::PeerIndex;
use ckb_protocol::{cast, AddFilter, SetFilter};
use ckb_shared::store::ChainStore;
use failure::Error as FailureError;

pub struct SetFilterProcess<'a, CS: ChainStore + 'a> {
    message: &'a SetFilter<'a>,
    synchronizer: &'a Synchronizer<CS>,
    peer: PeerIndex,
}

impl<'a, CS> SetFilterProcess<'a, CS>
where
    CS: ChainStore + 'a,
{
    pub fn new(
        message: &'a SetFilter,
        synchronizer: &'a Synchronizer<CS>,
        peer: PeerIndex,
    ) -> Self {
        Self {
            peer,
            message,
            synchronizer,
        }
    }

    pub fn execute(self) -> Result<(), FailureError> {
        // TODO add filter size and num_hashes max value checking
        let mut filters = self.synchronizer.peers.transaction_filters.write();
        let msg = cast!(self.message.filter())?;
        filters.entry(self.peer).or_insert_with(|| {
            TransactionFilter::new(
                msg,
                self.message.num_hashes() as usize,
                self.message.hash_seed() as usize,
            )
        });
        Ok(())
    }
}

pub struct AddFilterProcess<'a, CS: ChainStore + 'a> {
    message: &'a AddFilter<'a>,
    synchronizer: &'a Synchronizer<CS>,
    peer: PeerIndex,
}

impl<'a, CS> AddFilterProcess<'a, CS>
where
    CS: ChainStore + 'a,
{
    pub fn new(
        message: &'a AddFilter,
        synchronizer: &'a Synchronizer<CS>,
        peer: PeerIndex,
    ) -> Self {
        Self {
            peer,
            message,
            synchronizer,
        }
    }

    pub fn execute(self) -> Result<(), FailureError> {
        let mut filters = self.synchronizer.peers.transaction_filters.write();
        let msg = cast!(self.message.filter())?;
        filters
            .entry(self.peer)
            .and_modify(|filter| filter.update(msg));
        Ok(())
    }
}

pub struct ClearFilterProcess<'a, CS: ChainStore + 'a> {
    synchronizer: &'a Synchronizer<CS>,
    peer: PeerIndex,
}

impl<'a, CS> ClearFilterProcess<'a, CS>
where
    CS: ChainStore + 'a,
{
    pub fn new(synchronizer: &'a Synchronizer<CS>, peer: PeerIndex) -> Self {
        Self { peer, synchronizer }
    }

    pub fn execute(self) -> Result<(), FailureError> {
        let mut filters = self.synchronizer.peers.transaction_filters.write();
        filters.remove(&self.peer);
        Ok(())
    }
}
